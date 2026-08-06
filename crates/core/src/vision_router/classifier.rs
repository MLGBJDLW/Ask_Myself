use crate::error::CoreError;

use super::types::{
    VisionIntent, VisionMode, VisionRouteDecision, VisionRoutePlan, VisionRouterPolicy,
    VisionTurnOverride,
};

pub struct VisionClassificationInput<'a> {
    pub original_name: &'a str,
    pub mime_type: &'a str,
    pub user_prompt: &'a str,
    pub policy: &'a VisionRouterPolicy,
    pub turn_override: Option<VisionTurnOverride>,
    pub primary_supports_vision: bool,
    pub primary_is_local: bool,
    pub auxiliary_available: bool,
    pub auxiliary_is_local: bool,
    pub ocr_available: bool,
}

pub fn classify_vision_route(
    input: VisionClassificationInput<'_>,
) -> Result<VisionRouteDecision, CoreError> {
    if !input.mime_type.starts_with("image/") {
        return Err(CoreError::InvalidInput(
            "unsupported_media_type: Vision Router accepts image attachments only".to_string(),
        ));
    }

    let explicit = input.turn_override;
    if input.policy.mode == VisionMode::Ask && explicit.is_none() {
        return Err(CoreError::InvalidInput(
            "decision_required: choose Auto, OCR only, or Vision only for the current attachments"
                .to_string(),
        ));
    }
    if input.policy.mode == VisionMode::Off && explicit.is_none() {
        return Ok(decision(
            VisionIntent::Unknown,
            VisionRoutePlan::MetadataOnly,
            1.0,
            &["vision_disabled"],
        ));
    }

    let auxiliary_available =
        input.auxiliary_available && (!input.policy.local_only || input.auxiliary_is_local);
    let native_available =
        input.primary_supports_vision && (!input.policy.local_only || input.primary_is_local);

    if let Some(turn_override) = explicit {
        return match turn_override {
            VisionTurnOverride::OcrOnly if input.ocr_available => Ok(decision(
                VisionIntent::DenseText,
                VisionRoutePlan::OcrOnly,
                1.0,
                &["explicit_ocr_only"],
            )),
            VisionTurnOverride::OcrOnly => Err(CoreError::InvalidInput(
                "ocr_unavailable: OCR-only processing was selected".to_string(),
            )),
            VisionTurnOverride::VisionOnly if auxiliary_available => Ok(decision(
                VisionIntent::VisualReasoning,
                VisionRoutePlan::VisionOnly,
                1.0,
                &["explicit_vision_only", "auxiliary_vision_selected"],
            )),
            VisionTurnOverride::VisionOnly if native_available => Ok(decision(
                VisionIntent::VisualReasoning,
                VisionRoutePlan::NativeDirect,
                1.0,
                &["explicit_vision_only", "native_vision_selected"],
            )),
            VisionTurnOverride::VisionOnly => Err(CoreError::InvalidInput(
                if input.policy.local_only {
                    "local_only_route_unavailable: no local vision target is available"
                } else {
                    "vision_binding_missing: no eligible vision target is available"
                }
                .to_string(),
            )),
            VisionTurnOverride::Auto => classify_auto(
                &input,
                native_available,
                auxiliary_available,
                &["explicit_auto"],
            ),
        };
    }

    if input.policy.mode == VisionMode::AlwaysAuxiliary {
        if auxiliary_available {
            return Ok(decision(
                VisionIntent::VisualReasoning,
                VisionRoutePlan::VisionOnly,
                1.0,
                &["always_auxiliary", "auxiliary_vision_selected"],
            ));
        }
        return Err(CoreError::InvalidInput(
            if input.policy.local_only {
                "local_only_route_unavailable: no local auxiliary vision target is available"
            } else {
                "vision_binding_missing: Always auxiliary requires an eligible vision binding"
            }
            .to_string(),
        ));
    }

    classify_auto(&input, native_available, auxiliary_available, &[])
}

