use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::models::Source;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathKind {
    Any,
    File,
    Directory,
}

pub(crate) fn has_path_traversal(path: &str) -> bool {
    path.contains('\0')
        || Path::new(path)
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn canonicalize_with_optional_missing(path: &Path, allow_missing: bool) -> Result<PathBuf, String> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Ok(canonical);
    }

    if !allow_missing {
        return Err(format!("Cannot resolve path '{}'.", path.display()));
    }

    let mut suffix_parts: Vec<OsString> = Vec::new();
    let mut ancestor = path.to_path_buf();

    loop {
        if ancestor.exists() {
            let mut rebuilt = std::fs::canonicalize(&ancestor)
                .map_err(|e| format!("Cannot resolve path '{}': {e}", path.display()))?;
            for part in suffix_parts.into_iter().rev() {
                rebuilt = rebuilt.join(part);
            }
            return Ok(rebuilt);
        }

        let file_name = ancestor
            .file_name()
            .ok_or_else(|| format!("Invalid path '{}'.", path.display()))?;
        suffix_parts.push(file_name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| format!("Cannot resolve path '{}'.", path.display()))?
            .to_path_buf();
    }
}

fn validate_kind(
    path: &Path,
    requested: &Path,
    kind: PathKind,
    allow_missing: bool,
) -> Result<(), String> {
    if !path.exists() {
        if allow_missing {
            return Ok(());
        }

        return Err(match kind {
            PathKind::Any => format!("Path not found: '{}'", requested.display()),
            PathKind::File => format!("File not found: '{}'", requested.display()),
            PathKind::Directory => format!("Directory not found: '{}'", requested.display()),
        });
    }

    match kind {
        PathKind::Any => Ok(()),
        PathKind::File if !path.is_file() => {
            Err(format!("'{}' is not a file.", requested.display()))
        }
        PathKind::Directory if !path.is_dir() => {
            Err(format!("'{}' is not a directory.", requested.display()))
        }
        _ => Ok(()),
    }
}

pub(crate) fn resolve_path_from_base_in_sources(
    requested: &Path,
    base: &Path,
    sources: &[Source],
    kind: PathKind,
    allow_missing: bool,
) -> Result<PathBuf, String> {
    if requested.as_os_str().is_empty() {
        return Err("Path must not be empty.".to_string());
    }

    if requested.to_string_lossy().contains('\0') {
        return Err("Path must not contain null bytes.".to_string());
    }

    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    let resolved = canonicalize_with_optional_missing(&candidate, allow_missing)?;
    let in_scope = sources.iter().any(|source| {
        std::fs::canonicalize(Path::new(&source.root_path))
            .map(|root| resolved.starts_with(&root))
            .unwrap_or(false)
    });

    if !in_scope {
        return Err(format!(
            "Access denied: '{}' is not within any registered source directory.",
            requested.display()
        ));
    }

    validate_kind(&resolved, requested, kind, allow_missing)?;
    Ok(resolved)
}

fn collect_matching_source_paths(
    requested: &Path,
    sources: &[Source],
    kind: PathKind,
    allow_missing: bool,
) -> Vec<(String, PathBuf, PathBuf)> {
    let mut matches = Vec::new();

    for source in sources {
        let Ok(root) = std::fs::canonicalize(Path::new(&source.root_path)) else {
            continue;
        };

        let candidate = root.join(requested);
        let Ok(resolved) = canonicalize_with_optional_missing(&candidate, allow_missing) else {
            continue;
        };

        if !resolved.starts_with(&root) {
            continue;
        }

        if validate_kind(&resolved, requested, kind, allow_missing).is_ok()
            && !matches.iter().any(|(_, _, existing)| existing == &resolved)
        {
            matches.push((source.id.clone(), root, resolved));
        }
    }

    matches
}

pub(crate) fn resolve_path_in_sources(
    requested: &Path,
    sources: &[Source],
    kind: PathKind,
    allow_missing: bool,
) -> Result<PathBuf, String> {
    if requested.as_os_str().is_empty() {
        return Err("Path must not be empty.".to_string());
    }

    if let Ok(resolved) = canonicalize_with_optional_missing(requested, allow_missing) {
        let in_scope = sources.iter().any(|source| {
            std::fs::canonicalize(Path::new(&source.root_path))
                .map(|root| resolved.starts_with(&root))
                .unwrap_or(false)
        });
        if in_scope {
            validate_kind(&resolved, requested, kind, allow_missing)?;
            return Ok(resolved);
        }
        if requested.is_absolute() {
            return Err(format!(
                "Access denied: '{}' is not within any registered source directory.",
                requested.display()
            ));
        }
    }

    if !requested.is_absolute() && has_path_traversal(&requested.to_string_lossy()) {
        return Err("Path must not contain '..' traversal sequences.".to_string());
    }

    let matches = if requested.is_absolute() {
        Vec::new()
    } else {
        collect_matching_source_paths(requested, sources, kind, allow_missing)
    };

    match matches.len() {
        1 => Ok(matches[0].2.clone()),
        n if n > 1 => {
            let mut text = format!(
                "Path '{}' is ambiguous across multiple source directories. Use an absolute path or narrow the source scope.\nCandidates:",
                requested.display()
            );
            for (source_id, root, resolved) in matches {
                text.push_str(&format!(
                    "\n- source {} (root: {}): {}",
                    source_id,
                    root.display(),
                    resolved.display()
                ));
            }
            Err(text)
        }
        _ => Err(format!(
            "Cannot resolve path '{}' within any registered source directory.",
            requested.display()
        )),
    }
}

