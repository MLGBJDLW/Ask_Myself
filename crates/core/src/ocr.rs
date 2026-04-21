//! OCR module — ONNX-based PaddleOCR for text extraction from images.
//!
//! Provides a lazy-initialized, thread-safe OCR engine using PP-OCRv4
//! ONNX models (detection + optional classification + recognition).
//! Falls back to LLM Vision API when confidence is below threshold.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ndarray::Array4;

use crate::error::CoreError;

// ── Configuration ───────────────────────────────────────────────────

/// User-configurable OCR settings, persisted in the database.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrConfig {
    /// Master toggle — set `false` to skip OCR entirely (images become
    /// metadata-only stubs like today).
    pub enabled: bool,

    /// Minimum average CTC confidence (0.0–1.0) to accept OCR output.
    /// Below this threshold the LLM Vision fallback is attempted.
    pub confidence_threshold: f32,

    /// Whether to attempt LLM Vision when OCR confidence is low or errors.
    pub llm_fallback_enabled: bool,

    /// Maximum image dimension (longest side) sent to the detection model.
    /// Larger = more accurate but slower.  Default 960.
    pub det_limit_side_len: u32,

    /// Enable the orientation classifier (cls model).
    /// Disable to save ~10 ms per box when text is known to be upright.
    pub use_cls: bool,

    /// Optional override path for OCR model files.
    /// When empty, defaults to `<data_dir>/<APP_DIR>/models/paddleocr/`.
    pub model_path: String,

    /// ISO 639-1 language codes controlling which rec model + dictionary
    /// to load.  Default: `["en", "zh"]`.
    pub languages: Vec<String>,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            confidence_threshold: 0.6,
            llm_fallback_enabled: true,
            det_limit_side_len: 960,
            use_cls: true,
            model_path: String::new(),
            languages: vec!["en".into(), "zh".into()],
        }
    }
}

// ── OCR Result ──────────────────────────────────────────────────────

/// A single recognized text region with its bounding box and confidence.
#[derive(Debug, Clone)]
pub struct OcrTextRegion {
    /// Recognized text content.
    pub text: String,
    /// Average CTC confidence for this region (0.0–1.0).
    pub confidence: f32,
    /// Bounding box: `[top_left_x, top_left_y, width, height]`.
    pub bbox: [f32; 4],
}

/// Result of running OCR on a single image.
#[derive(Debug, Clone)]
pub struct OcrResult {
    /// All recognized text regions, sorted reading-order (top→bottom, left→right).
    pub regions: Vec<OcrTextRegion>,
    /// Combined full text (regions joined with newlines).
    pub full_text: String,
    /// Average confidence across all regions.
    pub avg_confidence: f32,
    /// Whether the result came from OCR or LLM fallback.
    pub source: OcrSource,
}

/// How the text was extracted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OcrSource {
    PaddleOcr,
    LlmVision,
    None,
}

/// Download progress for OCR models.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrDownloadProgress {
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub file_index: usize,
    pub total_files: usize,
}

// ── Engine (lazy, thread-safe) ──────────────────────────────────────

/// Thread-safe, lazily-initialized PaddleOCR engine.
///
/// Uses `OnceLock` so models are loaded exactly once on first call.
/// The inner `ort::Session` objects are wrapped in `Mutex` so multiple
/// threads can share the engine without data races (ONNX Runtime itself
/// is thread-safe for inference, but session mutation isn't).
pub struct OcrEngine {
    det_session: Mutex<ort::session::Session>,
    cls_session: Option<Mutex<ort::session::Session>>,
    rec_session: Mutex<ort::session::Session>,
    dictionary: Vec<String>,
    config: OcrConfig,
}

/// Global singleton — initialised on first `ocr_engine()` call.
static OCR_ENGINE: OnceLock<Result<Arc<OcrEngine>, String>> = OnceLock::new();

/// Get or initialise the global OCR engine.
///
/// Returns `Err` with a human-readable message if model files are missing
/// or ONNX sessions fail to build.
pub fn ocr_engine(config: &OcrConfig) -> Result<Arc<OcrEngine>, CoreError> {
    let result = OCR_ENGINE.get_or_init(|| {
        OcrEngine::new(config)
            .map(Arc::new)
            .map_err(|e| e.to_string())
    });

    match result {
        Ok(engine) => Ok(Arc::clone(engine)),
        Err(msg) => Err(CoreError::Ocr(format!("OCR engine init failed: {msg}"))),
    }
}

