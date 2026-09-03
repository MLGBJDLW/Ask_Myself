use crate::error::CoreError;

use super::model::{
    derive_skill_metadata, Skill, SkillFrontmatter, SkillResourceEncoding, SkillResourceFile,
};
use super::storage::{
    builtin_skill_source_path, resource_bundle_metadata, resource_kind_from_relative_path,
    substitute_skill_dir,
};

pub(crate) struct BuiltinSkillBundle {
    pub(crate) slug: &'static str,
    pub(crate) skill_md: &'static str,
    pub(crate) resources: &'static [BuiltinSkillResource],
}

pub(crate) struct BuiltinSkillResource {
    pub(crate) path: &'static str,
    pub(crate) content: &'static str,
}

// The build script recursively embeds every UTF-8 file under each directory that
// contains a SKILL.md. This makes the materialized bundle an automatic resource
// closure instead of a second hand-maintained list that can drift from source.
include!(concat!(env!("OUT_DIR"), "/builtin_skills.rs"));

pub(crate) fn builtin_skill_bundles() -> &'static [BuiltinSkillBundle] {
    BUILTIN_SKILLS
}

/// Parse a SKILL.md file (YAML frontmatter + markdown body).
///
/// The frontmatter must be delimited by `---` on its own line at the start
/// of the file, and closed by another `---` line.
pub fn parse_skill_file(content: &str) -> Result<(SkillFrontmatter, String), CoreError> {
    let trimmed = content.trim_start_matches('\u{feff}');
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            CoreError::InvalidInput("SKILL.md must start with YAML frontmatter (---)".into())
        })?;

    let (front_matter_text, body) = split_frontmatter(rest)?;
    let fm: SkillFrontmatter = serde_yaml::from_str(front_matter_text)
        .map_err(|e| CoreError::InvalidInput(format!("Invalid SKILL.md YAML frontmatter: {e}")))?;

    if fm.name.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "SKILL.md frontmatter must include a non-empty `name`".into(),
        ));
    }

    Ok((fm, body.trim().to_string()))
}

pub(crate) fn split_frontmatter(rest: &str) -> Result<(&str, &str), CoreError> {
    let mut cursor = 0;
    for line in rest.split_inclusive('\n') {
        let stripped = line.trim_end_matches(['\n', '\r']);
        if stripped == "---" {
            let fm = &rest[..cursor];
            let body_start = cursor + line.len();
            let body = &rest[body_start..];
            return Ok((fm, body));
        }
        cursor += line.len();
    }
    Err(CoreError::InvalidInput(
        "SKILL.md frontmatter is not closed with `---`".into(),
    ))
}

/// Load all built-in skills bundled with the binary.
pub fn load_builtin_skills() -> Vec<Skill> {
    let mut out = Vec::with_capacity(BUILTIN_SKILLS.len());
    for bundle in BUILTIN_SKILLS {
        match parse_skill_file(bundle.skill_md) {
            Ok((fm, body)) => {
                let body = substitute_skill_dir(body, bundle.slug);
                let resource_bundle = bundle
                    .resources
                    .iter()
                    .map(|resource| SkillResourceFile {
                        path: resource.path.to_string(),
                        kind: resource_kind_from_relative_path(resource.path),
                        encoding: SkillResourceEncoding::Utf8,
                        content: resource.content.to_string(),
                    })
                    .collect::<Vec<_>>();
                let (interface, dependencies, policy) =
                    derive_skill_metadata(&fm.name, &fm.description, &resource_bundle);
                out.push(Skill {
                    id: format!("builtin-{}", bundle.slug),
                    canonical_name: fm.name.clone(),
                    name: fm.name,
                    description: fm.description,
                    content: body,
                    enabled: true,
                    created_at: String::new(),
                    updated_at: String::new(),
                    builtin: true,
                    interface,
                    dependencies,
                    policy,
                    source_path: builtin_skill_source_path(bundle.slug),
                    resources: resource_bundle_metadata(&resource_bundle),
                    resource_bundle,
                });
            }
            Err(e) => {
                tracing::error!(skill = bundle.slug, error = %e, "Failed to parse bundled SKILL.md");
            }
        }
    }
    out
}
