use serde_json::json;

use crate::office_runtime::{OfficeDependencyStatus, OfficeRuntimeReadiness};

use super::{
    PluginCheckSeverity, PluginManifest, PluginProviderCatalog, PluginRuntimeCheck,
    PluginRuntimeStatus, PluginSettingsField, PluginSettingsSchema,
};

pub(super) fn enrich_manifest(
    mut manifest: PluginManifest,
    readiness: Option<&OfficeRuntimeReadiness>,
) -> PluginManifest {
    manifest.settings_schema = Some(settings_schema());
    manifest.provider_catalogs = vec![document_format_catalog(), python_package_catalog()];
    manifest.runtime_checks = runtime_checks(readiness);
    manifest
}

fn settings_schema() -> PluginSettingsSchema {
    PluginSettingsSchema {
        config_key: "officeRuntime".to_string(),
        fields: vec![
            PluginSettingsField {
                key: "managedEnvironment".to_string(),
                label: "Managed Python environment".to_string(),
                kind: "readOnlyPath".to_string(),
                required: true,
                secret: false,
                description: "App-managed virtual environment used by prepare_document_tools."
                    .to_string(),
                options_source: None,
                default_value: Some(json!("runtimes/office-python")),
            },
            PluginSettingsField {
                key: "requirements".to_string(),
                label: "Python requirements".to_string(),
                kind: "readOnlyPath".to_string(),
                required: true,
                secret: false,
                description:
                    "Bundled requirements installed for DOCX, XLSX, PPTX, and PDF workflows."
                        .to_string(),
                options_source: Some("officeRuntime.requirementsPath".to_string()),
                default_value: None,
            },
            PluginSettingsField {
                key: "prepareAction".to_string(),
                label: "Prepare action".to_string(),
                kind: "workflow".to_string(),
                required: true,
                secret: false,
                description:
                    "Checks and prepares the local Python runtime behind the document tools."
                        .to_string(),
                options_source: Some("prepare_document_tools".to_string()),
                default_value: Some(json!("check")),
            },
        ],
    }
}

fn document_format_catalog() -> PluginProviderCatalog {
    PluginProviderCatalog {
        id: "officeDocumentFormats".to_string(),
        label: "Office document formats".to_string(),
        item_kind: "documentFormat".to_string(),
        items: vec![
            json!({
                "id": "pptx",
                "label": "PowerPoint",
                "extensions": [".pptx"],
                "workflows": ["generate-presentation", "analyze-document", "compare-documents"],
                "requiredPackages": ["python-pptx"],
            }),
            json!({
                "id": "docx",
                "label": "Word",
                "extensions": [".docx"],
                "workflows": ["analyze-document", "compare-documents"],
                "requiredPackages": ["python-docx"],
            }),
            json!({
                "id": "xlsx",
                "label": "Excel",
                "extensions": [".xlsx"],
                "workflows": ["analyze-document", "compare-documents"],
                "requiredPackages": ["openpyxl"],
            }),
            json!({
                "id": "pdf",
                "label": "PDF",
                "extensions": [".pdf"],
                "workflows": ["analyze-document", "compare-documents"],
                "requiredPackages": ["pypdf"],
            }),
            json!({
                "id": "html",
                "label": "HTML document",
                "extensions": [".html", ".htm"],
                "workflows": ["compile-document", "generate-presentation"],
                "requiredPackages": [],
            }),
        ],
    }
}

fn python_package_catalog() -> PluginProviderCatalog {
    PluginProviderCatalog {
        id: "officePythonPackages".to_string(),
        label: "Office Python packages".to_string(),
        item_kind: "pythonPackage".to_string(),
        items: vec![
            json!({
                "id": "python",
                "label": "Python 3",
                "kind": "runtime",
                "required": true,
                "installUrl": "https://www.python.org/downloads/",
            }),
            json!({
                "id": "python-docx",
                "label": "python-docx",
                "kind": "package",
                "required": true,
                "formats": ["docx"],
            }),
            json!({
                "id": "openpyxl",
                "label": "openpyxl",
                "kind": "package",
                "required": true,
                "formats": ["xlsx"],
            }),
            json!({
                "id": "python-pptx",
                "label": "python-pptx",
                "kind": "package",
                "required": true,
                "formats": ["pptx"],
            }),
            json!({
                "id": "pypdf",
                "label": "pypdf",
                "kind": "package",
                "required": true,
                "formats": ["pdf"],
            }),
        ],
    }
}

fn runtime_checks(readiness: Option<&OfficeRuntimeReadiness>) -> Vec<PluginRuntimeCheck> {
    let Some(readiness) = readiness else {
        return vec![check(
            "readiness",
            "Runtime readiness",
            PluginRuntimeStatus::Unknown,
            PluginCheckSeverity::Info,
            "Office runtime readiness is checked by the desktop host.",
        )];
    };

    let (status, severity) = readiness_status(&readiness.status);
    let mut checks = vec![check(
        "runtime",
        "Runtime",
        status,
        severity,
        &readiness.summary,
    )];

    checks.extend(readiness.dependencies.iter().map(dependency_check));
    checks
}

