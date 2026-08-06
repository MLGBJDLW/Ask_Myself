//! Deterministic image-understanding routing and structured observation boundary.
//!
//! The module owns policy parsing, route classification, provider-output
//! validation, and cache identity. Desktop orchestration supplies concrete OCR
//! and LLM providers; provider-specific image serialization remains in `llm`.

mod classifier;
mod observation;
mod types;

pub use classifier::{classify_vision_route, VisionClassificationInput};
pub use observation::{
    build_ocr_observation, execute_vision_observation, merge_vision_observations,
    observation_prompt_text, parse_vision_model_observation, VisionExecutionInput,
    VisionProviderInput,
};
pub use types::*;
