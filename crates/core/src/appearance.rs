//! Durable appearance registry shared by Settings and the Agent.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;
use crate::theme_resource_plugin::ThemeResourcePlugin;

const APPEARANCE_REGISTRY_KEY: &str = "appearance_registry_v2";
const BUILTIN_THEME_IDS: &[&str] = &["dark", "light", "midnight", "aurora", "bloom", "dream"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceRegistry {
    pub version: u8,
    pub initialized: bool,
    pub revision: u64,
    pub active_theme_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_theme_id: Option<String>,
    #[serde(default)]
    pub plugins: Vec<ThemeResourcePlugin>,
    #[serde(default)]
    file_projection_initialized: bool,
    #[serde(default)]
    file_backed_theme_ids: Vec<String>,
}

impl Default for AppearanceRegistry {
    fn default() -> Self {
        Self {
            version: 2,
            initialized: false,
            revision: 0,
            active_theme_id: "dark".to_string(),
            previous_theme_id: None,
            plugins: Vec::new(),
            file_projection_initialized: false,
            file_backed_theme_ids: Vec::new(),
        }
    }
}

impl AppearanceRegistry {
    pub fn normalize(mut self) -> Result<Self, CoreError> {
        self.version = 2;
        let mut normalized = Vec::with_capacity(self.plugins.len());
        for plugin in self.plugins {
            let plugin = plugin.normalize()?;
            if let Some(index) = normalized
                .iter()
                .position(|existing: &ThemeResourcePlugin| existing.id == plugin.id)
            {
                normalized[index] = plugin;
            } else {
                normalized.push(plugin);
            }
        }
        self.plugins = normalized;
        let plugin_ids = self
            .plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<HashSet<_>>();
        self.file_backed_theme_ids
            .retain(|id| plugin_ids.contains(id.as_str()));
        self.file_backed_theme_ids.sort();
        self.file_backed_theme_ids.dedup();
        if !self.has_theme(&self.active_theme_id) {
            self.active_theme_id = "dark".to_string();
        }
        if self
            .previous_theme_id
            .as_deref()
            .is_some_and(|id| !self.has_theme(id))
        {
            self.previous_theme_id = None;
        }
        Ok(self)
    }

    pub fn hydrate(
        mut self,
        plugins: Vec<ThemeResourcePlugin>,
        active_theme_id: String,
    ) -> Result<Self, CoreError> {
        if self.initialized {
            return self.normalize();
        }
        self.initialized = true;
        self.plugins = plugins;
        self.active_theme_id = active_theme_id;
        self.revision = self.revision.saturating_add(1);
        self.normalize()
    }

    /// Reconcile authoritative file-backed theme declarations. Once a
    /// successful file projection has been recorded, a previously tracked id
    /// that is neither present nor protected is treated as a user deletion.
    pub fn reconcile_file_plugins(
        mut self,
        plugins: Vec<ThemeResourcePlugin>,
        preserved_theme_ids: &[String],
    ) -> Result<Self, CoreError> {
        let plugins = plugins
            .into_iter()
            .map(ThemeResourcePlugin::normalize)
            .collect::<Result<Vec<_>, _>>()?;
        let present = plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<HashSet<_>>();
        let preserved = preserved_theme_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut changed = false;
        if self.file_projection_initialized {
            let removed = self
                .file_backed_theme_ids
                .iter()
                .filter(|id| !present.contains(id.as_str()) && !preserved.contains(id.as_str()))
                .cloned()
                .collect::<HashSet<_>>();
            if !removed.is_empty() {
                self.plugins.retain(|plugin| !removed.contains(&plugin.id));
                if removed.contains(&self.active_theme_id) {
                    self.active_theme_id = self
                        .previous_theme_id
                        .clone()
                        .filter(|id| self.has_theme(id))
                        .unwrap_or_else(|| "dark".to_string());
                }
                if self
                    .previous_theme_id
                    .as_ref()
                    .is_some_and(|id| removed.contains(id))
                {
                    self.previous_theme_id = None;
                }
                changed = true;
            }
        }
        for plugin in plugins {
            if let Some(existing) = self.plugins.iter_mut().find(|item| item.id == plugin.id) {
                if *existing != plugin {
                    *existing = plugin;
                    changed = true;
                }
            } else {
                self.plugins.push(plugin);
                changed = true;
            }
        }
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
        self.normalize()
    }

    pub fn commit_file_projection(mut self, theme_ids: Vec<String>) -> Result<Self, CoreError> {
        let installed = self
            .plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<HashSet<_>>();
        self.file_backed_theme_ids = theme_ids
            .into_iter()
            .filter(|id| installed.contains(id.as_str()))
            .collect();
        self.file_projection_initialized = true;
        self.normalize()
    }

    pub fn apply(mut self, plugin: ThemeResourcePlugin) -> Result<Self, CoreError> {
        let plugin = plugin.normalize()?;
        self.initialized = true;
        if self.active_theme_id != plugin.id {
            self.previous_theme_id = Some(self.active_theme_id.clone());
            self.active_theme_id = plugin.id.clone();
        }
        if let Some(existing) = self.plugins.iter_mut().find(|item| item.id == plugin.id) {
            *existing = plugin;
        } else {
            self.plugins.push(plugin);
        }
        self.revision = self.revision.saturating_add(1);
        self.normalize()
    }

    pub fn activate(mut self, theme_id: &str) -> Result<Self, CoreError> {
        if !self.has_theme(theme_id) {
            return Err(CoreError::NotFound(format!("Theme {theme_id}")));
        }
        if self.active_theme_id != theme_id {
            self.previous_theme_id = Some(self.active_theme_id.clone());
            self.active_theme_id = theme_id.to_string();
            self.revision = self.revision.saturating_add(1);
        }
        self.initialized = true;
        Ok(self)
    }

    pub fn rollback(mut self) -> Result<Self, CoreError> {
        let previous = self
            .previous_theme_id
            .clone()
            .filter(|id| self.has_theme(id))
            .ok_or_else(|| CoreError::InvalidInput("No previous appearance is available".into()))?;
        self.previous_theme_id = Some(self.active_theme_id.clone());
        self.active_theme_id = previous;
        self.revision = self.revision.saturating_add(1);
        Ok(self)
    }

    pub fn remove(mut self, theme_id: &str) -> Result<Self, CoreError> {
        let before = self.plugins.len();
        self.plugins.retain(|plugin| plugin.id != theme_id);
        if self.plugins.len() == before {
            return Err(CoreError::NotFound(format!("Custom theme {theme_id}")));
        }
        if self.active_theme_id == theme_id {
            self.active_theme_id = self
                .previous_theme_id
                .clone()
                .filter(|id| self.has_theme(id))
                .unwrap_or_else(|| "dark".to_string());
        }
        if self.previous_theme_id.as_deref() == Some(theme_id) {
            self.previous_theme_id = None;
        }
        self.revision = self.revision.saturating_add(1);
        Ok(self)
    }

    fn has_theme(&self, id: &str) -> bool {
        BUILTIN_THEME_IDS.contains(&id) || self.plugins.iter().any(|plugin| plugin.id == id)
    }
}

impl Database {
    pub fn load_appearance_registry(&self) -> Result<AppearanceRegistry, CoreError> {
        load_registry(&self.conn())
    }

    pub fn hydrate_appearance_registry(
        &self,
        plugins: Vec<ThemeResourcePlugin>,
        active_theme_id: String,
    ) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(|registry| registry.hydrate(plugins, active_theme_id))
    }

    pub fn apply_appearance_plugin(
        &self,
        plugin: ThemeResourcePlugin,
    ) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(|registry| registry.apply(plugin))
    }

    pub fn reconcile_appearance_file_plugins(
        &self,
        plugins: Vec<ThemeResourcePlugin>,
        preserved_theme_ids: Vec<String>,
    ) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(|registry| {
            registry.reconcile_file_plugins(plugins, &preserved_theme_ids)
        })
    }

    pub fn commit_appearance_file_projection(
        &self,
        theme_ids: Vec<String>,
    ) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(|registry| registry.commit_file_projection(theme_ids))
    }

    pub fn activate_appearance(&self, theme_id: &str) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(|registry| registry.activate(theme_id))
    }

    pub fn rollback_appearance(&self) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(AppearanceRegistry::rollback)
    }

    pub fn remove_appearance(&self, theme_id: &str) -> Result<AppearanceRegistry, CoreError> {
        self.mutate_appearance_registry(|registry| registry.remove(theme_id))
    }

    fn mutate_appearance_registry(
        &self,
        update: impl FnOnce(AppearanceRegistry) -> Result<AppearanceRegistry, CoreError>,
    ) -> Result<AppearanceRegistry, CoreError> {
        let mut connection = self.conn();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure_app_config_table(&transaction)?;
        let next = update(load_registry_from_transaction(&transaction)?)?.normalize()?;
        let json = serde_json::to_string(&next)?;
        transaction.execute(
            "INSERT INTO app_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![APPEARANCE_REGISTRY_KEY, json],
        )?;
        transaction.commit()?;
        Ok(next)
    }
}