impl OcrEngine {
    /// Build a new engine, loading all three ONNX models + the character
    /// dictionary from disk.
    fn new(config: &OcrConfig) -> Result<Self, CoreError> {
        let model_dir = ocr_model_dir(config)?;

        let det_path = model_dir.join("pp-ocrv4-det.onnx");
        let cls_path = model_dir.join("pp-ocrv4-cls.onnx");
        let rec_path = model_dir.join("pp-ocrv4-rec.onnx");
        let dict_path = model_dir.join("ppocr_keys_v1.txt");

        if !det_path.exists() || !rec_path.exists() || !dict_path.exists() {
            return Err(CoreError::Ocr(format!(
                "PaddleOCR model files not found in {}. \
                 Download them from Settings → OCR Models.",
                model_dir.display()
            )));
        }

        let num_threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(1))
            .unwrap_or(1);

        let det_session = load_onnx_session(&det_path, num_threads)?;
        let rec_session = load_onnx_session(&rec_path, num_threads)?;

        let cls_session = if config.use_cls && cls_path.exists() {
            Some(Mutex::new(load_onnx_session(&cls_path, num_threads)?))
        } else {
            None
        };

        let dictionary: Vec<String> = std::fs::read_to_string(&dict_path)
            .map_err(|e| CoreError::Ocr(format!("read dictionary: {e}")))?
            .lines()
            .map(|l| l.to_string())
            .collect();

        tracing::info!(
            "PaddleOCR engine loaded (det={}, cls={}, rec={}, dict={} chars)",
            det_path.display(),
            if config.use_cls { "yes" } else { "no" },
            rec_path.display(),
            dictionary.len(),
        );

        Ok(Self {
            det_session: Mutex::new(det_session),
            cls_session,
            rec_session: Mutex::new(rec_session),
            dictionary,
            config: config.clone(),
        })
    }
}

/// Load an ONNX session from a model file.
fn load_onnx_session(path: &Path, num_threads: usize) -> Result<ort::session::Session, CoreError> {
    ort::session::Session::builder()
        .map_err(|e| CoreError::Ocr(format!("ort session builder: {e}")))?
        .with_intra_threads(num_threads)
        .map_err(|e| CoreError::Ocr(format!("ort intra threads: {e}")))?
        .commit_from_file(path)
        .map_err(|e| CoreError::Ocr(format!("ort load model {}: {e}", path.display())))
}

/// Resolve the model directory: user override or default data dir.
fn ocr_model_dir(config: &OcrConfig) -> Result<PathBuf, CoreError> {
    if !config.model_path.is_empty() {
        return Ok(PathBuf::from(&config.model_path));
    }
    let data_dir =
        dirs::data_dir().ok_or_else(|| CoreError::Ocr("cannot determine data directory".into()))?;
    Ok(data_dir
        .join(crate::APP_DIR)
        .join("models")
        .join("paddleocr"))
}

// ── ImageNet normalisation constants ────────────────────────────────

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

// ── Internal types ──────────────────────────────────────────────────

/// Pre-processed detection input ready for ONNX inference.
struct DetInput {
    tensor: Array4<f32>,
    scale_x: f32,
    scale_y: f32,
}

/// A detected text bounding box (4 corner points).
#[derive(Debug, Clone)]
struct TextBox {
    /// Four corner points: `[top-left, top-right, bottom-right, bottom-left]`.
    points: [[f32; 2]; 4],
    /// Whether the box has been classified as rotated 180°.
    is_rotated: bool,
}

// ── OCR Pipeline ────────────────────────────────────────────────────

