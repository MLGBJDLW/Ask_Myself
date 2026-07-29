use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::db::Database;
use crate::error::CoreError;
use uuid::Uuid;

use super::model::{
    derive_skill_metadata, SaveSkillInput, Skill, SkillResourceEncoding, SkillResourceFile,
    SkillResourceInfo, SkillResourceKind, OPENAI_AGENT_METADATA_PATH,
};
use super::prompt::export_skill_to_md;
use super::registry::builtin_skill_bundles;

/// Global base directory where built-in skill bundles have been materialized to
/// disk. Set by [`materialize_skills_to_disk`] at startup; if unset, the
/// `<SKILL_DIR>` placeholder in bundled SKILL.md bodies is left untouched so
/// the model can still reason about relative paths.
static SKILLS_BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Substitute `<SKILL_DIR>` in a bundled skill body with the materialized
/// on-disk path for that skill, if materialization has been performed.
pub(crate) fn substitute_skill_dir(body: String, slug: &str) -> String {
    if !body.contains("<SKILL_DIR>") {
        return body;
    }
    match SKILLS_BASE_DIR.get() {
        Some(base) => {
            let skill_dir = base.join(slug);
            body.replace("<SKILL_DIR>", &skill_dir.to_string_lossy())
        }
        None => body,
    }
}

pub(crate) fn builtin_skill_source_path(slug: &str) -> Option<String> {
    SKILLS_BASE_DIR.get().map(|base| {
        base.join(slug)
            .join("SKILL.md")
            .to_string_lossy()
            .to_string()
    })
}

/// Materialize all bundled built-in skills (SKILL.md + scripts/references/assets)
/// onto disk under `<app_data_dir>/skills/<slug>/`. Idempotent: skips files
/// whose on-disk content already matches the embedded content. Per-file
/// failures are logged but do not abort other skills.
///
/// Returns the base `<app_data_dir>/skills/` path on success. The base path is
/// also stored in a process-global `OnceLock` so [`load_builtin_skills`] can
/// substitute `<SKILL_DIR>` placeholders in skill bodies with real paths.
pub fn materialize_skills_to_disk(app_data_dir: &Path) -> Result<PathBuf, CoreError> {
    let base = app_data_dir.join("skills");
    fs::create_dir_all(&base).map_err(|e| {
        CoreError::Internal(format!(
            "Failed to create skills base dir {}: {e}",
            base.display()
        ))
    })?;

    for bundle in builtin_skill_bundles() {
        let skill_dir = base.join(bundle.slug);
        if let Err(e) = fs::create_dir_all(&skill_dir) {
            tracing::warn!(skill = bundle.slug, error = %e, "Failed to create skill dir");
            continue;
        }
        write_if_changed(
            &skill_dir.join("SKILL.md"),
            bundle.skill_md.as_bytes(),
            bundle.slug,
        );
        for resource in bundle.resources {
            let target = skill_dir.join(resource.path);
            if let Some(parent) = target.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    tracing::warn!(
                        skill = bundle.slug,
                        path = %target.display(),
                        error = %e,
                        "Failed to create resource parent dir"
                    );
                    continue;
                }
            }
            write_if_changed(&target, resource.content.as_bytes(), bundle.slug);
        }
    }

    // Record the base dir so skill-body rendering can substitute <SKILL_DIR>.
    // `OnceLock::set` returns Err if already set — that's fine; first call wins.
    let _ = SKILLS_BASE_DIR.set(base.clone());
    Ok(base)
}

/// Return the on-disk directory where a bundled skill is materialized.
///
/// This is intentionally path-only: callers that need guaranteed files should
/// call [`materialize_skills_to_disk`] first.
pub fn builtin_skill_dir(app_data_dir: &Path, slug: &str) -> PathBuf {
    app_data_dir.join("skills").join(slug)
}

fn safe_skill_dir_name(id: &str) -> String {
    let safe = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "skill".to_string()
    } else {
        safe
    }
}

/// Return the on-disk directory where a user-created skill should be
/// materialized. User skills are stored separately from built-ins so user
/// scripts/assets cannot collide with bundled skill resources.
pub fn user_skill_dir(app_data_dir: &Path, skill_id: &str) -> PathBuf {
    app_data_dir
        .join("skills")
        .join("user")
        .join(safe_skill_dir_name(skill_id))
}

fn substitute_user_skill_dir(body: String, skill_id: &str) -> String {
    if !body.contains("<SKILL_DIR>") {
        return body;
    }
    if let Some(base) = SKILLS_BASE_DIR.get() {
        let skill_dir = base.join("user").join(safe_skill_dir_name(skill_id));
        body.replace("<SKILL_DIR>", &skill_dir.to_string_lossy())
    } else {
        body
    }
}

