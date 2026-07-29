//! OCR compatibility surface used when the `ocr` feature is disabled.
//!
//! Keeping the configuration DTO available lets parsing and host code compile
//! without pulling ONNX Runtime into minimal builds. Execution APIs fail
//! explicitly instead of silently pretending OCR ran.

use std::sync::Arc;

use crate::error::CoreError;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrConfig {
    pub enabled: bool,
    pub confidence_threshold: f32,
    pub llm_fallback_enabled: bool,
    pub det_limit_side_len: u32,
    pub use_cls: bool,
    pub model_path: String,
    pub languages: Vec<String>,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            confidence_threshold: 0.6,
            llm_fallback_enabled: false,
            det_limit_side_len: 960,
            use_cls: false,
            model_path: String::new(),
            languages: vec!["en".into(), "zh".into()],
        }
    }
}

#[derive(Debug, Clone)]
pub struct OcrTextRegion {
    pub text: String,
    pub confidence: f32,
    pub bbox: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct OcrResult {
    pub regions: Vec<OcrTextRegion>,
    pub full_text: String,
    pub avg_confidence: f32,
    pub source: OcrSource,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OcrSource {
    PaddleOcr,
    LlmVision,
    None,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrDownloadProgress {
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub file_index: usize,
    pub total_files: usize,
}

pub struct OcrEngine;

fn disabled_error() -> CoreError {
    CoreError::Ocr("OCR support is disabled in this build".to_string())
}

pub fn ocr_engine(_config: &OcrConfig) -> Result<Arc<OcrEngine>, CoreError> {
    Err(disabled_error())
}

pub fn extract_text_from_image(
    _image_bytes: &[u8],
    _mime_type: &str,
    _config: &OcrConfig,
    _llm_provider: Option<&dyn crate::llm::LlmProvider>,
) -> Result<OcrResult, CoreError> {
    Err(disabled_error())
}

pub fn extract_text_from_image_with_llm_provider_type(
    _image_bytes: &[u8],
    _mime_type: &str,
    _config: &OcrConfig,
    _llm_provider: Option<&dyn crate::llm::LlmProvider>,
    _llm_provider_type: Option<crate::llm::ProviderType>,
) -> Result<OcrResult, CoreError> {
    Err(disabled_error())
}

pub fn ocr_pdf(
    _pdf_bytes: &[u8],
    _config: &OcrConfig,
    _llm_provider: Option<&dyn crate::llm::LlmProvider>,
) -> Result<String, CoreError> {
    Err(disabled_error())
}

pub fn ocr_pdf_with_llm_provider_type(
    _pdf_bytes: &[u8],
    _config: &OcrConfig,
    _llm_provider: Option<&dyn crate::llm::LlmProvider>,
    _llm_provider_type: Option<crate::llm::ProviderType>,
) -> Result<String, CoreError> {
    Err(disabled_error())
}

pub(crate) fn extract_images_from_pdf_page(
    _doc: &lopdf::Document,
    _page_id: lopdf::ObjectId,
) -> Vec<image::DynamicImage> {
    Vec::new()
}

pub fn check_ocr_models_exist(_config: &OcrConfig) -> bool {
    false
}

pub fn delete_ocr_models(_config: &OcrConfig) -> Result<(), CoreError> {
    Ok(())
}

pub fn download_ocr_models(
    _config: &OcrConfig,
    _hf_mirror_base: &str,
    _ghproxy_base: &str,
    _on_progress: impl Fn(OcrDownloadProgress),
) -> Result<(), CoreError> {
    Err(disabled_error())
}