fn classify_auto(
    input: &VisionClassificationInput<'_>,
    native_available: bool,
    auxiliary_available: bool,
    prefix_reasons: &[&str],
) -> Result<VisionRouteDecision, CoreError> {
    let (intent, confidence, mut reason_codes) = classify_intent(
        input.original_name,
        input.user_prompt,
        input.policy.prefer_local_processing,
    );
    reason_codes.splice(
        0..0,
        prefix_reasons.iter().map(|reason| (*reason).to_string()),
    );

    let plan = match intent {
        VisionIntent::DenseText if input.ocr_available && auxiliary_available => {
            reason_codes.push("ocr_first_with_vision_supplement".to_string());
            VisionRoutePlan::OcrThenVision
        }
        VisionIntent::DenseText if input.ocr_available => {
            reason_codes.push("ocr_first".to_string());
            VisionRoutePlan::OcrOnly
        }
        VisionIntent::DenseText if auxiliary_available => {
            reason_codes.push("ocr_unavailable_vision_selected".to_string());
            VisionRoutePlan::VisionOnly
        }
        VisionIntent::DenseText if native_available => {
            reason_codes.push("ocr_unavailable_native_vision_selected".to_string());
            VisionRoutePlan::NativeDirect
        }
        VisionIntent::VisualReasoning if native_available => {
            reason_codes.push("native_vision_selected".to_string());
            VisionRoutePlan::NativeDirect
        }
        VisionIntent::VisualReasoning if auxiliary_available => {
            reason_codes.push("auxiliary_vision_selected".to_string());
            VisionRoutePlan::VisionOnly
        }
        VisionIntent::VisualReasoning if input.ocr_available => {
            reason_codes.push("vision_unavailable_ocr_fallback".to_string());
            VisionRoutePlan::OcrOnly
        }
        VisionIntent::Mixed if native_available => {
            reason_codes.push("native_vision_selected_for_mixed_input".to_string());
            if input.ocr_available {
                reason_codes.push("local_ocr_supplement_requested".to_string());
            }
            VisionRoutePlan::NativeDirect
        }
        VisionIntent::Mixed if input.ocr_available && auxiliary_available => {
            reason_codes.push("mixed_ocr_and_vision".to_string());
            VisionRoutePlan::OcrThenVision
        }
        VisionIntent::Mixed if auxiliary_available => {
            reason_codes.push("mixed_vision_without_ocr".to_string());
            VisionRoutePlan::VisionOnly
        }
        VisionIntent::Mixed if input.ocr_available => {
            reason_codes.push("mixed_ocr_without_vision".to_string());
            VisionRoutePlan::OcrOnly
        }
        VisionIntent::Unknown
            if input.ocr_available
                && auxiliary_available
                && input.policy.prefer_local_processing =>
        {
            reason_codes.push("uncertain_local_ocr_probe".to_string());
            VisionRoutePlan::OcrThenVision
        }
        VisionIntent::Unknown if native_available => {
            reason_codes.push("uncertain_native_vision".to_string());
            VisionRoutePlan::NativeDirect
        }
        VisionIntent::Unknown if auxiliary_available => {
            reason_codes.push("uncertain_auxiliary_vision".to_string());
            VisionRoutePlan::VisionOnly
        }
        VisionIntent::Unknown if input.ocr_available => {
            reason_codes.push("uncertain_ocr_only".to_string());
            VisionRoutePlan::OcrOnly
        }
        _ => {
            reason_codes.push(if input.policy.local_only {
                "local_only_route_unavailable".to_string()
            } else {
                "no_image_processor_available".to_string()
            });
            VisionRoutePlan::MetadataOnly
        }
    };

    Ok(VisionRouteDecision {
        intent,
        plan,
        classification_confidence: confidence,
        reason_codes,
    })
}

