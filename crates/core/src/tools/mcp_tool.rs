//! McpTool — adapter that bridges an MCP server tool to the local `Tool` trait.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{Mutex, Notify, RwLock};

use crate::error::CoreError;
use crate::mcp::client::McpClient;
use crate::mcp::{McpConnectionHealth, McpManager, McpToolInfo};

use super::{Tool, ToolCategory, ToolResult};

const MCP_RECOVERY_WAIT_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct McpClientSlot {
    client: RwLock<Arc<Mutex<McpClient>>>,
    recovering: AtomicBool,
    recovered: Notify,
}

impl McpClientSlot {
    pub(crate) fn new(client: Arc<Mutex<McpClient>>) -> Self {
        Self {
            client: RwLock::new(client),
            recovering: AtomicBool::new(false),
            recovered: Notify::new(),
        }
    }

    fn begin_recovery(&self) -> bool {
        self.recovering
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    async fn active_client(&self) -> Result<Arc<Mutex<McpClient>>, CoreError> {
        if self.recovering.load(Ordering::Acquire) {
            let notified = self.recovered.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.recovering.load(Ordering::Acquire)
                && tokio::time::timeout(MCP_RECOVERY_WAIT_TIMEOUT, &mut notified)
                    .await
                    .is_err()
            {
                return Err(CoreError::Mcp(
                    "MCP connection recovery is still in progress".into(),
                ));
            }
        }
        Ok(self.client.read().await.clone())
    }

    async fn finish_recovery(&self, client: Option<Arc<Mutex<McpClient>>>) {
        if let Some(client) = client {
            *self.client.write().await = client;
        }
        self.recovering.store(false, Ordering::Release);
        self.recovered.notify_waiters();
    }
}

/// Wraps an MCP tool so it implements the local `Tool` trait.
pub struct McpTool {
    info: McpToolInfo,
    registry_name: String,
    description: String,
    client: Arc<McpClientSlot>,
    connection_health: Arc<McpConnectionHealth>,
    recovery_manager: Option<Weak<Mutex<McpManager>>>,
    server_id: String,
}

impl McpTool {
    pub(crate) fn new(
        info: McpToolInfo,
        client: Arc<McpClientSlot>,
        server_id: String,
        registry_name: String,
        server_name: String,
        connection_health: Arc<McpConnectionHealth>,
        recovery_manager: Option<Weak<Mutex<McpManager>>>,
    ) -> Self {
        let description = match info.description.as_deref() {
            Some(text) if !text.trim().is_empty() => {
                format!("MCP server '{server_name}': {}", text.trim())
            }
            _ => format!("MCP server '{server_name}' tool '{}'", info.name),
        };
        Self {
            info,
            registry_name,
            description,
            client,
            connection_health,
            recovery_manager,
            server_id,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Mcp]
    }

    fn parameters_schema(&self) -> Value {
        self.info.input_schema.clone()
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: Value =
            serde_json::from_str(arguments).unwrap_or(Value::Object(Default::default()));
        let active_client = match self.client.active_client().await {
            Ok(client) => client,
            Err(error) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: error.to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }
        };
        let result = {
            let mut client = active_client.lock().await;
            client.call_tool(&self.info.name, args).await
        };
        match result {
            Ok(result) => Ok(ToolResult {
                call_id: call_id.to_string(),
                content: result,
                is_error: false,
                artifacts: None,
            }),
            Err(e) => {
                self.connection_health.mark_unhealthy();
                let recovery = if let Some(manager) = self
                    .recovery_manager
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .filter(|_| self.client.begin_recovery())
                {
                    let client_slot = Arc::clone(&self.client);
                    let server_id = self.server_id.clone();
                    tokio::spawn(async move {
                        let recovered = manager
                            .lock()
                            .await
                            .recover_server_after_failure(&server_id, &active_client)
                            .await
                            .ok();
                        client_slot.finish_recovery(recovered).await;
                    });
                    " Connection recovery scheduled for subsequent calls in this turn.".to_string()
                } else if self.recovery_manager.is_some() {
                    " Connection recovery is already in progress.".to_string()
                } else {
                    String::new()
                };
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("MCP tool error: {e}.{recovery}"),
                    is_error: true,
                    artifacts: None,
                })
            }
        }
    }
}
