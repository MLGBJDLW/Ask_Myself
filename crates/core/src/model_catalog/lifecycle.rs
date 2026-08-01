use serde::{Deserialize, Serialize};

use super::{ModelDescriptor, ModelLifecycle};

pub fn select_implicit_default(models: &[ModelDescriptor]) -> Option<&ModelDescriptor> {
    models
        .iter()
        .find(|model| model.recommended && model.is_implicit_default_eligible())
        .or_else(|| {
            models
                .iter()
                .find(|model| model.is_implicit_default_eligible())
        })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedModelSelection {
    pub provider_id: String,
    #[serde(default)]
    pub provider_endpoint_id: Option<String>,
    pub model_id: String,
}

impl SavedModelSelection {
    pub fn new(
        provider_id: impl Into<String>,
        provider_endpoint_id: Option<String>,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_endpoint_id,
            model_id: model_id.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionResolutionKind {
    Unchanged,
    Alias,
    Replacement,
    Unverified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedModelSelectionResolution {
    pub provider_id: String,
    #[serde(default)]
    pub provider_endpoint_id: Option<String>,
    pub model_id: String,
    pub kind: SelectionResolutionKind,
    pub requires_user_notice: bool,
}

pub fn resolve_saved_selection(
    saved: &SavedModelSelection,
    models: &[ModelDescriptor],
) -> SavedModelSelectionResolution {
    let matching = models.iter().find(|model| {
        selection_scope_matches(saved, model) && model.matches_id_or_alias(&saved.model_id)
    });

    let Some(model) = matching else {
        return SavedModelSelectionResolution {
            provider_id: saved.provider_id.trim().to_string(),
            provider_endpoint_id: saved.provider_endpoint_id.clone(),
            model_id: saved.model_id.trim().to_string(),
            kind: SelectionResolutionKind::Unverified,
            requires_user_notice: true,
        };
    };

    if model.lifecycle == ModelLifecycle::Removed {
        if let Some(replacement_id) = model.replacement_model_id.as_deref() {
            if let Some(replacement) = models.iter().find(|candidate| {
                candidate
                    .provider_id
                    .eq_ignore_ascii_case(&model.provider_id)
                    && candidate.lifecycle != ModelLifecycle::Removed
                    && candidate.matches_id_or_alias(replacement_id)
            }) {
                return resolution_from_model(
                    saved,
                    replacement,
                    SelectionResolutionKind::Replacement,
                    true,
                );
            }
        }
        return SavedModelSelectionResolution {
            provider_id: model.provider_id.clone(),
            provider_endpoint_id: saved.provider_endpoint_id.clone(),
            model_id: model.id.clone(),
            kind: SelectionResolutionKind::Unverified,
            requires_user_notice: true,
        };
    }

    let exact = model.id.eq_ignore_ascii_case(saved.model_id.trim());
    resolution_from_model(
        saved,
        model,
        if exact {
            SelectionResolutionKind::Unchanged
        } else {
            SelectionResolutionKind::Alias
        },
        !exact,
    )
}

fn selection_scope_matches(saved: &SavedModelSelection, model: &ModelDescriptor) -> bool {
    if let Some(endpoint_id) = saved
        .provider_endpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return model
            .endpoint_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(endpoint_id));
    }

    canonical_provider_id(&saved.provider_id) == canonical_provider_id(&model.provider_id)
}

fn canonical_provider_id(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "open_ai" => "openai".into(),
        "deep_seek" => "deepseek".into(),
        "lm_studio" => "lmstudio".into(),
        "qwen" | "dashscope" | "alibaba" => "alibaba_model_studio".into(),
        other => other.to_string(),
    }
}

fn resolution_from_model(
    saved: &SavedModelSelection,
    model: &ModelDescriptor,
    kind: SelectionResolutionKind,
    requires_user_notice: bool,
) -> SavedModelSelectionResolution {
    SavedModelSelectionResolution {
        provider_id: model.provider_id.clone(),
        provider_endpoint_id: saved
            .provider_endpoint_id
            .clone()
            .or_else(|| model.endpoint_ids.first().cloned()),
        model_id: model.id.clone(),
        kind,
        requires_user_notice,
    }
}