fn load_registry(connection: &Connection) -> Result<AppearanceRegistry, CoreError> {
    let table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_config')",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(AppearanceRegistry::default());
    }
    let json = connection
        .query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![APPEARANCE_REGISTRY_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str::<AppearanceRegistry>(&value))
        .transpose()?
        .unwrap_or_default()
        .normalize()
}

fn load_registry_from_transaction(
    transaction: &Transaction<'_>,
) -> Result<AppearanceRegistry, CoreError> {
    let json = transaction
        .query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![APPEARANCE_REGISTRY_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str::<AppearanceRegistry>(&value))
        .transpose()?
        .unwrap_or_default()
        .normalize()
}

fn ensure_app_config_table(transaction: &Transaction<'_>) -> Result<(), CoreError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_config (
             key TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         )",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plugin(id: &str) -> ThemeResourcePlugin {
        serde_json::from_value(json!({
            "manifestVersion": 1,
            "kind": "theme-resource",
            "id": id,
            "name": "Autumn",
            "theme": {
                "baseTheme": "dark",
                "mode": "dark",
                "colors": { "accent": "#d66a3e" },
                "effects": {},
                "background": { "kind": "none" }
            }
        }))
        .unwrap()
    }

    #[test]
    fn registry_migrates_v1_plugins_and_rolls_back() {
        let db = Database::open_memory().unwrap();
        let hydrated = db
            .hydrate_appearance_registry(vec![plugin("autumn")], "autumn".into())
            .unwrap();
        assert_eq!(hydrated.plugins[0].manifest_version, 2);
        let light = db.activate_appearance("light").unwrap();
        assert_eq!(light.previous_theme_id.as_deref(), Some("autumn"));
        let rolled_back = db.rollback_appearance().unwrap();
        assert_eq!(rolled_back.active_theme_id, "autumn");
    }

    #[test]
    fn applying_and_removing_custom_theme_is_durable() {
        let db = Database::open_memory().unwrap();
        db.hydrate_appearance_registry(Vec::new(), "dark".into())
            .unwrap();
        let applied = db.apply_appearance_plugin(plugin("autumn")).unwrap();
        assert_eq!(applied.active_theme_id, "autumn");
        assert_eq!(db.load_appearance_registry().unwrap(), applied);
        let removed = db.remove_appearance("autumn").unwrap();
        assert_eq!(removed.active_theme_id, "dark");
        assert!(removed.plugins.is_empty());
    }

    #[test]
    fn updating_the_active_plugin_preserves_the_previous_theme_for_rollback() {
        let db = Database::open_memory().unwrap();
        db.hydrate_appearance_registry(Vec::new(), "dark".into())
            .unwrap();
        let first = db.apply_appearance_plugin(plugin("autumn")).unwrap();
        assert_eq!(first.previous_theme_id.as_deref(), Some("dark"));

        let updated = db.apply_appearance_plugin(plugin("autumn")).unwrap();
        assert_eq!(updated.previous_theme_id.as_deref(), Some("dark"));
        assert_eq!(db.rollback_appearance().unwrap().active_theme_id, "dark");
    }

    #[test]
    fn file_backed_plugins_reconcile_after_initialization_without_changing_selection() {
        let db = Database::open_memory().unwrap();
        let initial = db
            .hydrate_appearance_registry(vec![plugin("autumn")], "autumn".into())
            .unwrap();
        let mut edited = plugin("autumn");
        edited.name = "Edited on disk".into();

        let reconciled = db
            .reconcile_appearance_file_plugins(vec![edited], Vec::new())
            .unwrap();

        assert_eq!(reconciled.active_theme_id, "autumn");
        assert_eq!(reconciled.plugins[0].name, "Edited on disk");
        assert_eq!(reconciled.revision, initial.revision + 1);
        assert_eq!(db.load_appearance_registry().unwrap(), reconciled);
    }

    #[test]
    fn file_projection_seeds_once_then_treats_missing_tracked_themes_as_deletions() {
        let db = Database::open_memory().unwrap();
        let initial = db
            .hydrate_appearance_registry(vec![plugin("autumn")], "autumn".into())
            .unwrap();

        let seeded = db
            .reconcile_appearance_file_plugins(Vec::new(), Vec::new())
            .unwrap();
        assert_eq!(seeded.plugins, initial.plugins);
        let seeded = db
            .commit_appearance_file_projection(
                seeded
                    .plugins
                    .iter()
                    .map(|plugin| plugin.id.clone())
                    .collect(),
            )
            .unwrap();
        assert!(seeded.file_projection_initialized);

        let deleted = db
            .reconcile_appearance_file_plugins(Vec::new(), Vec::new())
            .unwrap();
        assert!(deleted.plugins.is_empty());
        assert_eq!(deleted.active_theme_id, "dark");
        assert_eq!(deleted.revision, seeded.revision + 1);
    }

    #[test]
    fn rejected_file_backed_theme_is_not_treated_as_deleted() {
        let db = Database::open_memory().unwrap();
        let initial = db
            .hydrate_appearance_registry(vec![plugin("autumn")], "autumn".into())
            .unwrap();
        db.commit_appearance_file_projection(vec!["autumn".into()])
            .unwrap();

        let preserved = db
            .reconcile_appearance_file_plugins(Vec::new(), vec!["autumn".into()])
            .unwrap();

        assert_eq!(preserved.plugins, initial.plugins);
        assert_eq!(preserved.active_theme_id, "autumn");
    }
}