impl OcrEngine {
    /// Run OCR on raw image bytes.
    ///
    /// Returns an `OcrResult` with all detected text regions.
    /// This is the primary entry point called by `parse_image()`.
    pub fn recognize_image(&self, image_bytes: &[u8]) -> Result<OcrResult, CoreError> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| CoreError::Ocr(format!("decode image for OCR: {e}")))?;

        let rgb = img.to_rgb8();
        let (orig_w, orig_h) = (rgb.width(), rgb.height());

        // ── Step 1: Detection ──
        let det_input = self.preprocess_det(&rgb);
        let boxes = self.run_det(&det_input, orig_w, orig_h)?;

        if boxes.is_empty() {
            return Ok(OcrResult {
                regions: vec![],
                full_text: String::new(),
                avg_confidence: 0.0,
                source: OcrSource::PaddleOcr,
            });
        }

        // ── Step 2: Classification (optional) ──
        let oriented_boxes = if self.cls_session.is_some() {
            self.run_cls(&rgb, &boxes)?
        } else {
            boxes
        };

        // ── Step 3: Recognition ──
        let regions = self.run_rec(&rgb, &oriented_boxes)?;

        let full_text = regions
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let avg_confidence = if regions.is_empty() {
            0.0
        } else {
            regions.iter().map(|r| r.confidence).sum::<f32>() / regions.len() as f32
        };

        Ok(OcrResult {
            regions,
            full_text,
            avg_confidence,
            source: OcrSource::PaddleOcr,
        })
    }

    /// Pre-process image for the detection model.
    ///
    /// Resize longest side to `det_limit_side_len`, pad to multiple of 32,
    /// normalize with ImageNet mean/std.
    fn preprocess_det(&self, rgb: &image::RgbImage) -> DetInput {
        let (w, h) = (rgb.width(), rgb.height());
        let limit = self.config.det_limit_side_len as f32;

        // Scale so the longest side equals det_limit_side_len.
        let scale = if w > h {
            limit / w as f32
        } else {
            limit / h as f32
        }
        .min(1.0); // Never upscale.

        let new_w = (w as f32 * scale).round() as u32;
        let new_h = (h as f32 * scale).round() as u32;

        // Pad to next multiple of 32.
        let pad_w = (new_w.div_ceil(32)) * 32;
        let pad_h = (new_h.div_ceil(32)) * 32;

        let resized =
            image::imageops::resize(rgb, new_w, new_h, image::imageops::FilterType::Triangle);

        let scale_x = w as f32 / new_w as f32;
        let scale_y = h as f32 / new_h as f32;

        // Build [1, 3, pad_h, pad_w] tensor, normalized.
        let mut tensor = Array4::<f32>::zeros((1, 3, pad_h as usize, pad_w as usize));
        for y in 0..new_h {
            for x in 0..new_w {
                let pixel = resized.get_pixel(x, y);
                for c in 0..3 {
                    let val = pixel[c] as f32 / 255.0;
                    tensor[[0, c, y as usize, x as usize]] =
                        (val - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
                }
            }
        }

        DetInput {
            tensor,
            scale_x,
            scale_y,
        }
    }

    /// Run detection model → list of oriented bounding boxes.
    ///
    /// Post-processes the probability map: threshold at 0.3, find connected
    /// components, extract bounding boxes, expand by `unclip_ratio`, sort
    /// reading-order.
    fn run_det(
        &self,
        input: &DetInput,
        orig_w: u32,
        orig_h: u32,
    ) -> Result<Vec<TextBox>, CoreError> {
        let mut session = self
            .det_session
            .lock()
            .map_err(|_| CoreError::Ocr("det session lock poisoned".into()))?;

        let shape = input.tensor.shape().to_vec();

        let input_tensor = ort::value::Tensor::from_array(input.tensor.clone())
            .map_err(|e| CoreError::Ocr(format!("det tensor creation: {e}")))?;

        let outputs = session
            .run(ort::inputs![input_tensor])
            .map_err(|e| CoreError::Ocr(format!("det inference: {e}")))?;

        let output_array = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Ocr(format!("det output extract: {e}")))?;

        let det_h = shape[2];
        let det_w = shape[3];

        // Threshold probability map at 0.3 → binary mask.
        let mut binary = vec![0u8; det_h * det_w];
        for y in 0..det_h {
            for x in 0..det_w {
                if output_array[[0, 0, y, x]] > 0.3 {
                    binary[y * det_w + x] = 255;
                }
            }
        }

        // Find connected components and extract bounding boxes.
        let gray_image = image::GrayImage::from_raw(det_w as u32, det_h as u32, binary)
            .ok_or_else(|| CoreError::Ocr("failed to create binary mask image".into()))?;

        let components = imageproc::region_labelling::connected_components(
            &gray_image,
            imageproc::region_labelling::Connectivity::Eight,
            image::Luma([0u8]),
        );

        // Collect bounding boxes per label.
        let mut label_bounds: std::collections::HashMap<u32, (u32, u32, u32, u32)> =
            std::collections::HashMap::new();

        for y in 0..det_h as u32 {
            for x in 0..det_w as u32 {
                let label = components.get_pixel(x, y).0[0];
                if label == 0 {
                    continue;
                }
                let entry = label_bounds.entry(label).or_insert((x, y, x, y));
                entry.0 = entry.0.min(x);
                entry.1 = entry.1.min(y);
                entry.2 = entry.2.max(x);
                entry.3 = entry.3.max(y);
            }
        }

        let unclip_ratio: f32 = 1.5;
        let mut boxes: Vec<TextBox> = Vec::new();

        for (min_x, min_y, max_x, max_y) in label_bounds.values() {
            let bw = (max_x - min_x) as f32;
            let bh = (max_y - min_y) as f32;

            // Filter out tiny regions.
            if bw < 3.0 || bh < 3.0 {
                continue;
            }

            // Expand by unclip_ratio.
            let cx = (*min_x as f32 + *max_x as f32) / 2.0;
            let cy = (*min_y as f32 + *max_y as f32) / 2.0;
            let half_w = bw / 2.0 * unclip_ratio;
            let half_h = bh / 2.0 * unclip_ratio;

            // Scale back to original image coordinates.
            let x0 = ((cx - half_w) * input.scale_x).max(0.0).min(orig_w as f32);
            let y0 = ((cy - half_h) * input.scale_y).max(0.0).min(orig_h as f32);
            let x1 = ((cx + half_w) * input.scale_x).max(0.0).min(orig_w as f32);
            let y1 = ((cy + half_h) * input.scale_y).max(0.0).min(orig_h as f32);

            boxes.push(TextBox {
                points: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                is_rotated: false,
            });
        }

        // Sort top-to-bottom, then left-to-right.
        boxes.sort_by(|a, b| {
            let ay = a.points[0][1];
            let by = b.points[0][1];
            let y_cmp = ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal);
            if y_cmp != std::cmp::Ordering::Equal {
                return y_cmp;
            }
            let ax = a.points[0][0];
            let bx = b.points[0][0];
            ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(boxes)
    }

    /// Run classifier on cropped text regions to detect 180° rotation.
    ///
    /// For each box: crop, resize to `[48, 192]`, batch inference,
    /// flip box orientation if P(180°) > 0.9.
    fn run_cls(&self, rgb: &image::RgbImage, boxes: &[TextBox]) -> Result<Vec<TextBox>, CoreError> {
        let mut session_guard = self
            .cls_session
            .as_ref()
            .unwrap()
            .lock()
            .map_err(|_| CoreError::Ocr("cls session lock poisoned".into()))?;

        let n = boxes.len();
        let cls_h: u32 = 48;
        let cls_w: u32 = 192;

        // Build batch tensor [N, 3, 48, 192].
        let mut tensor = Array4::<f32>::zeros((n, 3, cls_h as usize, cls_w as usize));

        for (i, b) in boxes.iter().enumerate() {
            let crop = crop_and_resize_box(rgb, b, cls_h, cls_w);
            for y in 0..cls_h {
                for x in 0..cls_w {
                    let pixel = crop.get_pixel(x, y);
                    for c in 0..3 {
                        let val = pixel[c] as f32 / 255.0;
                        tensor[[i, c, y as usize, x as usize]] =
                            (val - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
                    }
                }
            }
        }

        let input_tensor = ort::value::Tensor::from_array(tensor)
            .map_err(|e| CoreError::Ocr(format!("cls tensor creation: {e}")))?;

        let outputs = session_guard
            .run(ort::inputs![input_tensor])
            .map_err(|e| CoreError::Ocr(format!("cls inference: {e}")))?;

        let output_array = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Ocr(format!("cls output extract: {e}")))?;

        let mut result = boxes.to_vec();
        for i in 0..n {
            let prob_180 = output_array[[i, 1]];
            if prob_180 > 0.9 {
                result[i].is_rotated = true;
            }
        }

        Ok(result)
    }

    /// Run recognition on cropped text regions → `OcrTextRegion` list.
    ///
    /// For each box: crop, resize to height 48 (proportional width, max 320),
    /// normalize, run rec session, CTC greedy decode against the dictionary.
    fn run_rec(
        &self,
        rgb: &image::RgbImage,
        boxes: &[TextBox],
    ) -> Result<Vec<OcrTextRegion>, CoreError> {
        let mut session_guard = self
            .rec_session
            .lock()
            .map_err(|_| CoreError::Ocr("rec session lock poisoned".into()))?;

        let rec_h: u32 = 48;
        let max_rec_w: u32 = 320;
        let mut regions = Vec::with_capacity(boxes.len());

        // Process each box individually (variable width).
        for b in boxes {
            let crop = crop_box(rgb, b);

            // Resize to height 48, proportional width (max 320).
            let aspect = crop.width() as f32 / crop.height() as f32;
            let target_w = ((rec_h as f32 * aspect).round() as u32)
                .min(max_rec_w)
                .max(1);

            let resized = image::imageops::resize(
                &crop,
                target_w,
                rec_h,
                image::imageops::FilterType::Triangle,
            );

            // Build [1, 3, 48, W] tensor.
            let mut tensor = Array4::<f32>::zeros((1, 3, rec_h as usize, target_w as usize));
            for y in 0..rec_h {
                for x in 0..target_w {
                    let pixel = resized.get_pixel(x, y);
                    for c in 0..3 {
                        let val = pixel[c] as f32 / 255.0;
                        tensor[[0, c, y as usize, x as usize]] =
                            (val - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
                    }
                }
            }

            let input_tensor = ort::value::Tensor::from_array(tensor)
                .map_err(|e| CoreError::Ocr(format!("rec tensor creation: {e}")))?;

            let outputs = session_guard
                .run(ort::inputs![input_tensor])
                .map_err(|e| CoreError::Ocr(format!("rec inference: {e}")))?;

            let output_array = outputs[0]
                .try_extract_array::<f32>()
                .map_err(|e| CoreError::Ocr(format!("rec output extract: {e}")))?;

            // output shape: [1, W/4, num_classes]
            let steps = output_array.shape()[1];
            let num_classes = output_array.shape()[2];

            let logits: Vec<Vec<f32>> = (0..steps)
                .map(|t| (0..num_classes).map(|c| output_array[[0, t, c]]).collect())
                .collect();

            let (text, confidence) = ctc_greedy_decode(&logits, &self.dictionary);

            if !text.is_empty() {
                let x0 = b.points[0][0];
                let y0 = b.points[0][1];
                let bw = b.points[1][0] - b.points[0][0];
                let bh = b.points[2][1] - b.points[0][1];

                regions.push(OcrTextRegion {
                    text,
                    confidence,
                    bbox: [x0, y0, bw, bh],
                });
            }
        }

        Ok(regions)
    }
}

// ── CTC Decode ──────────────────────────────────────────────────────

/// CTC greedy decode: argmax per step, collapse repeats, strip blanks.
///
/// Index 0 is the blank token. Dictionary indices are offset by 1
/// (dictionary\[0\] corresponds to logit index 1).
fn ctc_greedy_decode(logits: &[Vec<f32>], dictionary: &[String]) -> (String, f32) {
    let mut result = String::new();
    let mut confidences = Vec::new();
    let mut prev_idx: usize = 0; // 0 = blank

    for step in logits {
        let (max_idx, &max_val) = step
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();

        if max_idx != 0 && max_idx != prev_idx {
            if let Some(ch) = dictionary.get(max_idx - 1) {
                result.push_str(ch);
                confidences.push(max_val);
            }
        }
        prev_idx = max_idx;
    }

    let avg_confidence = if confidences.is_empty() {
        0.0
    } else {
        confidences.iter().sum::<f32>() / confidences.len() as f32
    };

    (result, avg_confidence)
}

// ── Image crop helpers ──────────────────────────────────────────────

/// Crop the bounding box region from the source image.
fn crop_box(rgb: &image::RgbImage, b: &TextBox) -> image::RgbImage {
    let x0 = b.points[0][0].max(0.0) as u32;
    let y0 = b.points[0][1].max(0.0) as u32;
    let x1 = (b.points[1][0] as u32).min(rgb.width());
    let y1 = (b.points[2][1] as u32).min(rgb.height());

    let w = x1.saturating_sub(x0).max(1);
    let h = y1.saturating_sub(y0).max(1);

    image::imageops::crop_imm(rgb, x0, y0, w, h).to_image()
}

/// Crop a box region and resize to a fixed target size.
fn crop_and_resize_box(
    rgb: &image::RgbImage,
    b: &TextBox,
    target_h: u32,
    target_w: u32,
) -> image::RgbImage {
    let cropped = crop_box(rgb, b);
    image::imageops::resize(
        &cropped,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    )
}

// ── High-level API ──────────────────────────────────────────────────

/// High-level: extract text from image bytes using OCR with LLM fallback.
///
/// This is the function that `parse_image()` and `parse_pdf()` call.
/// It is synchronous for OCR but requires a tokio runtime for the LLM
/// fallback path (same as the rest of the crate).
pub fn extract_text_from_image(
    image_bytes: &[u8],
    mime_type: &str,
    config: &OcrConfig,
    llm_provider: Option<&dyn crate::llm::LlmProvider>,
) -> Result<OcrResult, CoreError> {
    if !config.enabled {
        return Ok(OcrResult {
            regions: vec![],
            full_text: String::new(),
            avg_confidence: 0.0,
            source: OcrSource::None,
        });
    }

    // ── Try ONNX OCR first ──
    let engine = ocr_engine(config)?;
    let ocr_result = engine.recognize_image(image_bytes);

    match ocr_result {
        Ok(result)
            if result.avg_confidence >= config.confidence_threshold
                && !result.full_text.is_empty() =>
        {
            tracing::debug!(
                "OCR succeeded: {} regions, confidence={:.2}",
                result.regions.len(),
                result.avg_confidence
            );
            Ok(result)
        }
        Ok(low_conf_result) => {
            tracing::info!(
                "OCR confidence {:.2} below threshold {:.2}, attempting LLM fallback",
                low_conf_result.avg_confidence,
                config.confidence_threshold
            );

            if config.llm_fallback_enabled {
                if let Some(provider) = llm_provider {
                    match tokio::runtime::Handle::try_current() {
                        Ok(handle) => {
                            match handle.block_on(extract_text_via_llm_vision(
                                image_bytes,
                                mime_type,
                                provider,
                            )) {
                                Ok(llm_result) => return Ok(llm_result),
                                Err(e) => {
                                    tracing::warn!("LLM vision fallback failed: {e}");
                                }
                            }
                        }
                        Err(_) => {
                            tracing::warn!("No tokio runtime available for LLM vision fallback");
                        }
                    }
                }
            }
            // Return low-confidence OCR result as best-effort.
            Ok(low_conf_result)
        }
        Err(e) => {
            tracing::warn!("OCR failed: {e}");

            if config.llm_fallback_enabled {
                if let Some(provider) = llm_provider {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        return handle.block_on(extract_text_via_llm_vision(
                            image_bytes,
                            mime_type,
                            provider,
                        ));
                    }
                }
            }

            Err(e)
        }
    }
}