pub(crate) fn user_skill_source_path(skill_id: &str) -> Option<String> {
    SKILLS_BASE_DIR.get().map(|base| {
        base.join("user")
            .join(safe_skill_dir_name(skill_id))
            .join("SKILL.md")
            .to_string_lossy()
            .to_string()
    })
}

pub fn materialize_user_skill_to_disk(
    app_data_dir: &Path,
    skill: &Skill,
) -> Result<PathBuf, CoreError> {
    if skill.builtin {
        return Err(CoreError::InvalidInput(
            "Built-in skills are materialized by materialize_skills_to_disk".into(),
        ));
    }

    let skill_dir = user_skill_dir(app_data_dir, &skill.id);
    if skill_dir.exists() {
        fs::remove_dir_all(&skill_dir).map_err(|e| {
            CoreError::Internal(format!(
                "Failed to replace user skill dir {}: {e}",
                skill_dir.display()
            ))
        })?;
    }
    fs::create_dir_all(&skill_dir).map_err(|e| {
        CoreError::Internal(format!(
            "Failed to create user skill dir {}: {e}",
            skill_dir.display()
        ))
    })?;

    fs::write(skill_dir.join("SKILL.md"), export_skill_to_md(skill)).map_err(|e| {
        CoreError::Internal(format!(
            "Failed to write user skill SKILL.md for {}: {e}",
            skill.id
        ))
    })?;

    let resources = normalize_resource_bundle(&skill.resource_bundle)?;
    for resource in resources {
        let target = skill_dir.join(&resource.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to create user skill resource dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let bytes = match resource.encoding {
            SkillResourceEncoding::Utf8 => resource.content.into_bytes(),
            SkillResourceEncoding::Base64 => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(resource.content)
                    .map_err(|e| {
                        CoreError::InvalidInput(format!(
                            "Invalid base64 skill resource {}: {e}",
                            resource.path
                        ))
                    })?
            }
        };
        fs::write(&target, bytes).map_err(|e| {
            CoreError::Internal(format!(
                "Failed to write user skill resource {}: {e}",
                target.display()
            ))
        })?;
    }

    Ok(skill_dir)
}

pub fn materialize_user_skills_to_disk(
    app_data_dir: &Path,
    skills: &[Skill],
) -> Result<(), CoreError> {
    for skill in skills {
        if skill.builtin {
            continue;
        }
        if skill.enabled {
            materialize_user_skill_to_disk(app_data_dir, skill)?;
        } else {
            remove_materialized_user_skill(app_data_dir, &skill.id)?;
        }
    }
    Ok(())
}

pub fn remove_materialized_user_skill(
    app_data_dir: &Path,
    skill_id: &str,
) -> Result<(), CoreError> {
    let skill_dir = user_skill_dir(app_data_dir, skill_id);
    if skill_dir.exists() {
        fs::remove_dir_all(&skill_dir).map_err(|e| {
            CoreError::Internal(format!(
                "Failed to remove user skill dir {}: {e}",
                skill_dir.display()
            ))
        })?;
    }
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8], skill_slug: &str) {
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return;
        }
    }
    if let Err(e) = fs::write(path, bytes) {
        tracing::warn!(
            skill = skill_slug,
            path = %path.display(),
            error = %e,
            "Failed to write skill file"
        );
    }
}

pub(crate) fn resource_kind_from_relative_path(path: &str) -> SkillResourceKind {
    if path.starts_with("scripts/") {
        SkillResourceKind::Script
    } else if path.starts_with("references/") {
        SkillResourceKind::Reference
    } else if path == OPENAI_AGENT_METADATA_PATH {
        SkillResourceKind::Metadata
    } else {
        SkillResourceKind::Asset
    }
}

pub(crate) fn resource_bundle_metadata(resources: &[SkillResourceFile]) -> Vec<SkillResourceInfo> {
    resources
        .iter()
        .map(|resource| SkillResourceInfo {
            path: resource.path.clone(),
            kind: resource.kind.clone(),
            bytes: match resource.encoding {
                SkillResourceEncoding::Utf8 => resource.content.len(),
                SkillResourceEncoding::Base64 => resource.content.len().saturating_mul(3) / 4,
            },
        })
        .collect()
}

pub(crate) fn normalize_resource_bundle(
    resources: &[SkillResourceFile],
) -> Result<Vec<SkillResourceFile>, CoreError> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(resources.len());
    for resource in resources {
        let path = resource.path.trim().replace('\\', "/");
        if path.is_empty() {
            return Err(CoreError::InvalidInput(
                "Skill resource path cannot be empty".into(),
            ));
        }
        if path.contains("..") || path.starts_with('/') {
            return Err(CoreError::InvalidInput(format!(
                "Skill resource path must stay relative: {}",
                resource.path
            )));
        }
        if !seen.insert(path.clone()) {
            return Err(CoreError::InvalidInput(format!(
                "Duplicate skill resource path: {path}"
            )));
        }
        normalized.push(SkillResourceFile {
            kind: resource_kind_from_relative_path(&path),
            path,
            encoding: resource.encoding.clone(),
            content: resource.content.clone(),
        });
    }
    Ok(normalized)
}

