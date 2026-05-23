use super::model::{SkillWarning, SkillWarningSeverity};
use super::registry::split_frontmatter;

/// Size at which a SKILL.md import gets an advisory warning.
///
/// Large skills are valid; this is a review signal, not an import cap. Keep it
/// high enough for longform Top-agent style skills with bundled workflows.
pub(crate) const SKILL_MAX_BYTES: usize = 256 * 1024;

/// Scan raw SKILL.md text for suspicious patterns before import.
///
/// Pure function — does not modify the database or import state. Returns a
/// list of findings so the UI can decide whether to surface a confirmation
/// dialog. The importer itself still runs unchanged; scanning is advisory.
pub fn scan_skill_content(content: &str) -> Vec<SkillWarning> {
    let mut warnings = Vec::new();

    // Size check.
    if content.len() > SKILL_MAX_BYTES {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Warn,
            "size.too_large",
            format!(
                "SKILL.md is unusually large ({} KB > {} KB).",
                content.len() / 1024,
                SKILL_MAX_BYTES / 1024,
            ),
        ));
    }

    // Frontmatter-structural checks.
    let (fm_name, fm_description, allowed_tools) = extract_frontmatter_fields(content);
    if fm_name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Warn,
            "frontmatter.missing_name",
            "Frontmatter is missing a non-empty `name` field.",
        ));
    }
    if fm_description
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Info,
            "frontmatter.missing_description",
            "Frontmatter is missing a `description` — matching will fall back to name/content only.",
        ));
    }

    // allowed-tools permissions.
    for tool in &allowed_tools {
        let t = tool.trim();
        if t == "*" {
            warnings.push(SkillWarning::new(
                SkillWarningSeverity::Warn,
                "permissions.wildcard_tools",
                "allowed-tools contains `*` — grants access to every tool.",
            ));
        } else if t.eq_ignore_ascii_case("run_shell_tool") || t.eq_ignore_ascii_case("shell") {
            warnings.push(SkillWarning::new(
                SkillWarningSeverity::Warn,
                "permissions.shell_tool",
                format!("allowed-tools grants shell access via `{t}`."),
            ));
        }
    }

    // Suspicious body patterns. Use case-insensitive substring checks — skills
    // are prose, so false positives are tolerable and easy for the user to
    // confirm-through.
    let body_lower = content.to_lowercase();

    if contains_rm_rf(&body_lower) {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Block,
            "pattern.rm_rf",
            "Contains `rm -rf` — recursive force deletion.",
        ));
    }
    if contains_curl_pipe_sh(&body_lower) {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Block,
            "pattern.curl_pipe_sh",
            "Contains `curl … | sh` — remote script execution.",
        ));
    }
    if body_lower.contains("eval(") {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Warn,
            "pattern.eval",
            "Contains `eval(` — dynamic code evaluation.",
        ));
    }
    if body_lower.contains("base64 -d") || body_lower.contains("base64 --decode") {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Warn,
            "pattern.base64_decode",
            "Contains `base64 -d` — decoded payload execution.",
        ));
    }
    if contains_shell_subst(content) {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Info,
            "pattern.shell_subst",
            "Contains shell substitution `$(…)` — verify command contents.",
        ));
    }
    if has_long_hex_escape_run(content) {
        warnings.push(SkillWarning::new(
            SkillWarningSeverity::Warn,
            "pattern.hex_escape_run",
            "Contains a long run of hex escape sequences (possible obfuscation).",
        ));
    }

    warnings
}

/// Best-effort extraction of a few frontmatter fields without forcing a hard
/// parse — a malformed frontmatter still yields actionable warnings.
///
/// Returns `(name, description, allowed_tools)`.
fn extract_frontmatter_fields(content: &str) -> (Option<String>, Option<String>, Vec<String>) {
    let trimmed = content.trim_start_matches('\u{feff}');
    let rest = match trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    {
        Some(r) => r,
        None => return (None, None, Vec::new()),
    };
    let Ok((fm_text, _body)) = split_frontmatter(rest) else {
        return (None, None, Vec::new());
    };

    let mut name = None;
    let mut description = None;
    let mut allowed_tools = Vec::new();
    let mut in_tools_list = false;

    for line in fm_text.lines() {
        let raw = line.trim_end_matches('\r');
        let trimmed_start = raw.trim_start();
        if let Some(rest) = raw.strip_prefix("name:") {
            name = Some(unquote_yaml_scalar(rest.trim()));
            in_tools_list = false;
        } else if let Some(rest) = raw.strip_prefix("description:") {
            description = Some(unquote_yaml_scalar(rest.trim()));
            in_tools_list = false;
        } else if let Some(rest) = raw.strip_prefix("allowed-tools:") {
            let rest = rest.trim();
            if rest.is_empty() {
                in_tools_list = true;
            } else if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                in_tools_list = false;
                for item in inner.split(',') {
                    let s = unquote_yaml_scalar(item.trim());
                    if !s.is_empty() {
                        allowed_tools.push(s);
                    }
                }
            } else {
                in_tools_list = false;
            }
        } else if in_tools_list {
            if let Some(rest) = trimmed_start.strip_prefix("- ") {
                let s = unquote_yaml_scalar(rest.trim());
                if !s.is_empty() {
                    allowed_tools.push(s);
                }
            } else if !trimmed_start.is_empty() && !raw.starts_with(' ') && !raw.starts_with('\t') {
                in_tools_list = false;
            }
        }
    }

    (name, description, allowed_tools)
}

fn unquote_yaml_scalar(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

fn contains_rm_rf(body_lower: &str) -> bool {
    // Match `rm -rf`, `rm  -rf`, `rm -fr`, `rm -Rf`, tolerating whitespace.
    for (idx, _) in body_lower.match_indices("rm ") {
        let tail = &body_lower[idx + 3..];
        let tail = tail.trim_start();
        if tail.starts_with("-rf")
            || tail.starts_with("-fr")
            || tail.starts_with("-r ")
            || tail.starts_with("-r\t")
        {
            return true;
        }
    }
    false
}

fn contains_curl_pipe_sh(body_lower: &str) -> bool {
    // Very conservative: "curl" appearing before "| sh" or "|sh" on the same
    // line is enough to flag.
    for line in body_lower.lines() {
        if line.contains("curl") && (line.contains("| sh") || line.contains("|sh")) {
            return true;
        }
        if line.contains("wget") && (line.contains("| sh") || line.contains("|sh")) {
            return true;
        }
    }
    false
}

fn contains_shell_subst(content: &str) -> bool {
    // Look for `$(` outside fenced code blocks is overkill; flagging anywhere
    // is acceptable for an info-level warning.
    content.contains("$(")
}

fn has_long_hex_escape_run(content: &str) -> bool {
    // Detect runs of 4+ consecutive \xNN escapes — common in obfuscated payloads.
    let bytes = content.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == b'\\' && bytes[i + 1] == b'x' {
            let mut count = 0;
            let mut j = i;
            while j + 3 < bytes.len()
                && bytes[j] == b'\\'
                && bytes[j + 1] == b'x'
                && bytes[j + 2].is_ascii_hexdigit()
                && bytes[j + 3].is_ascii_hexdigit()
            {
                count += 1;
                j += 4;
            }
            if count >= 4 {
                return true;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    false
}
