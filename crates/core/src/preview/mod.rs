mod docx;
pub mod model;
mod pdf;
mod xlsx;

use std::path::{Path, PathBuf};

pub use model::{PreviewCapabilities, StructuredPreview};

#[derive(Debug, Clone, Default)]
pub struct PreviewBuildOptions {
    pub asset_cache_dir: Option<PathBuf>,
}

pub fn build_structured_preview(
    path: &Path,
    mime_type: &str,
    content_hash: &str,
    options: &PreviewBuildOptions,
) -> Result<Option<StructuredPreview>, String> {
    match mime_type {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            docx::preview_docx(path, content_hash, options).map(Some)
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("xlsx"))
            {
                xlsx::preview_xlsx(path).map(Some)
            } else {
                Ok(None)
            }
        }
        "application/pdf" => pdf::preview_pdf(path),
        _ => Ok(None),
    }
}
