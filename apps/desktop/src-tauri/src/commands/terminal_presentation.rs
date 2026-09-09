//! Read-only native terminal appearance and incremental PTY text decoding.
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAppearance {
    pub source: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub font_weight: Option<u16>,
    pub cursor_style: Option<String>,
    pub theme: BTreeMap<String, String>,
}

pub fn read_appearance(shell: &str, light: bool) -> TerminalAppearance {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return TerminalAppearance::default();
    };
    let local = std::path::PathBuf::from(local);
    for relative in [
        "Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json",
        "Microsoft/Windows Terminal/settings.json",
        "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json",
    ] {
        let path = local.join(relative);
        if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() <= 2 * 1024 * 1024) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(settings) = json5::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
            continue;
        };
        return appearance_from_settings(&settings, shell, light);
    }
    TerminalAppearance::default()
}

fn merge(target: &mut Value, source: &Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            if value.is_object() {
                merge(
                    target
                        .entry(key.clone())
                        .or_insert_with(|| serde_json::json!({})),
                    value,
                );
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

pub fn appearance_from_settings(settings: &Value, shell: &str, light: bool) -> TerminalAppearance {
    let shell = shell.to_ascii_lowercase();
    let profiles = settings["profiles"]["list"]
        .as_array()
        .or_else(|| settings["profiles"].as_array());
    let default_id = settings["defaultProfile"].as_str();
    let matches_shell = |profile: &&Value| {
        if profile["hidden"].as_bool() == Some(true) {
            return false;
        }
        let identity = format!(
            "{} {} {}",
            profile["commandline"].as_str().unwrap_or(""),
            profile["source"].as_str().unwrap_or(""),
            profile["name"].as_str().unwrap_or("")
        )
        .to_ascii_lowercase();
        match shell.as_str() {
            "default" => default_id.is_some() && profile["guid"].as_str() == default_id,
            "windows powershell" => {
                identity.contains("windows powershell") || identity.contains("powershell.exe")
            }
            "powershell" | "pwsh" => identity.contains("pwsh") || identity.contains("powershell"),
            "cmd" | "command prompt" => {
                identity.contains("cmd.exe")
                    || profile["guid"].as_str() == Some("{0caa0dad-35be-5f56-a8ff-afceeeaa6101}")
            }
            "bash" | "git bash" => identity.contains("bash"),
            _ => false,
        }
    };
    let profile = profiles.and_then(|profiles| {
        profiles.iter().filter(matches_shell).max_by_key(|profile| {
            let is_core = profile["commandline"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("pwsh")
                || profile["source"].as_str() == Some("Windows.Terminal.PowershellCore");
            // The running `PowerShell` label denotes pwsh; Windows PowerShell has
            // its own label and profile. Prefer the user's default within a shell.
            u8::from(is_core && matches!(shell.as_str(), "powershell" | "pwsh")) * 2
                + u8::from(default_id.is_some() && profile["guid"].as_str() == default_id)
        })
    });
    let mut effective = serde_json::json!({ "font": { "face": "Cascadia Mono", "size": 12 }, "colorScheme": "Campbell", "cursorShape": "bar" });
    merge(&mut effective, &settings["profiles"]["defaults"]);
    if let Some(profile) = profile {
        merge(&mut effective, profile);
    }
    let scheme_name = effective["colorScheme"]
        .as_str()
        .or_else(|| effective["colorScheme"][if light { "light" } else { "dark" }].as_str())
        .unwrap_or("Campbell");
    let builtins: Value = serde_json::from_str(
        include_str!("terminal_color_schemes.json").trim_start_matches('\u{feff}'),
    )
    .unwrap_or_default();
    let mut colors = builtins["schemes"]
        .as_array()
        .and_then(|schemes| schemes.iter().find(|scheme| scheme["name"] == scheme_name))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(custom) = settings["schemes"]
        .as_array()
        .and_then(|schemes| schemes.iter().find(|scheme| scheme["name"] == scheme_name))
    {
        merge(&mut colors, custom);
    }
    for name in [
        "foreground",
        "background",
        "cursorColor",
        "selectionBackground",
    ] {
        if !effective[name].is_null() {
            colors[name] = effective[name].clone();
        }
    }
    let mut theme = BTreeMap::new();
    for name in [
        "foreground",
        "background",
        "cursorColor",
        "selectionBackground",
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "purple",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightPurple",
        "brightCyan",
        "brightWhite",
    ] {
        if let Some(color) = colors[name].as_str().filter(|color| {
            matches!(color.len(), 4 | 7)
                && color.starts_with('#')
                && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            let key = match name {
                "purple" => "magenta",
                "brightPurple" => "brightMagenta",
                "cursorColor" => "cursor",
                _ => name,
            };
            theme.insert(key.to_string(), color.to_string());
        }
    }
    let font = &effective["font"];
    TerminalAppearance {
        source: Some("Windows Terminal".to_string()),
        font_family: font["face"]
            .as_str()
            .filter(|face| !face.trim().is_empty() && face.len() <= 256)
            .map(str::to_string),
        // Windows Terminal uses points; xterm uses CSS pixels.
        font_size: font["size"]
            .as_f64()
            .filter(|size| size.is_finite() && (4.0..=96.0).contains(size))
            .map(|size| size * 96.0 / 72.0),
        font_weight: font["weight"]
            .as_u64()
            .filter(|weight| (100..=900).contains(weight))
            .map(|weight| weight as u16),
        cursor_style: Some(
            match effective["cursorShape"].as_str().unwrap_or("bar") {
                "filledBox" | "emptyBox" => "block",
                "underscore" | "doubleUnderscore" => "underline",
                _ => "bar",
            }
            .to_string(),
        ),
        theme,
    }
}

#[derive(Default)]
pub struct TerminalTextDecoder {
    pending: Vec<u8>,
}

impl TerminalTextDecoder {
    pub fn decode(&mut self, bytes: &[u8], finished: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        let mut consumed = 0;
        while consumed < self.pending.len() {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(text) => {
                    output.push_str(text);
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    output.push_str(
                        std::str::from_utf8(&self.pending[consumed..consumed + valid])
                            .unwrap_or_default(),
                    );
                    consumed += valid;
                    match error.error_len() {
                        Some(length) => {
                            output.push('\u{fffd}');
                            consumed += length;
                        }
                        None if finished => {
                            output.push('\u{fffd}');
                            consumed = self.pending.len();
                        }
                        None => break,
                    }
                }
            }
        }
        self.pending.drain(..consumed);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_utf8_survives_every_byte_boundary() {
        let text = "\x1b[38;2;211;54;130m中文  󰊢 👩‍💻\x1b[0m";
        for split in 0..=text.len() {
            let mut decoder = TerminalTextDecoder::default();
            let first = decoder.decode(&text.as_bytes()[..split], false);
            assert_eq!(
                first + &decoder.decode(&text.as_bytes()[split..], true),
                text
            );
        }
        let mut decoder = TerminalTextDecoder::default();
        assert_eq!(decoder.decode(&[0xe4], false), "");
        assert_eq!(decoder.decode(&[], true), "�");
    }
    #[test]
    fn terminal_inherits_jsonc_profile_and_native_palette() {
        let settings: Value = json5::from_str(r##"{
            // Defaults are inherited by every shell.
            "defaultProfile": "ps", "profiles": { "defaults": { "font": { "face": "JetBrainsMono Nerd Font", "size": 15 }, "colorScheme": "Solarized Light" },
            "list": [{"guid":"ps","name":"PowerShell","font":{"size":18},"foreground":"#112233"}, {"name":"Cmd","commandline":"cmd.exe","font":{"size":10}}], },
        }"##).unwrap();
        let appearance = appearance_from_settings(&settings, "default", false);
        assert_eq!(
            appearance.font_family.as_deref(),
            Some("JetBrainsMono Nerd Font")
        );
        assert_eq!(appearance.font_size, Some(24.0));
        assert_eq!(appearance.theme["background"].to_lowercase(), "#fdf6e3");
        assert_eq!(appearance.theme["magenta"].to_lowercase(), "#d33682");
        assert_eq!(appearance.theme["foreground"], "#112233");
        assert_eq!(
            appearance_from_settings(&settings, "cmd", false).font_size,
            Some(40.0 / 3.0)
        );
    }
}
