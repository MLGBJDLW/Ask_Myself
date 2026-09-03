use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
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
static USER_SKILLS_DIR: OnceLock<PathBuf> = OnceLock::new();
pub const MAX_CANONICAL_SKILL_NAME_CHARS: usize = 64;

/// Configure the single user-owned skill source root before skills are loaded
/// from the database. Reconfiguration to a different path is rejected so
/// runtime paths cannot drift after prompts have been rendered.
pub fn configure_user_skills_directory(user_skills_dir: &Path) -> Result<(), CoreError> {
    if let Some(configured) = USER_SKILLS_DIR.get() {
        if configured == user_skills_dir {
            return Ok(());
        }
        return Err(CoreError::Conflict(format!(
            "User skill directory is already configured as {}",
            configured.display()
        )));
    }
    USER_SKILLS_DIR
        .set(user_skills_dir.to_path_buf())
        .map_err(|_| CoreError::Conflict("User skill directory was configured concurrently".into()))
}

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
/// onto disk under `<app_data_dir>/runtimes/builtin-skills/<slug>/`. Idempotent: skips files
/// whose on-disk content already matches the embedded content. Per-file
/// failures are logged but do not abort other skills.
///
/// Returns the base `<app_data_dir>/runtimes/builtin-skills/` path on success. The base path is
/// also stored in a process-global `OnceLock` so [`load_builtin_skills`] can
/// substitute `<SKILL_DIR>` placeholders in skill bodies with real paths.
pub fn materialize_skills_to_disk(app_data_dir: &Path) -> Result<PathBuf, CoreError> {
    let base = app_data_dir.join("runtimes").join("builtin-skills");
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

    cleanup_legacy_builtin_skill_cache(app_data_dir);

    // Record the base dir so skill-body rendering can substitute <SKILL_DIR>.
    // `OnceLock::set` returns Err if already set — that's fine; first call wins.
    let _ = SKILLS_BASE_DIR.set(base.clone());
    Ok(base)
}

fn cleanup_legacy_builtin_skill_cache(app_data_dir: &Path) {
    let legacy_base = app_data_dir.join("skills");
    for bundle in builtin_skill_bundles() {
        let legacy_skill_dir = legacy_base.join(bundle.slug);
        let mut owned_files = vec![(PathBuf::from("SKILL.md"), bundle.skill_md.as_bytes())];
        owned_files.extend(
            bundle
                .resources
                .iter()
                .map(|resource| (PathBuf::from(resource.path), resource.content.as_bytes())),
        );
        for (relative, expected) in owned_files {
            let path = legacy_skill_dir.join(relative);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = %error, "Could not inspect legacy built-in skill cache");
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                tracing::warn!(path = %path.display(), "Preserved non-regular legacy built-in skill path");
                continue;
            }
            if !fs::read(&path).is_ok_and(|bytes| bytes == expected) {
                tracing::warn!(path = %path.display(), "Preserved modified legacy built-in skill cache file");
                continue;
            }
            if let Err(error) = fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), error = %error, "Could not remove verified legacy built-in skill cache file");
                continue;
            }
            remove_empty_resource_parents(&legacy_skill_dir, path.parent());
        }
        let _ = fs::remove_dir(&legacy_skill_dir);
    }
    let _ = fs::remove_dir(&legacy_base);
}

/// Return the on-disk directory where a bundled skill is materialized.
///
/// This is intentionally path-only: callers that need guaranteed files should
/// call [`materialize_skills_to_disk`] first.
pub fn builtin_skill_dir(app_data_dir: &Path, slug: &str) -> PathBuf {
    app_data_dir
        .join("runtimes")
        .join("builtin-skills")
        .join(slug)
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

pub fn validate_canonical_skill_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    let valid = !name.is_empty()
        && name.len() <= MAX_CANONICAL_SKILL_NAME_CHARS
        && name.is_ascii()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(CoreError::InvalidInput(format!(
            "Skill canonical name must be 1-{MAX_CANONICAL_SKILL_NAME_CHARS} lowercase ASCII letters, digits, or single hyphens"
        )));
    }
    Ok(name.to_string())
}