// ── LLM Vision Fallback ─────────────────────────────────────────────

/// Attempt LLM Vision API to extract text from an image.
///
/// Called when OCR avg_confidence is below threshold, OCR errors, or
/// OCR returns empty text on a non-trivially-sized image.
pub async fn extract_text_via_llm_vision(
    image_bytes: &[u8],
    mime_type: &str,
    provider: &dyn crate::llm::LlmProvider,
) -> Result<OcrResult, CoreError> {
    use crate::llm::{CompletionRequest, ContentPart, Message, Role};
    use crate::media::prepare_image_for_llm;

    let (b64, media) = prepare_image_for_llm(image_bytes, mime_type)?;

    let messages = vec![Message {
        role: Role::User,
        parts: vec![
            ContentPart::Text {
                text: "Extract ALL text from this image. \
                       Return only the extracted text, preserving layout. \
                       If no text is visible, respond with exactly: [NO TEXT]"
                    .into(),
            },
            ContentPart::Image {
                media_type: media,
                data: b64,
            },
        ],
        name: None,
        tool_calls: None,
        reasoning_content: None,
    }];

    let request = CompletionRequest {
        model: String::new(),
        messages,
        temperature: Some(0.0),
        max_tokens: Some(4096),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
        parallel_tool_calls: true,
    };

    let response = provider.complete(&request).await?;
    let text = response.content.trim().to_string();

    if text == "[NO TEXT]" || text.is_empty() {
        return Ok(OcrResult {
            regions: vec![],
            full_text: String::new(),
            avg_confidence: 0.0,
            source: OcrSource::LlmVision,
        });
    }

    Ok(OcrResult {
        regions: vec![OcrTextRegion {
            text: text.clone(),
            confidence: 1.0, // LLM doesn't provide per-region confidence.
            bbox: [0.0, 0.0, 0.0, 0.0],
        }],
        full_text: text,
        avg_confidence: 1.0,
        source: OcrSource::LlmVision,
    })
}