fn classify_intent(
    original_name: &str,
    user_prompt: &str,
    prefer_local: bool,
) -> (VisionIntent, f32, Vec<String>) {
    let name = original_name.to_ascii_lowercase();
    let prompt = user_prompt.to_ascii_lowercase();
    let dense_name = contains_any(
        &name,
        &[
            "scan",
            "receipt",
            "invoice",
            "document",
            "screenshot",
            "form",
            "bill",
        ],
    );
    let visual_name = contains_any(
        &name,
        &[
            "photo",
            "chart",
            "diagram",
            "graph",
            "plot",
            "ui",
            "wireframe",
            "map",
        ],
    );
    let dense_prompt = contains_any(
        &prompt,
        &[
            "ocr",
            "transcribe",
            "exact text",
            "read the text",
            "extract text",
            "receipt",
            "invoice",
            "scan",
            "screenshot",
            "逐字",
            "识别文字",
            "提取文字",
            "发票",
            "收据",
            "扫描",
            "截图文字",
        ],
    );
    let visual_prompt = contains_any(
        &prompt,
        &[
            "describe",
            "what is in",
            "chart",
            "diagram",
            "spatial",
            "layout",
            "compare",
            "ui",
            "visual",
            "photo",
            "image",
            "图片内容",
            "描述图片",
            "图表",
            "布局",
            "空间",
            "界面",
            "比较",
        ],
    );
    let dense = u8::from(dense_name) + u8::from(dense_prompt);
    let visual = u8::from(visual_name) + u8::from(visual_prompt);
    let mut reasons = Vec::new();
    if dense_name {
        reasons.push("dense_text_filename_hint".to_string());
    }
    if dense_prompt {
        reasons.push("exact_text_prompt_hint".to_string());
    }
    if visual_name {
        reasons.push("visual_filename_hint".to_string());
    }
    if visual_prompt {
        reasons.push("visual_reasoning_prompt_hint".to_string());
    }
    if dense > 0 && visual > 0 {
        (VisionIntent::Mixed, 0.75, reasons)
    } else if dense > 0 {
        (
            VisionIntent::DenseText,
            if dense > 1 { 0.9 } else { 0.78 },
            reasons,
        )
    } else if visual > 0 {
        (
            VisionIntent::VisualReasoning,
            if visual > 1 { 0.9 } else { 0.78 },
            reasons,
        )
    } else {
        if prefer_local {
            reasons.push("prefer_local_processing".to_string());
        }
        (VisionIntent::Unknown, 0.35, reasons)
    }
}

fn contains_any(value: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| value.contains(term))
}

fn decision(
    intent: VisionIntent,
    plan: VisionRoutePlan,
    confidence: f32,
    reasons: &[&str],
) -> VisionRouteDecision {
    VisionRouteDecision {
        intent,
        plan,
        classification_confidence: confidence,
        reason_codes: reasons.iter().map(|reason| (*reason).to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(policy: &'a VisionRouterPolicy) -> VisionClassificationInput<'a> {
        VisionClassificationInput {
            original_name: "image.png",
            mime_type: "image/png",
            user_prompt: "What is shown?",
            policy,
            turn_override: None,
            primary_supports_vision: false,
            primary_is_local: false,
            auxiliary_available: true,
            auxiliary_is_local: false,
            ocr_available: true,
        }
    }

    #[test]
    fn dense_text_uses_ocr_before_vision() {
        let policy = VisionRouterPolicy::default();
        let mut request = input(&policy);
        request.original_name = "receipt-scan.png";
        request.user_prompt = "Extract the exact text";
        let decision = classify_vision_route(request).unwrap();
        assert_eq!(decision.intent, VisionIntent::DenseText);
        assert_eq!(decision.plan, VisionRoutePlan::OcrThenVision);
    }

    #[test]
    fn native_model_avoids_auxiliary_call_for_visual_request() {
        let policy = VisionRouterPolicy::default();
        let mut request = input(&policy);
        request.original_name = "diagram.png";
        request.primary_supports_vision = true;
        let decision = classify_vision_route(request).unwrap();
        assert_eq!(decision.plan, VisionRoutePlan::NativeDirect);
    }

    #[test]
    fn ask_requires_a_turn_choice_before_work() {
        let policy = VisionRouterPolicy {
            mode: VisionMode::Ask,
            ..VisionRouterPolicy::default()
        };
        let error = classify_vision_route(input(&policy))
            .unwrap_err()
            .to_string();
        assert!(error.contains("decision_required"));
    }

    #[test]
    fn local_only_never_selects_remote_vision() {
        let policy = VisionRouterPolicy {
            local_only: true,
            ..VisionRouterPolicy::default()
        };
        let mut request = input(&policy);
        request.original_name = "photo.png";
        request.user_prompt = "Describe the photo";
        let decision = classify_vision_route(request).unwrap();
        assert_eq!(decision.plan, VisionRoutePlan::OcrOnly);
        assert!(decision
            .reason_codes
            .contains(&"vision_unavailable_ocr_fallback".to_string()));
    }

    #[test]
    fn explicit_vision_fails_closed_without_an_allowed_target() {
        let policy = VisionRouterPolicy {
            local_only: true,
            ..VisionRouterPolicy::default()
        };
        let mut request = input(&policy);
        request.turn_override = Some(VisionTurnOverride::VisionOnly);
        let error = classify_vision_route(request).unwrap_err().to_string();
        assert!(error.contains("local_only_route_unavailable"));
    }
}