pub fn derive_canonical_skill_name(display_name: &str) -> Result<String, CoreError> {
    let mut canonical = String::new();
    let mut pending_separator = false;
    for character in display_name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !canonical.is_empty() {
                canonical.push('-');
            }
            pending_separator = false;
            canonical.push(character.to_ascii_lowercase());
        } else if !canonical.is_empty() {
            pending_separator = true;
        }
        if canonical.len() >= MAX_CANONICAL_SKILL_NAME_CHARS {
            break;
        }
    }
    canonical.truncate(MAX_CANONICAL_SKILL_NAME_CHARS);
    while canonical.ends_with('-') {
        canonical.pop();
    }
    validate_canonical_skill_name(&canonical).map_err(|_| {
        CoreError::InvalidInput(
            "Skill display name cannot produce a portable canonical name; install a package whose SKILL.md name uses lowercase ASCII kebab-case"
                .into(),
        )
    })
}

fn canonical_skill_dir_name(skill: &Skill) -> Result<String, CoreError> {
    validate_canonical_skill_name(&skill.canonical_name)
}

/// Return the on-disk directory where a user-created skill should be
/// materialized. User skills are stored separately from built-ins so user
/// scripts/assets cannot collide with bundled skill resources.
pub fn user_skill_dir(app_data_dir: &Path, canonical_name: &str) -> PathBuf {
    app_data_dir
        .join("skills")
        .join("user")
        .join(canonical_name)
}

fn substitute_user_skill_dir(body: String, canonical_name: &str) -> String {
    if !body.contains("<SKILL_DIR>") {
        return body;
    }
    if let Some(base) = USER_SKILLS_DIR.get() {
        let skill_dir = base.join(canonical_name);
        body.replace("<SKILL_DIR>", &skill_dir.to_string_lossy())
    } else if let Some(base) = SKILLS_BASE_DIR.get() {
        let skill_dir = base.join("user").join(canonical_name);
        body.replace("<SKILL_DIR>", &skill_dir.to_string_lossy())
    } else {
        body
    }
}

pub(crate) fn user_skill_source_path(canonical_name: &str) -> Option<String> {
    USER_SKILLS_DIR
        .get()
        .map(|base| {
            base.join(canonical_name)
                .join("SKILL.md")
                .to_string_lossy()
                .to_string()
        })
        .or_else(|| {
            SKILLS_BASE_DIR.get().map(|base| {
                base.join("user")
                    .join(canonical_name)
                    .join("SKILL.md")
                    .to_string_lossy()
                    .to_string()
            })
        })
}

pub(crate) fn portable_user_skill_content(
    content: &str,
    skill_id: &str,
    canonical_name: &str,
    preferred_user_skills_dir: Option<&Path>,
) -> String {
    let mut portable = content.to_string();
    let mut candidate_dirs = Vec::new();
    if let Some(base) = preferred_user_skills_dir {
        candidate_dirs.push(base.join(canonical_name));
        candidate_dirs.push(base.join(safe_skill_dir_name(skill_id)));
    }
    if let Some(base) = USER_SKILLS_DIR.get() {
        candidate_dirs.push(base.join(canonical_name));
        candidate_dirs.push(base.join(safe_skill_dir_name(skill_id)));
    }
    if let Some(base) = SKILLS_BASE_DIR.get() {
        candidate_dirs.push(base.join("user").join(canonical_name));
        candidate_dirs.push(base.join("user").join(safe_skill_dir_name(skill_id)));
    }
    candidate_dirs.sort();
    candidate_dirs.dedup();
    for directory in candidate_dirs {
        let native = directory.to_string_lossy();
        portable = replace_skill_directory_reference(&portable, native.as_ref());
        let slash_normalized = native.replace('\\', "/");
        if slash_normalized != native.as_ref() {
            portable = replace_skill_directory_reference(&portable, &slash_normalized);
        }
    }
    portable
}

fn replace_skill_directory_reference(content: &str, directory: &str) -> String {
    if directory.is_empty() {
        return content.to_string();
    }

    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    for (start, _) in content.match_indices(directory) {
        if start < cursor {
            continue;
        }
        let end = start + directory.len();
        let has_path_boundary = content[end..]
            .chars()
            .next()
            .is_none_or(|character| matches!(character, '/' | '\\'));
        if !has_path_boundary {
            continue;
        }
        output.push_str(&content[cursor..start]);
        output.push_str("<SKILL_DIR>");
        cursor = end;
    }
    if cursor == 0 {
        return content.to_string();
    }
    output.push_str(&content[cursor..]);
    output
}

