//! Lightweight API-key encryption using AES-256-GCM with a machine-derived key.
//!
//! The key is derived deterministically from the local machine's hostname and
//! username via `blake3::Hasher::new_derive_key`.  This is "better than
//! plaintext" — it prevents casual extraction by anyone who copies the DB
//! file but does **not** defend against a determined attacker with full access
//! to the same user session.
//!
//! Encrypted values are stored as `enc:v1:<base64(nonce ‖ ciphertext)>`.
//! Values that do not carry the prefix are treated as legacy plaintext and
//! will be transparently encrypted on the next write.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};

use crate::error::CoreError;

/// Prefix that marks an already-encrypted value.
const ENCRYPTED_PREFIX: &str = "enc:v1:";

/// Derive a 256-bit key unique to this OS user + machine.
///
/// Uses `blake3::Hasher::new_derive_key` (a proper KDF) seeded with a
/// context string, a hardcoded salt, the hostname, and the username.
fn derive_machine_key() -> [u8; 32] {
    let hostname = std::env::var("COMPUTERNAME") // Windows
        .or_else(|_| std::env::var("HOSTNAME")) // Linux
        .unwrap_or_else(|_| "unknown-host".into());

    let username = std::env::var("USERNAME") // Windows
        .or_else(|_| std::env::var("USER")) // Unix
        .unwrap_or_else(|_| "unknown-user".into());

    // SECURITY: The two strings below are cryptographic domain separators, NOT brand tokens.
    // They derive the AES-GCM key used to encrypt users' saved API keys. Renaming them
    // (e.g., as part of the Nexa rebrand) would silently corrupt all existing encrypted
    // API keys on users' machines. They are intentionally preserved across the rebrand.
    // DO NOT MODIFY.
    let mut hasher = blake3::Hasher::new_derive_key("ask-myself api-key encryption v1");
    hasher.update(b"ask-myself-salt-2025");
    hasher.update(hostname.as_bytes());
    hasher.update(b"||");
    hasher.update(username.as_bytes());
    *hasher.finalize().as_bytes()
}

/// Encrypt an API key for storage.
///
/// Returns the original string unchanged if it is empty or already encrypted.
pub fn encrypt_api_key(plaintext: &str) -> Result<String, CoreError> {
    if plaintext.is_empty() || plaintext.starts_with(ENCRYPTED_PREFIX) {
        return Ok(plaintext.to_string());
    }

    let key_bytes = derive_machine_key();
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| CoreError::Internal(format!("crypto key init: {e}")))?;

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| CoreError::Internal(format!("encryption failed: {e}")))?;

    // nonce (12 bytes) ‖ ciphertext
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);

    Ok(format!("{ENCRYPTED_PREFIX}{}", B64.encode(&combined)))
}

/// Decrypt a stored API key.
///
/// If the value does not carry the encrypted prefix it is assumed to be
/// legacy plaintext and is returned as-is (the caller should re-encrypt it).
pub fn decrypt_api_key(stored: &str) -> Result<String, CoreError> {
    if stored.is_empty() || !stored.starts_with(ENCRYPTED_PREFIX) {
        return Ok(stored.to_string());
    }

    let encoded = &stored[ENCRYPTED_PREFIX.len()..];
    let combined = B64
        .decode(encoded)
        .map_err(|e| CoreError::Internal(format!("base64 decode: {e}")))?;

    if combined.len() < 12 {
        return Err(CoreError::Internal(
            "encrypted payload too short".to_string(),
        ));
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let key_bytes = derive_machine_key();
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| CoreError::Internal(format!("crypto key init: {e}")))?;

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CoreError::Internal(format!("decryption failed: {e}")))?;

    String::from_utf8(plaintext).map_err(|e| CoreError::Internal(format!("utf-8 decode: {e}")))
}

/// Returns `true` when the value is already encrypted.
pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(ENCRYPTED_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = "sk-test-1234567890";
        let encrypted = encrypt_api_key(key).unwrap();
        assert!(encrypted.starts_with(ENCRYPTED_PREFIX));
        assert_ne!(encrypted, key);

        let decrypted = decrypt_api_key(&encrypted).unwrap();
        assert_eq!(decrypted, key);
    }

    #[test]
    fn empty_passthrough() {
        assert_eq!(encrypt_api_key("").unwrap(), "");
        assert_eq!(decrypt_api_key("").unwrap(), "");
    }

    #[test]
    fn already_encrypted_not_double_encrypted() {
        let key = "sk-test";
        let encrypted = encrypt_api_key(key).unwrap();
        let double = encrypt_api_key(&encrypted).unwrap();
        assert_eq!(encrypted, double);
    }

    #[test]
    fn plaintext_decrypts_to_itself() {
        let plain = "sk-plain-key";
        assert_eq!(decrypt_api_key(plain).unwrap(), plain);
    }

    #[test]
    fn unique_nonces() {
        let key = "sk-test";
        let a = encrypt_api_key(key).unwrap();
        let b = encrypt_api_key(key).unwrap();
        // Different nonces → different ciphertexts
        assert_ne!(a, b);
        // Both decrypt to the same value
        assert_eq!(decrypt_api_key(&a).unwrap(), key);
        assert_eq!(decrypt_api_key(&b).unwrap(), key);
    }
}