// ── PDF OCR ─────────────────────────────────────────────────────────

/// Extract text from a scanned PDF by extracting embedded images and running OCR.
///
/// Scanned PDFs store each page as an embedded image.  This function uses
/// `lopdf` to extract those images and passes them through the OCR pipeline.
pub fn ocr_pdf(
    pdf_bytes: &[u8],
    config: &OcrConfig,
    llm_provider: Option<&dyn crate::llm::LlmProvider>,
) -> Result<String, CoreError> {
    let doc = lopdf::Document::load_mem(pdf_bytes)
        .map_err(|e| CoreError::Parse(format!("PDF load: {e}")))?;

    let pages: Vec<lopdf::ObjectId> = doc.get_pages().into_values().collect();

    let mut all_text = String::new();

    for (page_idx, &page_id) in pages.iter().enumerate() {
        let images = extract_images_from_pdf_page(&doc, page_id);
        if images.is_empty() {
            tracing::debug!("No embedded images found on PDF page {page_idx}");
            continue;
        }

        for img in images {
            // Encode extracted image as PNG bytes for OCR.
            let mut buf = std::io::Cursor::new(Vec::new());
            if let Err(e) = img.write_to(&mut buf, image::ImageFormat::Png) {
                tracing::warn!("Failed to encode PDF page {page_idx} image as PNG: {e}");
                continue;
            }

            match extract_text_from_image(&buf.into_inner(), "image/png", config, llm_provider) {
                Ok(result) if !result.full_text.is_empty() => {
                    if !all_text.is_empty() {
                        all_text.push_str("\n\n--- Page Break ---\n\n");
                    }
                    all_text.push_str(&result.full_text);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("OCR failed for PDF page {page_idx}: {e}");
                }
            }
        }
    }

    Ok(all_text)
}