fn serialize_resource_bundle(resources: &[SkillResourceFile]) -> Result<Option<String>, CoreError> {
    if resources.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(resources)
            .map(Some)
            .map_err(CoreError::from)
    }
}

fn deserialize_resource_bundle(raw: Option<String>) -> Result<Vec<SkillResourceFile>, CoreError> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => {
            let parsed: Vec<SkillResourceFile> = serde_json::from_str(&raw)?;
            normalize_resource_bundle(&parsed)
        }
        _ => Ok(Vec::new()),
    }
}

fn skill_from_row(row: &rusqlite::Row<'_>) -> Result<Skill, rusqlite::Error> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let description: String = row.get(2)?;
    let content: String = row.get(3)?;
    let resource_bundle_raw: Option<String> = row.get(7)?;
    let resource_bundle = deserialize_resource_bundle(resource_bundle_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let (interface, dependencies, policy) =
        derive_skill_metadata(&name, &description, &resource_bundle);
    Ok(Skill {
        id: id.clone(),
        name,
        description,
        content: substitute_user_skill_dir(content, &id),
        enabled: row.get::<_, i32>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        builtin: false,
        interface,
        dependencies,
        policy,
        source_path: user_skill_source_path(&id),
        resources: resource_bundle_metadata(&resource_bundle),
        resource_bundle,
    })
}

fn normalize_skill_input(input: &SaveSkillInput) -> Result<SaveSkillInput, CoreError> {
    let name = input
        .name
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let description = input.description.trim().to_string();
    let content = input.content.trim().to_string();

    if name.is_empty() {
        return Err(CoreError::InvalidInput("Skill name cannot be empty".into()));
    }

    if content.is_empty() {
        return Err(CoreError::InvalidInput(
            "Skill content cannot be empty".into(),
        ));
    }

    if description.len() > 2000 {
        return Err(CoreError::InvalidInput(
            "Skill description is too long (max 2000 chars)".into(),
        ));
    }

    Ok(SaveSkillInput {
        id: input.id.clone(),
        name,
        description,
        content,
        enabled: input.enabled,
        resource_bundle: normalize_resource_bundle(&input.resource_bundle)?,
    })
}

impl Database {
    /// List all user skills, newest first. Built-ins live in the static registry,
    /// while historical database rows were removed by migration v048.
    pub fn list_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, content, enabled, created_at, updated_at, resource_bundle_json
             FROM skills
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], skill_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Create or update a user skill.
    pub fn save_skill(&self, input: &SaveSkillInput) -> Result<Skill, CoreError> {
        let input = normalize_skill_input(input)?;
        let conn = self.conn();
        let resource_bundle_json = serialize_resource_bundle(&input.resource_bundle)?;
        let id = match &input.id {
            Some(existing_id) => {
                conn.execute(
                    "UPDATE skills
                     SET name = ?2, description = ?3, content = ?4, enabled = ?5,
                         resource_bundle_json = ?6,
                         updated_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![
                        existing_id,
                        &input.name,
                        &input.description,
                        &input.content,
                        input.enabled as i32,
                        &resource_bundle_json
                    ],
                )?;
                existing_id.clone()
            }
            None => {
                let new_id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO skills (id, name, description, content, enabled, resource_bundle_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        &new_id,
                        &input.name,
                        &input.description,
                        &input.content,
                        input.enabled as i32,
                        &resource_bundle_json
                    ],
                )?;
                new_id
            }
        };
        drop(conn);
        self.get_skill(&id)
    }

    /// Delete a user skill by ID.
    pub fn delete_skill(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM skills WHERE id = ?1", rusqlite::params![id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Skill {id}")));
        }
        Ok(())
    }

    /// Toggle a user skill's enabled state.
    pub fn toggle_skill(&self, id: &str, enabled: bool) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE skills SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Skill {id}")));
        }
        Ok(())
    }

    /// Get enabled database-backed user skills. Static built-ins are combined by
    /// the caller and exact IDs are deduplicated there.
    pub fn get_enabled_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, content, enabled, created_at, updated_at, resource_bundle_json
             FROM skills
             WHERE enabled = 1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], skill_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn get_skill(&self, id: &str) -> Result<Skill, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, description, content, enabled, created_at, updated_at, resource_bundle_json
             FROM skills
             WHERE id = ?1",
            rusqlite::params![id],
            skill_from_row,
        )
        .map_err(|_| CoreError::NotFound(format!("Skill {id}")))
    }
}
