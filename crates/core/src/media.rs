//! Image processing utilities for LLM submission.
//!
//! Handles resizing, format conversion, and compression of images before
//! sending them to LLM providers. All providers have different limits but
//! we normalise to a safe common baseline.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::error::CoreError;

/// Maximum image dimension (width or height) for LLM submission.
///
/// Producers whose pixel coordinates are exposed to a model must use this
/// same envelope before they publish coordinate metadata. Re-normalizing
/// those images may change their encoding, but must not change their pixel
/// dimensions.
pub const MAX_LLM_IMAGE_DIMENSION: u32 = 1_568;
/// Hard pre-decode and native-capture dimension limit for browser pixels.
pub const MAX_BROWSER_NATIVE_CAPTURE_EDGE: u32 = 8_192;
/// Hard native/decoded pixel budget for one browser observation.
pub const MAX_BROWSER_NATIVE_CAPTURE_PIXELS: u64 = 16_000_000;
/// Maximum encoded native PNG accepted before final normalization.
pub const MAX_BROWSER_NATIVE_CAPTURE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum encoded image emitted by the browser capture finalizer.
pub const MAX_BROWSER_FINAL_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
/// JPEG quality for compressed images (0-100).
const JPEG_QUALITY: u8 = 80;
/// Maximum local image input before it is normalized to the shared 1568px,
/// JPEG-80 provider/UI envelope. Tool screenshots can be larger than ordinary
/// uploads, but never reach a model or tool card in their original form.
const MAX_IMAGE_SIZE: usize = 12 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedBrowserCapture {
    pub image_bytes: Vec<u8>,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

pub fn validate_browser_capture_dimensions(width: u32, height: u32) -> Result<(), CoreError> {
    if width == 0 || height == 0 {
        return Err(CoreError::InvalidInput(
            "Browser capture reported empty pixel dimensions".to_string(),
        ));
    }
    if width > MAX_BROWSER_NATIVE_CAPTURE_EDGE || height > MAX_BROWSER_NATIVE_CAPTURE_EDGE {
        return Err(CoreError::InvalidInput(format!(
            "Browser capture exceeds the {MAX_BROWSER_NATIVE_CAPTURE_EDGE}px native edge limit"
        )));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            CoreError::InvalidInput("Browser capture pixel dimensions overflowed".to_string())
        })?;
    if pixels > MAX_BROWSER_NATIVE_CAPTURE_PIXELS {
        return Err(CoreError::InvalidInput(format!(
            "Browser capture exceeds the {MAX_BROWSER_NATIVE_CAPTURE_PIXELS}-pixel budget"
        )));
    }
    Ok(())
}

