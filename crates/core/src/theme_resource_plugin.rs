//! Declarative theme-resource plugin contract.
//!
//! Theme plugins intentionally contain data, not executable CSS or script.
//! The host validates semantic tokens and managed image references before the
//! renderer can preview or install a generated theme.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::CoreError;

pub const THEME_RESOURCE_PLUGIN_KIND: &str = "theme-resource";
pub const THEME_RESOURCE_PLUGIN_VERSION: u8 = 2;

const COLOR_SLOTS: &[&str] = &[
    "surface0",
    "surface1",
    "surface2",
    "surface3",
    "surface4",
    "textPrimary",
    "textSecondary",
    "textTertiary",
    "textInverse",
    "thinkingText",
    "replyText",
    "accent",
    "accentHover",
    "accentSubtle",
    "success",
    "warning",
    "danger",
    "info",
    "border",
    "borderHover",
    "borderActive",
    "contextPrompts",
    "contextConversation",
    "contextToolResults",
    "contextTools",
    "contextMcp",
    "contextOverhead",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourcePlugin {
    pub manifest_version: u8,
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub theme: ThemeResourceDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceDefinition {
    pub base_theme: String,
    pub mode: String,
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub effects: ThemeResourceEffects,
    #[serde(default)]
    pub typography: ThemeResourceTypography,
    #[serde(default)]
    pub motion: ThemeResourceMotion,
    #[serde(default)]
    pub brand: ThemeResourceBrand,
    #[serde(default)]
    pub content: ThemeResourceContent,
    #[serde(default)]
    pub components: BTreeMap<String, ThemeResourceComponentStyle>,
    #[serde(default)]
    pub background: ThemeResourceBackground,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceEffects {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glass_blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_intensity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density_scale: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceTypography {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceMotion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_style: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceBrand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_foreground: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_muted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_opacity: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceComponentStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_shadow: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeResourceBackground {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_color: Option<String>,
}

impl Default for ThemeResourceBackground {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            value: None,
            asset_id: None,
            fit: Some("cover".to_string()),
            position: Some("center".to_string()),
            opacity: Some(1.0),
            dim: Some(0.0),
            blur: Some(0.0),
            overlay_color: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedThemeResource {
    name: String,
    theme: ThemeResourceDefinition,
}

impl ThemeResourcePlugin {
    pub fn from_generated_value(value: Value, description: &str) -> Result<Self, CoreError> {
        let generated: GeneratedThemeResource = serde_json::from_value(value).map_err(|error| {
            CoreError::InvalidInput(format!(
                "Generated theme did not match the contract: {error}"
            ))
        })?;
        Self {
            manifest_version: THEME_RESOURCE_PLUGIN_VERSION,
            kind: THEME_RESOURCE_PLUGIN_KIND.to_string(),
            id: format!("theme-{}", Uuid::new_v4().simple()),
            name: generated.name,
            description: Some(description.trim().chars().take(500).collect()),
            theme: generated.theme,
        }
        .normalize()
    }

    pub fn normalize(mut self) -> Result<Self, CoreError> {
        if !matches!(self.manifest_version, 1 | THEME_RESOURCE_PLUGIN_VERSION) {
            return invalid("Unsupported theme-resource manifest version");
        }
        self.manifest_version = THEME_RESOURCE_PLUGIN_VERSION;
        if self.kind != THEME_RESOURCE_PLUGIN_KIND {
            return invalid("Invalid theme-resource plugin kind");
        }
        if !valid_id(&self.id) {
            return invalid("Invalid theme-resource plugin id");
        }
        self.name = self.name.trim().to_string();
        if self.name.is_empty() || self.name.chars().count() > 80 {
            return invalid("Theme-resource name must contain 1 to 80 characters");
        }
        if let Some(description) = self.description.take() {
            let description = description.trim().to_string();
            if description.chars().count() > 500 {
                return invalid("Theme-resource description cannot exceed 500 characters");
            }
            self.description = (!description.is_empty()).then_some(description);
        }
        if !matches!(
            self.theme.base_theme.as_str(),
            "dark" | "light" | "midnight" | "aurora" | "bloom" | "dream"
        ) {
            return invalid("Invalid base theme");
        }
        if !matches!(self.theme.mode.as_str(), "dark" | "light") {
            return invalid("Invalid theme mode");
        }
        for (slot, color) in &mut self.theme.colors {
            if !COLOR_SLOTS.contains(&slot.as_str()) {
                return invalid(format!("Unknown semantic color slot: {slot}"));
            }
            *color = color.trim().to_string();
            if !safe_color(color) {
                return invalid(format!("Invalid semantic color: {slot}"));
            }
            if slot.starts_with("surface") && color.eq_ignore_ascii_case("transparent") {
                return invalid(format!("Surface color cannot be transparent: {slot}"));
            }
        }
        clamp(&mut self.theme.effects.surface_opacity, 0.35, 1.0);
        clamp(&mut self.theme.effects.glass_blur, 0.0, 48.0);
        clamp(&mut self.theme.effects.shadow_intensity, 0.0, 2.0);
        clamp(&mut self.theme.effects.radius_scale, 0.5, 2.0);
        clamp(&mut self.theme.effects.density_scale, 0.8, 1.25);
        normalize_typography(&mut self.theme.typography)?;
        normalize_motion(&mut self.theme.motion)?;
        normalize_brand(&mut self.theme.brand)?;
        normalize_content(&mut self.theme.content)?;
        normalize_components(&mut self.theme.components)?;
        normalize_background(&mut self.theme.background)?;
        Ok(self)
    }
}

fn normalize_typography(typography: &mut ThemeResourceTypography) -> Result<(), CoreError> {
    normalize_font(&mut typography.font_family)?;
    normalize_font(&mut typography.mono_font_family)?;
    clamp(&mut typography.base_size, 12.0, 20.0);
    clamp(&mut typography.line_height, 1.2, 2.0);
    clamp(&mut typography.letter_spacing, -0.04, 0.12);
    Ok(())
}

fn normalize_font(value: &mut Option<String>) -> Result<(), CoreError> {
    let Some(font) = value.as_mut() else {
        return Ok(());
    };
    *font = font.trim().to_string();
    if font.is_empty() {
        *value = None;
        return Ok(());
    }
    if font.chars().count() > 160
        || font.to_ascii_lowercase().contains("url(")
        || font.to_ascii_lowercase().contains("@import")
        || !font.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character.is_whitespace()
                || "_,'\".-".contains(character)
        })
    {
        return invalid("Theme fonts must be local font-family names");
    }
    Ok(())
}

fn normalize_motion(motion: &mut ThemeResourceMotion) -> Result<(), CoreError> {
    clamp(&mut motion.duration_scale, 0.0, 2.0);
    if !matches!(
        motion.cursor_style.as_deref(),
        None | Some("precise" | "fluid" | "minimal")
    ) {
        return invalid("Invalid theme cursor style");
    }
    Ok(())
}

fn normalize_brand(brand: &mut ThemeResourceBrand) -> Result<(), CoreError> {
    if !matches!(
        brand.logo_variant.as_deref(),
        None | Some("auto" | "monochrome" | "accent")
    ) {
        return invalid("Invalid theme logo variant");
    }
    for color in [&mut brand.logo_foreground, &mut brand.logo_muted] {
        if let Some(value) = color.as_mut() {
            *value = value.trim().to_string();
            if !safe_color(value) {
                return invalid("Invalid theme logo color");
            }
        }
    }
    clamp(&mut brand.logo_opacity, 0.4, 1.0);
    Ok(())
}

fn normalize_content(content: &mut ThemeResourceContent) -> Result<(), CoreError> {
    normalize_plain_text(&mut content.tagline, 160)?;
    normalize_plain_text(&mut content.status_text, 80)?;
    normalize_plain_text(&mut content.quote, 240)?;
    Ok(())
}

fn normalize_plain_text(value: &mut Option<String>, maximum: usize) -> Result<(), CoreError> {
    let Some(text) = value.as_mut() else {
        return Ok(());
    };
    *text = text
        .trim()
        .chars()
        .map(|character| {
            if matches!(character, '\r' | '\n' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect();
    if text.is_empty() {
        *value = None;
    } else if text.chars().count() > maximum {
        return invalid(format!("Theme content exceeds {maximum} characters"));
    }
    Ok(())
}

fn normalize_components(
    components: &mut BTreeMap<String, ThemeResourceComponentStyle>,
) -> Result<(), CoreError> {
    for (slot, style) in components {
        if !matches!(slot.as_str(), "rail" | "header" | "card" | "browser") {
            return invalid(format!("Unknown theme component slot: {slot}"));
        }
        if let Some(background) = style.background.as_mut() {
            *background = background.trim().to_string();
            if !safe_color(background) && !safe_gradient(background) {
                return invalid(format!("Invalid {slot} background"));
            }
        }
        if let Some(border) = style.border_color.as_mut() {
            *border = border.trim().to_string();
            if !safe_color(border) {
                return invalid(format!("Invalid {slot} border color"));
            }
        }
        if let Some(shadow) = style.box_shadow.as_mut() {
            *shadow = shadow.trim().to_string();
            if shadow.chars().count() > 240
                || unsafe_css(shadow)
                || !shadow.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "#.%(), /+-".contains(character)
                })
            {
                return invalid(format!("Invalid {slot} shadow"));
            }
        }
    }
    Ok(())
}

fn normalize_background(background: &mut ThemeResourceBackground) -> Result<(), CoreError> {
    if !matches!(
        background.kind.as_str(),
        "none" | "color" | "gradient" | "image"
    ) {
        return invalid("Invalid theme background kind");
    }
    if let Some(value) = background.value.as_mut() {
        *value = value.trim().to_string();
        if unsafe_css(value) {
            return invalid("Theme backgrounds cannot contain URLs or CSS rules");
        }
    }
    match background.kind.as_str() {
        "color"
            if background
                .value
                .as_deref()
                .is_none_or(|value| !safe_color(value)) =>
        {
            return invalid("Invalid theme background color");
        }
        "gradient"
            if background
                .value
                .as_deref()
                .is_none_or(|value| !safe_gradient(value)) =>
        {
            return invalid("Invalid theme background gradient");
        }
        "image"
            if background.asset_id.as_deref().is_none_or(|asset| {
                asset.len() != 64 || !asset.chars().all(|c| c.is_ascii_hexdigit())
            }) =>
        {
            return invalid("Theme images must reference a managed local asset id");
        }
        _ => {}
    }
    if !matches!(
        background.fit.as_deref(),
        None | Some("cover" | "contain" | "tile")
    ) {
        return invalid("Invalid theme background fit");
    }
    if let Some(position) = background.position.as_deref() {
        if position.is_empty()
            || !position
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".% +-".contains(c))
        {
            return invalid("Invalid theme background position");
        }
    }
    if let Some(color) = background.overlay_color.as_deref() {
        if !safe_color(color) {
            return invalid("Invalid theme background overlay color");
        }
    }
    clamp(&mut background.opacity, 0.0, 1.0);
    clamp(&mut background.dim, 0.0, 1.0);
    clamp(&mut background.blur, 0.0, 32.0);
    Ok(())
}

fn safe_color(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    lowered == "transparent"
        || value.strip_prefix('#').is_some_and(|hex| {
            matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
        })
        || ["rgb(", "rgba(", "hsl(", "hsla(", "oklch(", "oklab("]
            .iter()
            .any(|prefix| lowered.starts_with(prefix))
            && value.ends_with(')')
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".%(), /+-".contains(c))
}

fn safe_gradient(value: &str) -> bool {
    ["linear-gradient(", "radial-gradient(", "conic-gradient("]
        .iter()
        .any(|prefix| value.starts_with(prefix))
        && value.ends_with(')')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "#.%(), /+-".contains(c))
}

fn unsafe_css(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    lowered.contains("url(")
        || lowered.contains("@import")
        || value.chars().any(|c| matches!(c, ';' | '{' | '}'))
}

fn valid_id(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn clamp(value: &mut Option<f64>, minimum: f64, maximum: f64) {
    if let Some(number) = value {
        *number = number.clamp(minimum, maximum);
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, CoreError> {
    Err(CoreError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated_theme() -> Value {
        serde_json::json!({
            "name": "Quiet Ocean",
            "theme": {
                "baseTheme": "dark",
                "mode": "dark",
                "colors": {
                    "surface0": "#08131f",
                    "surface1": "#102235",
                    "textPrimary": "#f2f8ff",
                    "textSecondary": "#a8bed1",
                    "thinkingText": "#7dd3fc",
                    "replyText": "#fef3c7",
                    "accent": "#38bdf8"
                },
                "effects": { "surfaceOpacity": 0.9, "glassBlur": 14 },
                "background": {
                    "kind": "gradient",
                    "value": "linear-gradient(145deg, #08131f, #164e63)",
                    "fit": "cover",
                    "position": "center"
                }
            }
        })
    }

    #[test]
    fn generated_theme_becomes_a_valid_declarative_plugin() {
        let plugin =
            ThemeResourcePlugin::from_generated_value(generated_theme(), "calm ocean dashboard")
                .expect("valid plugin");

        assert_eq!(plugin.kind, THEME_RESOURCE_PLUGIN_KIND);
        assert_eq!(plugin.manifest_version, THEME_RESOURCE_PLUGIN_VERSION);
        assert!(plugin.id.starts_with("theme-"));
        assert_eq!(plugin.theme.colors["accent"], "#38bdf8");
        assert_eq!(plugin.theme.colors["thinkingText"], "#7dd3fc");
        assert_eq!(plugin.theme.colors["replyText"], "#fef3c7");
    }

    #[test]
    fn generated_theme_rejects_remote_backgrounds() {
        let mut value = generated_theme();
        value["theme"]["background"]["value"] =
            Value::String("linear-gradient(#000, #111); background: url(https://evil)".into());

        let error = ThemeResourcePlugin::from_generated_value(value, "unsafe")
            .expect_err("unsafe CSS must fail");
        assert!(error.to_string().contains("cannot contain URLs"));
    }

    #[test]
    fn image_background_requires_a_managed_asset_id() {
        let mut value = generated_theme();
        value["theme"]["background"] = serde_json::json!({ "kind": "image" });

        let error = ThemeResourcePlugin::from_generated_value(value, "image")
            .expect_err("unmanaged image must fail");
        assert!(error.to_string().contains("managed local asset id"));
    }

    #[test]
    fn semantic_surface_colors_cannot_bypass_effect_opacity() {
        let mut value = generated_theme();
        value["theme"]["colors"]["surface0"] = Value::String("transparent".into());

        let error = ThemeResourcePlugin::from_generated_value(value, "transparent surface")
            .expect_err("surface opacity must remain the only transparency authority");
        assert!(error.to_string().contains("cannot be transparent"));
    }

    #[test]
    fn plugin_description_uses_the_renderer_length_contract() {
        let mut plugin = ThemeResourcePlugin::from_generated_value(generated_theme(), "safe")
            .expect("valid plugin");
        plugin.description = Some("🌊".repeat(501));

        let error = plugin
            .normalize()
            .expect_err("overlong description must fail");
        assert!(error.to_string().contains("500 characters"));
    }
}