fn readiness_status(status: &str) -> (PluginRuntimeStatus, PluginCheckSeverity) {
    match status {
        "ready" => (PluginRuntimeStatus::Pass, PluginCheckSeverity::Info),
        "degraded" => (PluginRuntimeStatus::Warning, PluginCheckSeverity::Warning),
        "missing" => (PluginRuntimeStatus::Warning, PluginCheckSeverity::Warning),
        "blocked" => (PluginRuntimeStatus::Error, PluginCheckSeverity::Error),
        _ => (PluginRuntimeStatus::Unknown, PluginCheckSeverity::Info),
    }
}

fn dependency_check(dep: &OfficeDependencyStatus) -> PluginRuntimeCheck {
    let (status, severity) = dependency_status(dep);
    check(
        &format!("dependency-{}", dep.id),
        &dep.label,
        status,
        severity,
        &dependency_message(dep),
    )
}

fn dependency_status(dep: &OfficeDependencyStatus) -> (PluginRuntimeStatus, PluginCheckSeverity) {
    match dep.status.as_str() {
        "ready" => (PluginRuntimeStatus::Pass, PluginCheckSeverity::Info),
        "broken" if dep.required => (PluginRuntimeStatus::Error, PluginCheckSeverity::Error),
        "broken" => (PluginRuntimeStatus::Warning, PluginCheckSeverity::Warning),
        "missing" if dep.required => (PluginRuntimeStatus::Error, PluginCheckSeverity::Error),
        "missing" => (PluginRuntimeStatus::Warning, PluginCheckSeverity::Warning),
        _ => (PluginRuntimeStatus::Unknown, PluginCheckSeverity::Info),
    }
}

fn dependency_message(dep: &OfficeDependencyStatus) -> String {
    if let Some(detail) = dep.detail.as_deref().filter(|value| !value.is_empty()) {
        return detail.to_string();
    }
    if dep.status == "ready" {
        if let Some(version) = dep.version.as_deref().filter(|value| !value.is_empty()) {
            return format!("{} is ready at version {version}.", dep.label);
        }
        if let Some(path) = dep.path.as_deref().filter(|value| !value.is_empty()) {
            return format!("{} is ready at {path}.", dep.label);
        }
        return format!("{} is ready.", dep.label);
    }
    if let Some(hint) = dep
        .install_hint
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return format!("{} is {}. Install from {hint}.", dep.label, dep.status);
    }
    format!("{} is {}.", dep.label, dep.status)
}

fn check(
    id: &str,
    label: &str,
    status: PluginRuntimeStatus,
    severity: PluginCheckSeverity,
    message: &str,
) -> PluginRuntimeCheck {
    PluginRuntimeCheck {
        id: id.to_string(),
        label: label.to_string(),
        status,
        severity,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dependency(id: &str, label: &str, status: &str) -> OfficeDependencyStatus {
        OfficeDependencyStatus {
            id: id.to_string(),
            label: label.to_string(),
            kind: "package".to_string(),
            required: true,
            status: status.to_string(),
            version: None,
            path: None,
            detail: None,
            install_hint: None,
        }
    }

    fn readiness(status: &str) -> OfficeRuntimeReadiness {
        OfficeRuntimeReadiness {
            status: status.to_string(),
            summary: "test summary".to_string(),
            python_path: Some("python".to_string()),
            app_managed_python_path: Some("runtime/python".to_string()),
            app_managed_env_path: "runtime".to_string(),
            skill_script_path: "skills/doc-script-editor/scripts/edit_doc.py".to_string(),
            requirements_path: "skills/doc-script-editor/scripts/requirements.txt".to_string(),
            can_prepare: true,
            can_install_python_packages: true,
            needs_python_install: false,
            python_download_url: "https://www.python.org/downloads/".to_string(),
            dependencies: vec![
                dependency("python", "Python 3", "ready"),
                dependency("python-docx", "python-docx", "ready"),
                dependency("openpyxl", "openpyxl", "missing"),
            ],
        }
    }

    fn manifest() -> PluginManifest {
        PluginManifest {
            id: "office-documents".to_string(),
            name: "Office Documents".to_string(),
            capability: "Document generation and analysis".to_string(),
            description: "test".to_string(),
            built_in: true,
            tools: vec!["prepare_document_tools".to_string()],
            settings_surfaces: vec!["office-runtime".to_string()],
            workflows: vec!["generate-presentation".to_string()],
            settings_schema: None,
            provider_catalogs: Vec::new(),
            runtime_checks: Vec::new(),
        }
    }

    #[test]
    fn office_manifest_carries_runtime_schema_and_catalogs() {
        let manifest = enrich_manifest(manifest(), Some(&readiness("missing")));

        assert!(manifest
            .settings_schema
            .as_ref()
            .is_some_and(|schema| schema.config_key == "officeRuntime"));
        assert!(manifest
            .provider_catalogs
            .iter()
            .any(|catalog| catalog.id == "officeDocumentFormats"));
        assert!(manifest.runtime_checks.iter().any(|check| {
            check.id == "dependency-openpyxl" && check.status == PluginRuntimeStatus::Error
        }));
    }

    #[test]
    fn office_manifest_without_readiness_is_explicitly_unknown() {
        let manifest = enrich_manifest(manifest(), None);

        assert!(manifest.runtime_checks.iter().any(|check| {
            check.id == "readiness" && check.status == PluginRuntimeStatus::Unknown
        }));
    }
}