pub fn materialize_user_skill_to_disk(
    app_data_dir: &Path,
    skill: &Skill,
) -> Result<PathBuf, CoreError> {
    materialize_user_skill(&app_data_dir.join("skills").join("user"), skill, true)
}

pub fn materialize_user_skill_to_directory(
    user_skills_dir: &Path,
    skill: &Skill,
) -> Result<PathBuf, CoreError> {
    materialize_user_skill(user_skills_dir, skill, false)
}

fn materialize_user_skill(
    user_skills_dir: &Path,
    skill: &Skill,
    prune_stale_projection_files: bool,
) -> Result<PathBuf, CoreError> {
    if skill.builtin {
        return Err(CoreError::InvalidInput(
            "Built-in skills are materialized by materialize_skills_to_disk".into(),
        ));
    }

    let canonical_name = canonical_skill_dir_name(skill)?;
    let skill_dir = user_skills_dir.join(&canonical_name);
    fs::create_dir_all(user_skills_dir).map_err(|e| {
        CoreError::Internal(format!(
            "Failed to create user skills dir {}: {e}",
            user_skills_dir.display()
        ))
    })?;
    migrate_legacy_skill_directory(user_skills_dir, skill, &skill_dir)?;
    for directory in [user_skills_dir, skill_dir.as_path()] {
        ensure_real_user_skill_directory(directory, &skill.id, prune_stale_projection_files)?;
    }
    let mut expected_files = BTreeSet::from([PathBuf::from("SKILL.md")]);
    let mut portable_skill = skill.clone();
    portable_skill.content = portable_user_skill_content(
        &skill.content,
        &skill.id,
        &skill.canonical_name,
        Some(user_skills_dir),
    );
    write_user_file_if_changed(
        &skill_dir.join("SKILL.md"),
        export_skill_to_md(&portable_skill).as_bytes(),
        &skill.id,
        prune_stale_projection_files,
    )?;

    let resources = normalize_resource_bundle(&skill.resource_bundle)?;
    for resource in resources {
        expected_files.insert(PathBuf::from(&resource.path));
        let target = skill_dir.join(&resource.path);
        if let Some(parent) = target.parent() {
            ensure_user_skill_resource_parent(
                &skill_dir,
                parent,
                &skill.id,
                prune_stale_projection_files,
            )?;
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
        write_user_file_if_changed(&target, &bytes, &skill.id, prune_stale_projection_files)?;
    }

    if prune_stale_projection_files {
        prune_stale_user_skill_files(&skill_dir, &skill_dir, &expected_files, &skill.id)?;
    }

    Ok(skill_dir)
}

fn migrate_legacy_skill_directory(
    user_skills_dir: &Path,
    skill: &Skill,
    canonical_dir: &Path,
) -> Result<(), CoreError> {
    let legacy_dir = user_skills_dir.join(safe_skill_dir_name(&skill.id));
    if legacy_dir == canonical_dir {
        return Ok(());
    }
    let legacy_metadata = match fs::symlink_metadata(&legacy_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if legacy_metadata.file_type().is_symlink() || !legacy_metadata.is_dir() {
        return Err(CoreError::Conflict(format!(
            "Legacy skill path is not a real directory and was preserved: {}",
            legacy_dir.display()
        )));
    }
    if fs::symlink_metadata(canonical_dir).is_ok() {
        return Err(CoreError::Conflict(format!(
            "Cannot migrate skill {} because canonical directory already exists: {}",
            skill.id,
            canonical_dir.display()
        )));
    }
    fs::rename(&legacy_dir, canonical_dir).map_err(|error| {
        CoreError::Internal(format!(
            "Failed to migrate skill {} from {} to {}: {error}",
            skill.id,
            legacy_dir.display(),
            canonical_dir.display()
        ))
    })
}

pub fn materialize_user_skill_to_configured_directory(
    skill: &Skill,
) -> Result<Option<PathBuf>, CoreError> {
    USER_SKILLS_DIR
        .get()
        .map(|directory| materialize_user_skill_to_directory(directory, skill))
        .transpose()
}

pub fn remove_obsolete_user_skill_resources_from_directory(
    user_skills_dir: &Path,
    previous: &Skill,
    next: &Skill,
) -> Result<(), CoreError> {
    if previous.id != next.id {
        return Err(CoreError::InvalidInput(
            "Cannot reconcile resources across different skill identities".into(),
        ));
    }
    let next_paths = normalize_resource_bundle(&next.resource_bundle)?
        .into_iter()
        .map(|resource| resource.path)
        .collect::<HashSet<_>>();
    let skill_dir = user_skills_dir.join(canonical_skill_dir_name(next)?);
    for resource in normalize_resource_bundle(&previous.resource_bundle)? {
        if next_paths.contains(&resource.path) {
            continue;
        }
        let target = skill_dir.join(&resource.path);
        let removed_file = match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fs::remove_file(&target)
                    .or_else(|_| fs::remove_dir(&target))
                    .map_err(|error| {
                        CoreError::Internal(format!(
                            "Failed to remove obsolete skill resource {}: {error}",
                            target.display()
                        ))
                    })?;
                true
            }
            Ok(metadata) if metadata.is_file() => {
                fs::remove_file(&target).map_err(|error| {
                    CoreError::Internal(format!(
                        "Failed to remove obsolete skill resource {}: {error}",
                        target.display()
                    ))
                })?;
                true
            }
            Ok(_) => {
                // Never recursively delete a directory in the user-owned
                // source tree; it may contain files Nexa never modeled.
                false
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if removed_file {
            remove_empty_resource_parents(&skill_dir, target.parent());
        }
    }
    Ok(())
}

fn remove_empty_resource_parents(skill_dir: &Path, mut parent: Option<&Path>) {
    while let Some(directory) = parent {
        if directory == skill_dir || !directory.starts_with(skill_dir) {
            break;
        }
        let next = directory.parent();
        let removable = fs::symlink_metadata(directory)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
        if !removable || fs::remove_dir(directory).is_err() {
            break;
        }
        parent = next;
    }
}

pub fn remove_obsolete_user_skill_resources_from_configured_directory(
    previous: &Skill,
    next: &Skill,
) -> Result<bool, CoreError> {
    let Some(directory) = USER_SKILLS_DIR.get() else {
        return Ok(false);
    };
    remove_obsolete_user_skill_resources_from_directory(directory, previous, next)?;
    Ok(true)
}

fn ensure_real_user_skill_directory(
    path: &Path,
    skill_id: &str,
    replace_conflicts: bool,
) -> Result<(), CoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if !replace_conflicts {
                return Err(CoreError::Conflict(format!(
                    "User-owned skill directory path is a link: {}",
                    path.display()
                )));
            }
            fs::remove_file(path)
                .or_else(|_| fs::remove_dir(path))
                .map_err(|e| {
                    CoreError::Internal(format!(
                        "Failed to remove user skill directory symlink {} for {skill_id}: {e}",
                        path.display()
                    ))
                })?;
            fs::create_dir(path).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to replace user skill directory {} for {skill_id}: {e}",
                    path.display()
                ))
            })
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => {
            if !replace_conflicts {
                return Err(CoreError::Conflict(format!(
                    "User-owned skill directory path is occupied: {}",
                    path.display()
                )));
            }
            fs::remove_file(path).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to replace user skill directory path {} for {skill_id}: {e}",
                    path.display()
                ))
            })?;
            fs::create_dir(path).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to create user skill directory {} for {skill_id}: {e}",
                    path.display()
                ))
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to create user skill directory {} for {skill_id}: {e}",
                    path.display()
                ))
            })
        }
        Err(error) => Err(CoreError::Internal(format!(
            "Failed to inspect user skill directory {} for {skill_id}: {error}",
            path.display()
        ))),
    }
}