/// Fully validate and normalize one native browser PNG for the current-turn
/// visual channel. This is the sole encoding finalizer used by every desktop
/// WebView adapter: small model-sized PNGs stay byte-identical, while larger
/// captures become JPEG-80 inside the shared model dimension envelope.
pub fn finalize_browser_capture(png_bytes: Vec<u8>) -> Result<FinalizedBrowserCapture, CoreError> {
    if png_bytes.len() > MAX_BROWSER_NATIVE_CAPTURE_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Browser capture exceeds the {MAX_BROWSER_NATIVE_CAPTURE_BYTES}-byte native limit"
        )));
    }
    if !matches!(image::guess_format(&png_bytes), Ok(image::ImageFormat::Png)) {
        return Err(CoreError::InvalidInput(
            "Browser capture was not a PNG image".to_string(),
        ));
    }

    let decoding_limits = || {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_BROWSER_NATIVE_CAPTURE_EDGE);
        limits.max_image_height = Some(MAX_BROWSER_NATIVE_CAPTURE_EDGE);
        limits.max_alloc = Some(MAX_BROWSER_NATIVE_CAPTURE_PIXELS.saturating_mul(8));
        limits
    };
    let mut dimensions_reader = image::ImageReader::with_format(
        std::io::Cursor::new(png_bytes.as_slice()),
        image::ImageFormat::Png,
    );
    dimensions_reader.limits(decoding_limits());
    let (declared_width, declared_height) =
        dimensions_reader.into_dimensions().map_err(|error| {
            CoreError::InvalidInput(format!(
                "Browser capture PNG dimensions could not be decoded: {error}"
            ))
        })?;
    validate_browser_capture_dimensions(declared_width, declared_height)?;

    let mut reader = image::ImageReader::with_format(
        std::io::Cursor::new(png_bytes.as_slice()),
        image::ImageFormat::Png,
    );
    reader.limits(decoding_limits());
    let decoded = reader.decode().map_err(|error| {
        CoreError::InvalidInput(format!(
            "Browser capture PNG could not be fully decoded: {error}"
        ))
    })?;
    let (decoded_width, decoded_height) = (decoded.width(), decoded.height());
    validate_browser_capture_dimensions(decoded_width, decoded_height)?;
    if (decoded_width, decoded_height) != (declared_width, declared_height) {
        return Err(CoreError::InvalidInput(
            "Browser capture PNG dimensions changed during decode".to_string(),
        ));
    }

    if decoded_width <= MAX_LLM_IMAGE_DIMENSION
        && decoded_height <= MAX_LLM_IMAGE_DIMENSION
        && png_bytes.len() <= MAX_BROWSER_FINAL_CAPTURE_BYTES
    {
        return Ok(FinalizedBrowserCapture {
            image_bytes: png_bytes,
            mime_type: "image/png".to_string(),
            width: decoded_width,
            height: decoded_height,
        });
    }

    let normalized = decoded.resize(
        MAX_LLM_IMAGE_DIMENSION,
        MAX_LLM_IMAGE_DIMENSION,
        image::imageops::FilterType::Lanczos3,
    );
    let (width, height) = (normalized.width(), normalized.height());
    let mut encoded = std::io::Cursor::new(Vec::new());
    normalized
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut encoded,
            JPEG_QUALITY,
        ))
        .map_err(|error| {
            CoreError::Internal(format!(
                "Could not encode normalized browser capture: {error}"
            ))
        })?;
    let image_bytes = encoded.into_inner();
    if image_bytes.is_empty() || image_bytes.len() > MAX_BROWSER_FINAL_CAPTURE_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Normalized browser capture exceeds the {}-byte output limit",
            MAX_BROWSER_FINAL_CAPTURE_BYTES
        )));
    }
    Ok(FinalizedBrowserCapture {
        image_bytes,
        mime_type: "image/jpeg".to_string(),
        width,
        height,
    })
}

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
    let img = if w > MAX_LLM_IMAGE_DIMENSION || h > MAX_LLM_IMAGE_DIMENSION {
        img.resize(
            MAX_LLM_IMAGE_DIMENSION,
            MAX_LLM_IMAGE_DIMENSION,
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

    fn real_png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("test fixture must be a real PNG");
        encoded.into_inner()
    }

    fn png_chunk(bytes: &[u8], kind: &[u8; 4]) -> (usize, usize, usize) {
        let kind_offset = bytes
            .windows(4)
            .position(|window| window == kind)
            .expect("PNG fixture must contain requested chunk");
        let length_offset = kind_offset - 4;
        let length = u32::from_be_bytes(
            bytes[length_offset..kind_offset]
                .try_into()
                .expect("chunk length"),
        ) as usize;
        (kind_offset, kind_offset + 4, kind_offset + 4 + length)
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn browser_capture_finalizer_preserves_only_fully_decoded_small_pngs() {
        let png = real_png(32, 24);
        let finalized = finalize_browser_capture(png.clone()).expect("valid PNG must finalize");

        assert_eq!(finalized.mime_type, "image/png");
        assert_eq!(finalized.image_bytes, png);
        assert_eq!((finalized.width, finalized.height), (32, 24));
    }

    #[test]
    fn browser_capture_finalizer_rejects_truncation_and_crc_corruption() {
        let png = real_png(32, 24);
        let (_kind, data_start, data_end) = png_chunk(&png, b"IDAT");

        let truncated = png[..data_start + (data_end - data_start) / 2].to_vec();
        assert!(finalize_browser_capture(truncated).is_err());

        let mut corrupt_crc = png;
        corrupt_crc[data_end] ^= 0x80;
        assert!(finalize_browser_capture(corrupt_crc).is_err());
    }

    #[test]
    fn browser_capture_finalizer_rejects_decompression_bomb_dimensions_before_decode() {
        let mut bomb = real_png(1, 1);
        bomb[16..20].copy_from_slice(&8_192_u32.to_be_bytes());
        bomb[20..24].copy_from_slice(&8_192_u32.to_be_bytes());
        let ihdr_crc = crc32(&bomb[12..29]);
        bomb[29..33].copy_from_slice(&ihdr_crc.to_be_bytes());

        let error = finalize_browser_capture(bomb).expect_err("pixel bomb must fail closed");
        assert!(error.to_string().contains("pixel budget"));
    }

    #[test]
    fn browser_capture_finalizer_normalizes_large_images_to_shared_jpeg_envelope() {
        let png = real_png(2_000, 100);
        let finalized = finalize_browser_capture(png).expect("bounded PNG must normalize");

        assert_eq!(finalized.mime_type, "image/jpeg");
        assert!(finalized.width <= MAX_LLM_IMAGE_DIMENSION);
        assert!(finalized.height <= MAX_LLM_IMAGE_DIMENSION);
        let decoded = image::load_from_memory(&finalized.image_bytes).expect("valid JPEG output");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (finalized.width, finalized.height)
        );
    }
}