pub(crate) fn resolve_path_for_file_access(
    requested: &Path,
    sources: &[Source],
    kind: PathKind,
    allow_missing: bool,
    allow_unregistered_absolute_paths: bool,
) -> Result<PathBuf, String> {
    if requested.as_os_str().is_empty() {
        return Err("Path must not be empty.".to_string());
    }

    if allow_unregistered_absolute_paths && requested.is_absolute() {
        let resolved = canonicalize_with_optional_missing(requested, allow_missing)?;
        validate_kind(&resolved, requested, kind, allow_missing)?;
        return Ok(resolved);
    }

    resolve_path_in_sources(requested, sources, kind, allow_missing)
}

pub(crate) fn resolve_existing_file_in_sources(
    requested: &Path,
    sources: &[Source],
) -> Result<PathBuf, String> {
    resolve_path_in_sources(requested, sources, PathKind::File, false)
}

pub(crate) fn resolve_existing_directory_in_sources(
    requested: &Path,
    sources: &[Source],
) -> Result<PathBuf, String> {
    resolve_path_in_sources(requested, sources, PathKind::Directory, false)
}

pub(crate) fn resolve_existing_file_for_file_access(
    requested: &Path,
    sources: &[Source],
    allow_unregistered_absolute_paths: bool,
) -> Result<PathBuf, String> {
    resolve_path_for_file_access(
        requested,
        sources,
        PathKind::File,
        false,
        allow_unregistered_absolute_paths,
    )
}

pub(crate) fn resolve_existing_directory_for_file_access(
    requested: &Path,
    sources: &[Source],
    allow_unregistered_absolute_paths: bool,
) -> Result<PathBuf, String> {
    resolve_path_for_file_access(
        requested,
        sources,
        PathKind::Directory,
        false,
        allow_unregistered_absolute_paths,
    )
}

pub(crate) fn resolve_writable_file_for_file_access(
    requested: &Path,
    sources: &[Source],
    allow_unregistered_absolute_paths: bool,
) -> Result<PathBuf, String> {
    resolve_path_for_file_access(
        requested,
        sources,
        PathKind::File,
        true,
        allow_unregistered_absolute_paths,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, root: &Path) -> Source {
        Source {
            id: id.to_string(),
            kind: "local_folder".to_string(),
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn resolves_source_relative_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes").join("hello.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "hello").unwrap();

        let sources = vec![source("src-1", dir.path())];
        let resolved = resolve_existing_file_in_sources(Path::new("notes/hello.txt"), &sources)
            .expect("should resolve");

        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn resolves_source_relative_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let sources = vec![source("src-1", dir.path())];

        let resolved =
            resolve_writable_file_for_file_access(Path::new("drafts/report.md"), &sources, false)
                .expect("should resolve");

        assert_eq!(
            resolved,
            std::fs::canonicalize(dir.path())
                .unwrap()
                .join("drafts")
                .join("report.md")
        );
    }

    #[test]
    fn rejects_ambiguous_relative_path() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let rel = Path::new("shared").join("notes.txt");
        let left_file = left.path().join(&rel);
        let right_file = right.path().join(&rel);
        std::fs::create_dir_all(left_file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(right_file.parent().unwrap()).unwrap();
        std::fs::write(&left_file, "left").unwrap();
        std::fs::write(&right_file, "right").unwrap();

        let sources = vec![source("left", left.path()), source("right", right.path())];
        let err = resolve_existing_file_in_sources(&rel, &sources).unwrap_err();

        assert!(err.contains("ambiguous"), "err was: {err}");
        assert!(err.contains("source left"), "err was: {err}");
        assert!(err.contains("source right"), "err was: {err}");
    }

    #[test]
    fn resolves_relative_path_from_cwd_inside_source() {
        let dir = tempfile::tempdir().unwrap();
        let source_root = std::fs::canonicalize(dir.path()).unwrap();
        let cwd = source_root.join("nested");
        let file = source_root.join("hello.txt");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(&file, "hello").unwrap();

        let sources = vec![source("src-1", &source_root)];
        let resolved = resolve_path_from_base_in_sources(
            Path::new("../hello.txt"),
            &cwd,
            &sources,
            PathKind::File,
            false,
        )
        .expect("should resolve");

        assert_eq!(resolved, file);
    }

    #[test]
    fn rejects_relative_path_from_cwd_that_escapes_source() {
        let workspace = tempfile::tempdir().unwrap();
        let source_root = workspace.path().join("source");
        let cwd = source_root.join("nested");
        let escaped = workspace.path().join("outside.txt");
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(&escaped, "nope").unwrap();

        let sources = vec![source("src-1", &source_root)];
        let err = resolve_path_from_base_in_sources(
            Path::new("../../outside.txt"),
            &cwd,
            &sources,
            PathKind::File,
            false,
        )
        .unwrap_err();

        assert!(err.contains("Access denied"), "err was: {err}");
    }
}