fn ensure_user_skill_resource_parent(
    skill_dir: &Path,
    parent: &Path,
    skill_id: &str,
    replace_conflicts: bool,
) -> Result<(), CoreError> {
    let relative = parent.strip_prefix(skill_dir).map_err(|error| {
        CoreError::InvalidInput(format!(
            "User skill resource escaped its materialization root: {error}"
        ))
    })?;
    let mut current = skill_dir.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(CoreError::InvalidInput(
                "User skill resource contains an invalid path component".to_string(),
            ));
        };
        current.push(component);
        ensure_real_user_skill_directory(&current, skill_id, replace_conflicts)?;
    }
    Ok(())
}

fn write_user_file_if_changed(
    path: &Path,
    bytes: &[u8],
    skill_id: &str,
    replace_conflicting_directories: bool,
) -> Result<(), CoreError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            fs::remove_file(path)
                .or_else(|_| fs::remove_dir(path))
                .map_err(|e| {
                    CoreError::Internal(format!(
                        "Failed to replace user skill symlink {} for {skill_id}: {e}",
                        path.display()
                    ))
                })?;
        } else if metadata.is_dir() {
            if replace_conflicting_directories {
                fs::remove_dir_all(path).map_err(|e| {
                    CoreError::Internal(format!(
                        "Failed to replace user skill resource directory {} for {skill_id}: {e}",
                        path.display()
                    ))
                })?;
            } else {
                fs::remove_dir(path).map_err(|e| {
                    CoreError::Conflict(format!(
                        "Cannot replace non-empty user-owned skill directory {} for {skill_id}: {e}",
                        path.display()
                    ))
                })?;
            }
        }
    }
    if fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    fs::write(path, bytes).map_err(|e| {
        CoreError::Internal(format!(
            "Failed to write user skill file {} for {skill_id}: {e}",
            path.display()
        ))
    })
}

