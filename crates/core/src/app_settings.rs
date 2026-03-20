use serde::{Deserialize, Serialize};
use rusqlite::params;
use crate::db::Database;
use crate::error::CoreError;

const APP_CONFIG_KEY: &str = "app_config";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: i64,
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout_secs: i64,
}

fn default_tool_timeout() -> i64 { 30 }
fn default_agent_timeout() -> i64 { 180 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            tool_timeout_secs: default_tool_timeout(),
            agent_timeout_secs: default_agent_timeout(),
        }
    }
}

impl Database {
    pub fn load_app_config(&self) -> Result<AppConfig, CoreError> {
        let conn = self.conn();
        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_config')",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(AppConfig::default());
        }
        let result = conn.query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![APP_CONFIG_KEY],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => {
                let config: AppConfig = serde_json::from_str(&json)?;
                Ok(config)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(AppConfig::default()),
            Err(e) => Err(CoreError::Database(e)),
        }
    }

    pub fn save_app_config(&self, config: &AppConfig) -> Result<(), CoreError> {
        let json = serde_json::to_string(config)?;
        let conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_config (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )?;
        conn.execute(
            "INSERT INTO app_config (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![APP_CONFIG_KEY, &json],
        )?;
        Ok(())
    }
}