/// Extract embedded images from a single PDF page using `lopdf`.
///
/// Scanned PDFs typically store each page as a large embedded image
/// (JPEG, JPEG2000, or raw pixel data).  This function finds image
/// XObjects on the given page and returns them as decoded `DynamicImage`s.
fn extract_images_from_pdf_page(
    doc: &lopdf::Document,
    page_id: lopdf::ObjectId,
) -> Vec<image::DynamicImage> {
    let mut images = Vec::new();

    // Get the page's Resources → XObject dictionary.
    let xobjects = (|| -> Option<&lopdf::Dictionary> {
        let page = doc.get_object(page_id).ok()?.as_dict().ok()?;
        let resources = page
            .get(b"Resources")
            .ok()
            .and_then(|r| doc.dereference(r).ok())
            .and_then(|(_, o)| o.as_dict().ok())?;
        let xobj = resources
            .get(b"XObject")
            .ok()
            .and_then(|x| doc.dereference(x).ok())
            .and_then(|(_, o)| o.as_dict().ok())?;
        Some(xobj)
    })();

    let xobjects = match xobjects {
        Some(x) => x,
        None => return images,
    };

    for (_name, obj_ref) in xobjects.iter() {
        let stream = match doc
            .dereference(obj_ref)
            .ok()
            .and_then(|(_, o)| o.as_stream().ok())
        {
            Some(s) => s,
            None => continue,
        };

        let subtype: &[u8] = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|s| s.as_name().ok())
            .unwrap_or(b"");
        if subtype != b"Image" {
            continue;
        }

        let width = stream
            .dict
            .get(b"Width")
            .ok()
            .and_then(|w| w.as_i64().ok())
            .unwrap_or(0) as u32;
        let height = stream
            .dict
            .get(b"Height")
            .ok()
            .and_then(|h| h.as_i64().ok())
            .unwrap_or(0) as u32;
        if width == 0 || height == 0 {
            continue;
        }

        let filter: Vec<u8> = stream
            .dict
            .get(b"Filter")
            .ok()
            .and_then(|f| {
                f.as_name().ok().map(|n| n.to_vec()).or_else(|| {
                    f.as_array().ok().and_then(|arr| {
                        arr.last()
                            .and_then(|n| n.as_name().ok())
                            .map(|n| n.to_vec())
                    })
                })
            })
            .unwrap_or_default();

        // Try to get the decoded stream content.
        let raw = match stream.decompressed_content() {
            Ok(data) => data,
            Err(_) => stream.content.clone(),
        };

        // DCTDecode = JPEG, JPXDecode = JPEG2000 — both loadable by `image`.
        if filter == b"DCTDecode" || filter == b"JPXDecode" {
            if let Ok(img) = image::load_from_memory(&raw) {
                images.push(img);
            }
        } else {
            // Raw pixel data — try to construct an image from BPC + colorspace info.
            let bpc = stream
                .dict
                .get(b"BitsPerComponent")
                .ok()
                .and_then(|b| b.as_i64().ok())
                .unwrap_or(8) as u32;
            if bpc != 8 {
                continue;
            }

            let cs_name: Vec<u8> = stream
                .dict
                .get(b"ColorSpace")
                .ok()
                .and_then(|c| {
                    c.as_name().ok().map(|n| n.to_vec()).or_else(|| {
                        c.as_array()
                            .ok()
                            .and_then(|a| a.first())
                            .and_then(|n| n.as_name().ok())
                            .map(|n| n.to_vec())
                    })
                })
                .unwrap_or_default();

            let expected_len = (width * height) as usize;
            if cs_name == b"DeviceGray" && raw.len() >= expected_len {
                if let Some(gray) =
                    image::GrayImage::from_raw(width, height, raw[..expected_len].to_vec())
                {
                    images.push(image::DynamicImage::ImageLuma8(gray));
                }
            } else if (cs_name == b"DeviceRGB" || cs_name.is_empty())
                && raw.len() >= expected_len * 3
            {
                if let Some(rgb) =
                    image::RgbImage::from_raw(width, height, raw[..expected_len * 3].to_vec())
                {
                    images.push(image::DynamicImage::ImageRgb8(rgb));
                }
            }
        }
    }

    images
}