fn prune_stale_user_skill_files(
    root: &Path,
    dir: &Path,
    expected_files: &BTreeSet<PathBuf>,
    skill_id: &str,
) -> Result<(), CoreError> {
    for entry in fs::read_dir(dir).map_err(|e| {
        CoreError::Internal(format!(
            "Failed to inspect user skill dir {} for {skill_id}: {e}",
            dir.display()
        ))
    })? {
        let entry = entry.map_err(|e| {
            CoreError::Internal(format!("Failed to inspect user skill {skill_id}: {e}"))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| {
            CoreError::Internal(format!(
                "Failed to inspect user skill path {}: {e}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            prune_stale_user_skill_files(root, &path, expected_files, skill_id)?;
            if fs::read_dir(&path)
                .map_err(|e| CoreError::Internal(e.to_string()))?
                .next()
                .is_none()
            {
                fs::remove_dir(&path).map_err(|e| {
                    CoreError::Internal(format!(
                        "Failed to remove stale user skill dir {}: {e}",
                        path.display()
                    ))
                })?;
            }
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|e| CoreError::Internal(format!("Failed to resolve user skill path: {e}")))?;
        if !expected_files.contains(relative) {
            fs::remove_file(&path).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to remove stale user skill file {}: {e}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
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
            remove_materialized_user_skill(app_data_dir, skill)?;
        }
    }
    Ok(())
}

pub fn materialize_user_skills_to_directory(
    user_skills_dir: &Path,
    skills: &[Skill],
) -> Result<(), CoreError> {
    materialize_user_skills_to_directory_except(user_skills_dir, skills, &[])
}

pub fn materialize_user_skills_to_directory_except(
    user_skills_dir: &Path,
    skills: &[Skill],
    preserved_skill_ids: &[String],
) -> Result<(), CoreError> {
    let preserved = preserved_skill_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    for skill in skills {
        if skill.builtin || preserved.contains(skill.id.as_str()) {
            continue;
        }
        // This directory is user-owned source, not an enabled-runtime cache.
        // Disabled skills remain editable and can be enabled again without
        // reconstructing or losing their files.
        materialize_user_skill_to_directory(user_skills_dir, skill)?;
    }
    Ok(())
}

pub fn remove_materialized_user_skill(app_data_dir: &Path, skill: &Skill) -> Result<(), CoreError> {
    let root = app_data_dir.join("skills").join("user");
    for skill_dir in [
        root.join(canonical_skill_dir_name(skill)?),
        root.join(safe_skill_dir_name(&skill.id)),
    ] {
        if skill_dir.exists() {
            fs::remove_dir_all(&skill_dir).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to remove user skill dir {}: {e}",
                    skill_dir.display()
                ))
            })?;
        }
    }
    Ok(())
}

