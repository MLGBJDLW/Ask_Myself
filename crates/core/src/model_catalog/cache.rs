use std::collections::HashMap;
use std::fmt;

use super::ModelCatalogSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CatalogCacheKey {
    provider_id: String,
    endpoint_id: String,
    credential_fingerprint_hash: String,
}

impl CatalogCacheKey {
    pub fn new(
        provider_id: impl AsRef<str>,
        endpoint_id: impl AsRef<str>,
        credential_fingerprint: impl AsRef<str>,
    ) -> Self {
        let digest = blake3::hash(credential_fingerprint.as_ref().as_bytes());
        Self {
            provider_id: normalize(provider_id.as_ref()),
            endpoint_id: normalize(endpoint_id.as_ref()),
            credential_fingerprint_hash: digest.to_hex()[..16].to_string(),
        }
    }
}

impl fmt::Display for CatalogCacheKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}::{}::{}",
            self.provider_id, self.endpoint_id, self.credential_fingerprint_hash
        )
    }
}

#[derive(Debug, Default)]
pub struct LastGoodCatalogCache {
    snapshots: HashMap<CatalogCacheKey, ModelCatalogSnapshot>,
}

impl LastGoodCatalogCache {
    pub fn get(&self, key: &CatalogCacheKey) -> Option<&ModelCatalogSnapshot> {
        self.snapshots.get(key)
    }

    pub fn store_if_success(&mut self, key: CatalogCacheKey, snapshot: ModelCatalogSnapshot) {
        if snapshot.live_discovery_succeeded {
            self.snapshots.insert(key, snapshot);
        }
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}