// ── Model download ──────────────────────────────────────────────────

/// Check whether PaddleOCR model files exist.
pub fn check_ocr_models_exist(config: &OcrConfig) -> bool {
    let dir = match ocr_model_dir(config) {
        Ok(d) => d,
        Err(_) => return false,
    };
    dir.join("pp-ocrv4-det.onnx").exists()
        && dir.join("pp-ocrv4-rec.onnx").exists()
        && dir.join("ppocr_keys_v1.txt").exists()
}

/// Download PaddleOCR ONNX models from the project's model server.
///
/// Model URLs are placeholders — replace with actual hosting URLs
/// (PaddlePaddle BOS, GitHub Releases, etc.) before production use.
pub fn download_ocr_models(
    config: &OcrConfig,
    on_progress: impl Fn(OcrDownloadProgress),
) -> Result<(), CoreError> {
    let dir = ocr_model_dir(config)?;
    std::fs::create_dir_all(&dir)?;

    let files = [
        (
            "pp-ocrv4-det.onnx",
            "https://paddleocr.bj.bcebos.com/PP-OCRv4/pp-ocrv4-det.onnx",
        ),
        (
            "pp-ocrv4-cls.onnx",
            "https://paddleocr.bj.bcebos.com/PP-OCRv4/pp-ocrv4-cls.onnx",
        ),
        (
            "pp-ocrv4-rec.onnx",
            "https://paddleocr.bj.bcebos.com/PP-OCRv4/pp-ocrv4-rec.onnx",
        ),
        (
            "ppocr_keys_v1.txt",
            "https://paddleocr.bj.bcebos.com/PP-OCRv4/ppocr_keys_v1.txt",
        ),
    ];

    for (idx, (filename, url)) in files.iter().enumerate() {
        let dest = dir.join(filename);
        if dest.exists() {
            tracing::debug!("OCR model already exists: {}", dest.display());
            continue;
        }

        tracing::info!("Downloading OCR model: {filename}");
        let response = reqwest::blocking::get(*url)
            .map_err(|e| CoreError::Ocr(format!("download {filename}: {e}")))?;

        let total_bytes = response.content_length();
        let bytes = response
            .bytes()
            .map_err(|e| CoreError::Ocr(format!("read {filename}: {e}")))?;

        on_progress(OcrDownloadProgress {
            filename: filename.to_string(),
            bytes_downloaded: bytes.len() as u64,
            total_bytes,
            file_index: idx,
            total_files: files.len(),
        });

        std::fs::write(&dest, &bytes)
            .map_err(|e| CoreError::Ocr(format!("write {filename}: {e}")))?;
    }

    Ok(())
}

