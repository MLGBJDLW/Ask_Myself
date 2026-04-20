//! Image processing utilities for LLM submission.
//!
//! Handles resizing, format conversion, and compression of images before
//! sending them to LLM providers. All providers have different limits but
//! we normalise to a safe common baseline.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::error::CoreError;

/// Maximum image dimension (width or height) for LLM submission.
const MAX_DIMENSION: u32 = 1568; // Anthropic's recommended max
/// JPEG quality for compressed images (0-100).
const JPEG_QUALITY: u8 = 80;
/// Maximum image file size in bytes (before base64) — ~5 MB.
const MAX_IMAGE_SIZE: usize = 5 * 1024 * 1024;

/// Returns `true` if the MIME type is a supported image format.
pub fn is_supported_image(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Process raw image bytes for LLM submission.
///
/// 1. Decode from raw bytes (auto-detect format).
/// 2. Resize if any dimension exceeds [`MAX_DIMENSION`] (aspect-ratio preserved).
/// 3. Re-encode as JPEG at [`JPEG_QUALITY`].
/// 4. Base64-encode the result.
///
/// Returns `(base64_data, media_type)`.
///
/// GIF images are kept as-is (they may be animated) — only base64-encoded.
pub fn prepare_image_for_llm(
    data: &[u8],
    original_mime: &str,
) -> Result<(String, String), CoreError> {
    if data.len() > MAX_IMAGE_SIZE {
        return Err(CoreError::Llm(format!(
            "Image too large: {} bytes (max {})",
            data.len(),
            MAX_IMAGE_SIZE
        )));
    }

    // GIF: pass through as-is (may be animated).
    if original_mime == "image/gif" {
        return Ok((BASE64.encode(data), "image/gif".to_string()));
    }

    let img = image::load_from_memory(data)
        .map_err(|e| CoreError::Llm(format!("Failed to decode image: {e}")))?;

    let (w, h) = (img.width(), img.height());

    // Resize if needed, preserving aspect ratio.
    let img = if w > MAX_DIMENSION || h > MAX_DIMENSION {
        img.resize(
            MAX_DIMENSION,
            MAX_DIMENSION,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    // Encode as JPEG.
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    img.write_with_encoder(encoder)
        .map_err(|e| CoreError::Llm(format!("Failed to encode image as JPEG: {e}")))?;

    Ok((BASE64.encode(buf.into_inner()), "image/jpeg".to_string()))
}

/// Process a base64-encoded image for LLM submission.
///
/// Decodes the base64 payload, then delegates to [`prepare_image_for_llm`].
pub fn prepare_base64_image_for_llm(
    base64_data: &str,
    media_type: &str,
) -> Result<(String, String), CoreError> {
    let raw = BASE64
        .decode(base64_data)
        .map_err(|e| CoreError::Llm(format!("Invalid base64 image data: {e}")))?;
    prepare_image_for_llm(&raw, media_type)
}

/// Rough token-cost estimate for an image.
///
/// Based on OpenAI's tiling model: ~85 tokens per 512×512 tile plus a
/// base cost of 85 tokens.
pub fn estimate_image_tokens(width: u32, height: u32) -> u32 {
    let tiles_x = width.div_ceil(512);
    let tiles_y = height.div_ceil(512);
    tiles_x * tiles_y * 85 + 85
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_image() {
        assert!(is_supported_image("image/jpeg"));
        assert!(is_supported_image("image/png"));
        assert!(is_supported_image("image/gif"));
        assert!(is_supported_image("image/webp"));
        assert!(!is_supported_image("image/bmp"));
        assert!(!is_supported_image("text/plain"));
    }

    #[test]
    fn test_estimate_image_tokens() {
        // 512×512 → 1 tile → 85 + 85 = 170
        assert_eq!(estimate_image_tokens(512, 512), 170);
        // 1024×1024 → 4 tiles → 4*85 + 85 = 425
        assert_eq!(estimate_image_tokens(1024, 1024), 425);
    }

    #[test]
    fn test_prepare_small_jpeg() {
        // Create a tiny 2×2 JPEG in memory.
        let img = image::RgbImage::from_fn(2, 2, |_, _| image::Rgb([128u8, 64, 32]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        let raw = buf.into_inner();

        let (b64, mime) = prepare_image_for_llm(&raw, "image/jpeg").unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(!b64.is_empty());
        // Should decode back to valid bytes.
        let decoded = BASE64.decode(&b64).unwrap();
        assert!(!decoded.is_empty());
    }

    #[test]
    fn test_gif_passthrough() {
        let data = b"GIF89a fake gif data";
        let (b64, mime) = prepare_image_for_llm(data, "image/gif").unwrap();
        assert_eq!(mime, "image/gif");
        assert_eq!(BASE64.decode(&b64).unwrap(), data);
    }

    #[test]
    fn test_rejects_oversized() {
        let data = vec![0u8; MAX_IMAGE_SIZE + 1];
        let err = prepare_image_for_llm(&data, "image/png");
        assert!(err.is_err());
    }
}
