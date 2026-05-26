//! Skills and workflows as first-class capability package entries.

use crate::capability_package::{
    package_entry, CapabilityComponentKind, CapabilityPackageEntry, CapabilityPackageManifest,
    CapabilityPackagePermissions,
};
use crate::ecosystem::EcosystemSurfaceKind;
use crate::workflow_catalog::WORKFLOW_TEMPLATES;

use super::registry::builtin_skill_bundles;

pub const BUILTIN_SKILLS_PACKAGE_ID: &str = "builtin-skills";
pub const BUILTIN_WORKFLOWS_PACKAGE_ID: &str = "builtin-workflows";
pub const SKILL_ENTRY_FILE: &str = "SKILL.md";
pub const WORKFLOW_ENTRY_FILE: &str = "workflow.yaml";

pub fn builtin_skill_package_manifest() -> CapabilityPackageManifest {
    CapabilityPackageManifest {
        id: BUILTIN_SKILLS_PACKAGE_ID.to_string(),
        name: "Built-in Skills".to_string(),
        surface: EcosystemSurfaceKind::SkillPackage,
        description:
            "Bundled skill packages with reusable instructions, references, scripts, and assets."
                .to_string(),
        version: 1,
        tools: Vec::new(),
        skills: builtin_skill_bundles()
            .iter()
            .map(|bundle| bundle.slug.to_string())
            .collect(),
        workflows: Vec::new(),
        settings_surfaces: vec!["extensions".to_string()],
        runtime_checks: Vec::new(),
        permissions: CapabilityPackagePermissions::default(),
    }
}

pub fn builtin_workflow_package_manifest() -> CapabilityPackageManifest {
    CapabilityPackageManifest {
        id: BUILTIN_WORKFLOWS_PACKAGE_ID.to_string(),
        name: "Built-in Workflows".to_string(),
        surface: EcosystemSurfaceKind::WorkflowPackage,
        description:
            "Bundled user-facing task templates that compose tools, skills, and agent roles."
                .to_string(),
        version: 1,
        tools: Vec::new(),
        skills: Vec::new(),
        workflows: WORKFLOW_TEMPLATES
            .iter()
            .map(|template| template.id.to_string())
            .collect(),
        settings_surfaces: vec!["workflows".to_string()],
        runtime_checks: Vec::new(),
        permissions: CapabilityPackagePermissions::default(),
    }
}

pub fn builtin_skill_package_entries() -> Vec<CapabilityPackageEntry> {
    builtin_skill_bundles()
        .iter()
        .map(|bundle| {
            package_entry(
                BUILTIN_SKILLS_PACKAGE_ID,
                CapabilityComponentKind::Skill,
                bundle.slug,
                &format!("{}/{}", bundle.slug, SKILL_ENTRY_FILE),
                true,
            )
        })
        .collect()
}

pub fn builtin_workflow_package_entries() -> Vec<CapabilityPackageEntry> {
    WORKFLOW_TEMPLATES
        .iter()
        .map(|template| {
            package_entry(
                BUILTIN_WORKFLOWS_PACKAGE_ID,
                CapabilityComponentKind::Workflow,
                template.id,
                &format!("{}/{}", template.id, WORKFLOW_ENTRY_FILE),
                true,
            )
        })
        .collect()
}

pub fn builtin_capability_package_entries() -> Vec<CapabilityPackageEntry> {
    let mut entries = builtin_skill_package_entries();
    entries.extend(builtin_workflow_package_entries());
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_skills_are_capability_package_entries() {
        let entries = builtin_skill_package_entries();

        assert_eq!(entries.len(), builtin_skill_bundles().len());
        assert!(entries.iter().any(|entry| {
            entry.component_id == "pptx-presentation-design"
                && entry.kind == CapabilityComponentKind::Skill
                && entry.path
                    == ".nexa/capabilities/builtin-skills/skills/pptx-presentation-design/SKILL.md"
        }));
    }

    #[test]
    fn builtin_workflows_use_the_same_capability_package_layout() {
        let entries = builtin_workflow_package_entries();

        assert_eq!(entries.len(), WORKFLOW_TEMPLATES.len());
        assert!(entries.iter().all(|entry| {
            entry.kind == CapabilityComponentKind::Workflow
                && entry
                    .path
                    .starts_with(".nexa/capabilities/builtin-workflows/workflows/")
                && entry.path.ends_with("/workflow.yaml")
        }));
    }

    #[test]
    fn builtin_skills_have_skill_package_manifest() {
        let manifest = builtin_skill_package_manifest();

        assert_eq!(manifest.id, BUILTIN_SKILLS_PACKAGE_ID);
        assert_eq!(
            manifest.surface,
            crate::ecosystem::EcosystemSurfaceKind::SkillPackage
        );
        assert_eq!(manifest.skills.len(), builtin_skill_bundles().len());
        crate::capability_package::validate_capability_manifest(&manifest)
            .expect("builtin skill package manifest should validate");
    }

    #[test]
    fn builtin_workflows_have_workflow_package_manifest() {
        let manifest = builtin_workflow_package_manifest();

        assert_eq!(manifest.id, BUILTIN_WORKFLOWS_PACKAGE_ID);
        assert_eq!(
            manifest.surface,
            crate::ecosystem::EcosystemSurfaceKind::WorkflowPackage
        );
        assert_eq!(manifest.workflows.len(), WORKFLOW_TEMPLATES.len());
        crate::capability_package::validate_capability_manifest(&manifest)
            .expect("builtin workflow package manifest should validate");
    }
}
