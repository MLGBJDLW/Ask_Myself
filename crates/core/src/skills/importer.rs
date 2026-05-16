use std::fs;
use std::path::{Path, PathBuf};

use crate::db::Database;
use crate::error::CoreError;
use walkdir::WalkDir;

use super::model::{
    DiscoveredSkillBundle, SaveSkillInput, Skill, SkillResourceEncoding, SkillResourceFile,
};
use super::registry::parse_skill_file;
use super::scanner::scan_skill_content;
use super::storage::{
    normalize_resource_bundle, resource_bundle_metadata, resource_kind_from_relative_path,
};

pub fn discover_skills_in_directory(root: &Path) -> Result<Vec<DiscoveredSkillBundle>, CoreError> {
    if !root.exists() {
        return Err(CoreError::NotFound(format!(
            "Skill directory not found: {}",
            root.display()
        )));
    }

    let mut discovered = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
    {
        let skill_file = entry.into_path();
        let skill_dir = skill_file.parent().unwrap_or(root).to_path_buf();
        let content = fs::read_to_string(&skill_file)?;
        let (frontmatter, _) = parse_skill_file(&content)?;
        let resources = load_resource_bundle_from_dir(&skill_dir)?;
        discovered.push(DiscoveredSkillBundle {
            skill_file: skill_file.to_string_lossy().to_string(),
            skill_dir: skill_dir.to_string_lossy().to_string(),
            name: frontmatter.name,
            description: frontmatter.description,
            resources: resource_bundle_metadata(&resources),
            warnings: scan_skill_content(&content),
        });
    }
    discovered.sort_by(|a, b| a.skill_file.cmp(&b.skill_file));
    Ok(discovered)
}

pub fn import_skills_from_directory(db: &Database, root: &Path) -> Result<Vec<Skill>, CoreError> {
    let discovered = discover_skills_in_directory(root)?;
    let mut imported = Vec::with_capacity(discovered.len());
    for bundle in discovered {
        let skill_file = PathBuf::from(&bundle.skill_file);
        let skill_dir = PathBuf::from(&bundle.skill_dir);
        let content = fs::read_to_string(&skill_file)?;
        let (frontmatter, body) = parse_skill_file(&content)?;
        let input = SaveSkillInput {
            id: None,
            name: frontmatter.name,
            description: frontmatter.description,
            content: body,
            enabled: true,
            resource_bundle: load_resource_bundle_from_dir(&skill_dir)?,
        };
        imported.push(db.save_skill(&input)?);
    }
    Ok(imported)
}

fn load_resource_bundle_from_dir(skill_dir: &Path) -> Result<Vec<SkillResourceFile>, CoreError> {
    let mut resources = Vec::new();
    for folder in ["scripts", "references", "assets"] {
        let dir = skill_dir.join(folder);
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.into_path();
            let relative = path
                .strip_prefix(skill_dir)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path)?;
            let (encoding, content) = match String::from_utf8(bytes.clone()) {
                Ok(text) => (SkillResourceEncoding::Utf8, text),
                Err(_) => (SkillResourceEncoding::Base64, {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                }),
            };
            resources.push(SkillResourceFile {
                path: relative.clone(),
                kind: resource_kind_from_relative_path(&relative),
                encoding,
                content,
            });
        }
    }
    normalize_resource_bundle(&resources)
}