// ── Database Persistence ────────────────────────────────────────────

use crate::db::Database;
use rusqlite::params;

const OCR_CONFIG_KEY: &str = "ocr_config";

impl Database {
    /// Persist an [`OcrConfig`] to the database.
    pub fn save_ocr_config(&self, config: &OcrConfig) -> Result<(), CoreError> {
        let json = serde_json::to_string(config)?;
        let conn = self.conn();

        // Self-healing: ensure table exists even if migration was recorded
        // but SQL never actually ran.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ocr_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )?;

        conn.execute(
            "INSERT INTO ocr_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![OCR_CONFIG_KEY, &json],
        )?;
        Ok(())
    }

    /// Load the stored [`OcrConfig`], returning `OcrConfig::default()`
    /// if none has been saved yet.
    pub fn load_ocr_config(&self) -> Result<OcrConfig, CoreError> {
        let conn = self.conn();

        // Guard: table might not exist yet if migration hasn't run.
        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ocr_config')",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(OcrConfig::default());
        }

        let result = conn.query_row(
            "SELECT value FROM ocr_config WHERE key = ?1",
            params![OCR_CONFIG_KEY],
            |row| row.get::<_, String>(0),
        );

        match result {
            Ok(json) => {
                let config: OcrConfig = serde_json::from_str(&json)?;
                Ok(config)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(OcrConfig::default()),
            Err(e) => Err(CoreError::Database(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn test_save_and_load_ocr_config() {
        let db = Database::open_memory().expect("open_memory");
        let config = OcrConfig {
            enabled: false,
            confidence_threshold: 0.8,
            llm_fallback_enabled: false,
            det_limit_side_len: 1280,
            use_cls: false,
            model_path: "/custom/path".into(),
            languages: vec!["en".into(), "ja".into()],
        };
        db.save_ocr_config(&config).expect("save");
        let loaded = db.load_ocr_config().expect("load");
        assert!(!loaded.enabled);
        assert!((loaded.confidence_threshold - 0.8).abs() < f32::EPSILON);
        assert_eq!(loaded.languages, vec!["en".to_string(), "ja".to_string()]);
    }

    #[test]
    fn test_load_ocr_config_default() {
        let db = Database::open_memory().expect("open_memory");
        let config = db.load_ocr_config().expect("load default");
        assert!(config.enabled);
    }

    #[test]
    fn test_save_ocr_config_upsert() {
        let db = Database::open_memory().expect("open_memory");
        let mut config = OcrConfig::default();
        db.save_ocr_config(&config).expect("save 1");
        config.enabled = false;
        db.save_ocr_config(&config).expect("save 2");
        let loaded = db.load_ocr_config().expect("load");
        assert!(!loaded.enabled);
    }
}