pub fn remove_materialized_user_skill_from_directory(
    user_skills_dir: &Path,
    skill: &Skill,
) -> Result<(), CoreError> {
    for skill_dir in [
        user_skills_dir.join(canonical_skill_dir_name(skill)?),
        user_skills_dir.join(safe_skill_dir_name(&skill.id)),
    ] {
        remove_materialized_user_skill_path(&skill_dir)?;
    }
    Ok(())
}

fn remove_materialized_user_skill_path(skill_dir: &Path) -> Result<(), CoreError> {
    match fs::symlink_metadata(skill_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::remove_file(skill_dir)
                .or_else(|_| fs::remove_dir(skill_dir))
                .map_err(|e| {
                    CoreError::Internal(format!(
                        "Failed to remove user skill link {}: {e}",
                        skill_dir.display()
                    ))
                })?;
        }
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(skill_dir).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to remove user skill dir {}: {e}",
                    skill_dir.display()
                ))
            })?;
        }
        Ok(_) => {
            fs::remove_file(skill_dir).map_err(|e| {
                CoreError::Internal(format!(
                    "Failed to remove user skill path {}: {e}",
                    skill_dir.display()
                ))
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
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
    let canonical_name: String = row.get(8)?;
    let resource_bundle = deserialize_resource_bundle(resource_bundle_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let (interface, dependencies, policy) =
        derive_skill_metadata(&name, &description, &resource_bundle);
    Ok(Skill {
        id: id.clone(),
        canonical_name: canonical_name.clone(),
        name,
        description,
        content: substitute_user_skill_dir(content, &canonical_name),
        enabled: row.get::<_, i32>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        builtin: false,
        interface,
        dependencies,
        policy,
        source_path: user_skill_source_path(&canonical_name),
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

fn ensure_skill_canonical_names(conn: &rusqlite::Connection) -> Result<(), CoreError> {
    let mut stmt = conn
        .prepare("SELECT id, name, canonical_name FROM skills ORDER BY created_at ASC, id ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut skills = Vec::new();
    for row in rows {
        skills.push(row?);
    }
    drop(stmt);

    let mut used = builtin_skill_bundles()
        .iter()
        .map(|bundle| {
            (
                bundle.slug.to_ascii_lowercase(),
                format!("built-in skill `{}`", bundle.slug),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    for (id, _, _) in &skills {
        if let Some(owner) = used.insert(id.to_ascii_lowercase(), format!("database ID `{id}`")) {
            return Err(CoreError::Conflict(format!(
                "Skill database ID `{id}` conflicts with {owner}"
            )));
        }
    }
    let mut updates = Vec::new();
    for (id, display_name, stored) in skills {
        let needs_backfill = stored
            .as_deref()
            .is_none_or(|value| value.trim().is_empty());
        let mut canonical_name = match stored.as_deref().filter(|value| !value.trim().is_empty()) {
            Some(stored) => validate_canonical_skill_name(stored)?,
            None => derive_canonical_skill_name(&display_name)
                .unwrap_or_else(|_| migration_canonical_skill_name(&display_name, &id)),
        };
        let mut key = canonical_name.to_ascii_lowercase();
        if needs_backfill && used.contains_key(&key) {
            canonical_name = migration_canonical_skill_name(&display_name, &id);
            key = canonical_name.to_ascii_lowercase();
        }
        if let Some(owner) = used.insert(key, format!("skill `{id}`")) {
            return Err(CoreError::Conflict(format!(
                "Skill `{id}` canonical name `{canonical_name}` conflicts with {owner}; rename it before migration"
            )));
        }
        if needs_backfill {
            updates.push((id.clone(), canonical_name));
        }
    }
    for (id, canonical_name) in updates {
        conn.execute(
            "UPDATE skills SET canonical_name = ?2 WHERE id = ?1 AND (canonical_name IS NULL OR canonical_name = '')",
            rusqlite::params![id, canonical_name],
        )?;
    }
    Ok(())
}

/// Older database rows stored only a display name. Agent Skills requires the
/// portable directory/frontmatter name to be lowercase ASCII kebab-case, so a
/// fully localized display name cannot be copied verbatim. Preserve a readable
/// encoding of its Unicode code points and add a short immutable-id digest so
/// same-named legacy skills migrate independently instead of blocking all skill
/// reads.
fn migration_canonical_skill_name(display_name: &str, id: &str) -> String {
    let digest = blake3::hash(id.as_bytes()).to_hex();
    let suffix = &digest.as_str()[..8];
    let suffix_chars = suffix.len() + 1;
    let base_limit = MAX_CANONICAL_SKILL_NAME_CHARS - suffix_chars;
    let mut base = String::from("skill");
    for character in display_name
        .chars()
        .filter(|character| !character.is_whitespace())
    {
        let segment = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase().to_string()
        } else {
            format!("u{:x}", character as u32)
        };
        if base.len() + 1 + segment.len() > base_limit {
            break;
        }
        base.push('-');
        base.push_str(&segment);
    }
    format!("{base}-{suffix}")
}

impl Database {
    /// List all user skills, newest first. Built-ins live in the static registry,
    /// while historical database rows were removed by migration v048.
    pub fn list_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        ensure_skill_canonical_names(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, content, enabled, created_at, updated_at, resource_bundle_json, canonical_name
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
        let mut input = normalize_skill_input(input)?;
        let conn = self.conn();
        ensure_skill_canonical_names(&conn)?;
        let resource_bundle_json = serialize_resource_bundle(&input.resource_bundle)?;
        let id = match &input.id {
            Some(existing_id) => {
                let installed_canonical: String = conn
                    .query_row(
                        "SELECT canonical_name FROM skills WHERE id = ?1",
                        rusqlite::params![existing_id],
                        |row| row.get(0),
                    )
                    .map_err(|_| CoreError::NotFound(format!("Skill {existing_id}")))?;
                input.content = portable_user_skill_content(
                    &input.content,
                    existing_id,
                    &installed_canonical,
                    None,
                );
                conn.execute(
                    "UPDATE skills
                     SET name = ?2, description = ?3, content = ?4, enabled = ?5,
                         resource_bundle_json = ?6, canonical_name = ?7,
                         updated_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![
                        existing_id,
                        &input.name,
                        &input.description,
                        &input.content,
                        input.enabled as i32,
                        &resource_bundle_json,
                        &installed_canonical,
                    ],
                )?;
                existing_id.clone()
            }
            None => {
                let new_id = loop {
                    let candidate = Uuid::new_v4().to_string();
                    let reserved: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM skills WHERE id = ?1 OR canonical_name = ?1 COLLATE NOCASE)",
                        rusqlite::params![&candidate],
                        |row| row.get(0),
                    )?;
                    if !reserved {
                        break candidate;
                    }
                };
                let canonical_name = derive_canonical_skill_name(&input.name)
                    .unwrap_or_else(|_| migration_canonical_skill_name(&input.name, &new_id));
                if builtin_skill_bundles()
                    .iter()
                    .any(|bundle| bundle.slug.eq_ignore_ascii_case(&canonical_name))
                {
                    return Err(CoreError::Conflict(format!(
                        "Skill canonical name `{canonical_name}` is reserved by a built-in skill"
                    )));
                }
                let conflict: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM skills WHERE canonical_name = ?1 COLLATE NOCASE OR id = ?1)",
                    rusqlite::params![&canonical_name],
                    |row| row.get(0),
                )?;
                if conflict {
                    return Err(CoreError::Conflict(format!(
                        "Skill canonical name `{canonical_name}` is reserved by an installed skill identity"
                    )));
                }
                input.content =
                    portable_user_skill_content(&input.content, &new_id, &canonical_name, None);
                conn.execute(
                    "INSERT INTO skills (id, canonical_name, name, description, content, enabled, resource_bundle_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        &new_id,
                        &canonical_name,
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
        ensure_skill_canonical_names(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, content, enabled, created_at, updated_at, resource_bundle_json, canonical_name
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
        ensure_skill_canonical_names(&conn)?;
        conn.query_row(
            "SELECT id, name, description, content, enabled, created_at, updated_at, resource_bundle_json, canonical_name
             FROM skills
             WHERE id = ?1",
            rusqlite::params![id],
            skill_from_row,
        )
        .map_err(|_| CoreError::NotFound(format!("Skill {id}")))
    }
}
