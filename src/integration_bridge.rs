// src/integration_bridge.rs
//
// Bridges cyan-backend-integrations crate into Cyan's FFI layer.
//
// This module:
// 1. Wraps IntegrationManager from cyan-backend-integrations
// 2. Persists integration bindings to SQLite (no tokens)
// 3. Transforms IntegrationEvent → SwiftEvent::IntegrationEvent
// 4. Provides single FFI entry point: cyan_integration_command(json)
//
// Usage in lib.rs:
//   mod integration_bridge;
//   pub use integration_bridge::IntegrationBridge;
//
//   // In CyanSystem struct:
//   pub integration_bridge: Arc<IntegrationBridge>,
//
//   // In CyanSystem::new():
//   let integration_bridge = Arc::new(IntegrationBridge::new(db.clone(), event_tx.clone()));
//   integration_bridge.start_event_forwarder();

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

// Import from cyan-backend-integrations crate
use cyan_backend_integrations::{
    IntegrationEvent as CrateIntegrationEvent,
    IntegrationManager,
    IntegrationType as CrateIntegrationType,
    Node,
    NodeKind,
};

use crate::SwiftEvent;

// ============================================================================
// Public Types (for FFI JSON interface)
// ============================================================================

/// Integration types supported (mirrors crate's IntegrationType)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationType {
    Slack,
    Jira,
    GitHub,
    Confluence,
    GoogleDocs,
}

impl std::fmt::Display for IntegrationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntegrationType::Slack => write!(f, "slack"),
            IntegrationType::Jira => write!(f, "jira"),
            IntegrationType::GitHub => write!(f, "github"),
            IntegrationType::Confluence => write!(f, "confluence"),
            IntegrationType::GoogleDocs => write!(f, "googledocs"),
        }
    }
}

impl IntegrationType {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "slack" => Some(IntegrationType::Slack),
            "jira" => Some(IntegrationType::Jira),
            "github" => Some(IntegrationType::GitHub),
            "confluence" => Some(IntegrationType::Confluence),
            "googledocs" => Some(IntegrationType::GoogleDocs),
            _ => None,
        }
    }
}

// ============================================================================
// Command/Response Types (JSON dispatch from Swift)
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum IntegrationCommand {
    /// Start an integration and auto-save binding
    Start {
        scope_type: String,
        scope_id: String,
        integration_type: String,
        token: String,
        config: serde_json::Value,
    },
    /// Stop an integration (binding remains)
    Stop {
        scope_id: String,
        integration_type: String,
    },
    /// Remove binding (stop + delete from DB)
    RemoveBinding {
        scope_id: String,
        integration_type: String,
    },
    /// Get bindings for a scope
    GetBindings {
        scope_type: Option<String>,
        scope_id: String,
    },
    /// Check if integration is running
    IsRunning {
        scope_id: String,
        integration_type: String,
    },
    /// List available channels (for Slack setup UI)
    ListSlackChannels {
        scope_id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct CommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl CommandResponse {
    fn ok() -> Self {
        Self { success: true, error: None, data: None }
    }

    fn ok_with_data(data: serde_json::Value) -> Self {
        Self { success: true, error: None, data: Some(data) }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self { success: false, error: Some(msg.into()), data: None }
    }
}

// ============================================================================
// Integration Bridge (Main Entry Point)
// ============================================================================

pub struct IntegrationBridge {
    db: Arc<Mutex<Connection>>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    /// The actual integration manager from cyan-backend-integrations
    manager: Arc<IntegrationManager>,
    /// Track which integrations are running: key = "scope_id:integration_type"
    running: RwLock<HashMap<String, RunningIntegration>>,
}

struct RunningIntegration {
    scope_type: String,
    scope_id: String,
    integration_type: IntegrationType,
}

impl IntegrationBridge {
    /// Create new bridge. Call once during CyanSystem::new()
    pub fn new(
        db: Arc<Mutex<Connection>>,
        event_tx: mpsc::UnboundedSender<SwiftEvent>,
    ) -> Self {
        // Ensure schema exists
        Self::ensure_schema(&db);

        Self {
            db,
            event_tx,
            manager: Arc::new(IntegrationManager::new()),
            running: RwLock::new(HashMap::new()),
        }
    }

    /// Start the background event forwarder task.
    /// Call this after creating IntegrationBridge.
    pub fn start_event_forwarder(self: &Arc<Self>) {
        let bridge = Arc::clone(self);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;
                bridge.forward_events().await;
            }
        });
    }

    /// Poll IntegrationManager for events and forward to Swift
    async fn forward_events(&self) {
        let running = self.running.read().await;

        for (key, info) in running.iter() {
            // Get events from the manager
            let events = self.manager.get_all_events(&info.scope_id).await;

            for event in events {
                // Transform to SwiftEvent
                let swift_event = self.transform_event(&event, &info.scope_type, &info.scope_id);

                if let Err(e) = self.event_tx.send(swift_event) {
                    tracing::error!("Failed to send integration event: {}", e);
                }
            }

            // Also get nodes and send as events (for console display)
            let nodes = self.manager.get_all_nodes(&info.scope_id).await;

            for node in nodes {
                if !matches!(node.kind, NodeKind::JiraTicket | NodeKind::GitHubPR | NodeKind::GitHubIssue) {
                    continue;
                }
                let swift_event = self.transform_node(&node, &info.scope_type, &info.scope_id);

                if let Err(e) = self.event_tx.send(swift_event) {
                    tracing::error!("Failed to send node event: {}", e);
                }
            }
        }
    }

    /// Transform IntegrationEvent from crate to SwiftEvent
    fn transform_event(&self, event: &CrateIntegrationEvent, scope_type: &str, scope_id: &str) -> SwiftEvent {
        // Parse payload to extract summary/context
        let payload: serde_json::Value = serde_json::from_str(&event.base.payload)
            .unwrap_or(serde_json::json!({}));

        let event_type = payload.get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let (summary, context, source) = match event_type {
            "jira_mention" => {
                let jira_id = payload.get("jira_id").and_then(|v| v.as_str()).unwrap_or("");
                let channel = payload.get("channel").and_then(|v| v.as_str()).unwrap_or("");
                (
                    format!("{} mentioned", jira_id),
                    format!("#{}", channel),
                    "slack".to_string(),
                )
            }
            "github_pr_reference" => {
                let pr_num = payload.get("pr_number").and_then(|v| v.as_str()).unwrap_or("");
                let jira_key = payload.get("jira_key").and_then(|v| v.as_str()).unwrap_or("");
                (
                    format!("PR #{} linked", pr_num),
                    jira_key.to_string(),
                    "jira".to_string(),
                )
            }
            _ => {
                (
                    format!("{:?}", event.relation),
                    "".to_string(),
                    "unknown".to_string(),
                )
            }
        };

        SwiftEvent::IntegrationEvent {
            id: event.base.id.clone(),
            scope_id: format!("{}:{}", scope_type, scope_id),
            source,
            summary,
            context,
            url: None,
            ts: event.base.ts,
        }
    }

    /// Transform Node from crate to SwiftEvent (for new messages, issues, etc.)
    fn transform_node(&self, node: &Node, scope_type: &str, scope_id: &str) -> SwiftEvent {
        let (source, summary, context, url) = match &node.kind {
            NodeKind::SlackMessage | NodeKind::SlackThread => {
                let author = if node.metadata.author.is_empty() {
                    "unknown".to_string()
                } else {
                    format!("@{}", node.metadata.author)
                };

                // Truncate message for summary
                let text = if node.content.len() > 50 {
                    format!("{}...", truncate_str(&node.content, 50))
                } else {
                    node.content.clone()
                };

                (
                    "slack".to_string(),
                    format!("{}: \"{}\"", author, text),
                    node.name.replace("Message in ", "#"),
                    Some(node.metadata.url.clone()),
                )
            }
            NodeKind::JiraTicket => {
                let status = node.metadata.status.as_deref().unwrap_or("Unknown");
                (
                    "jira".to_string(),
                    format!("{} → {}", node.external_id, status),
                    node.metadata.author.clone(),
                    Some(node.metadata.url.clone()),
                )
            }
            NodeKind::JiraComment => {
                (
                    "jira".to_string(),
                    format!("Comment on {}", node.external_id),
                    format!("@{}", node.metadata.author),
                    Some(node.metadata.url.clone()),
                )
            }
            NodeKind::GitHubPR | NodeKind::GitHubIssue => {
                let title = node.metadata.title.as_deref().unwrap_or(&node.name);
                (
                    "github".to_string(),
                    title.to_string(),
                    format!("@{}", node.metadata.author),
                    Some(node.metadata.url.clone()),
                )
            }
            NodeKind::GitHubCommit => {
                (
                    "github".to_string(),
                    node.content.lines().next().unwrap_or("Commit").to_string(),
                    format!("@{}", node.metadata.author),
                    Some(node.metadata.url.clone()),
                )
            }
            NodeKind::ConfluencePage => {
                let title = node.metadata.title.as_deref().unwrap_or("Page");
                (
                    "confluence".to_string(),
                    format!("{} updated", title),
                    format!("@{}", node.metadata.author),
                    Some(node.metadata.url.clone()),
                )
            }
            NodeKind::GoogleDoc | NodeKind::GoogleSheet | NodeKind::GoogleSlide => {
                let title = node.metadata.title.as_deref().unwrap_or("Document");
                (
                    "googledocs".to_string(),
                    format!("{} updated", title),
                    format!("@{}", node.metadata.author),
                    Some(node.metadata.url.clone()),
                )
            }
            _ => {
                (
                    "unknown".to_string(),
                    node.name.clone(),
                    "".to_string(),
                    None,
                )
            }
        };

        SwiftEvent::IntegrationEvent {
            id: node.id.clone(),
            scope_id: format!("{}:{}", scope_type, scope_id),
            source,
            summary,
            context,
            url,
            ts: node.ts,
        }
    }

    /// Handle a JSON command from FFI. Returns JSON response.
    pub async fn handle_command(&self, json: &str) -> String {
        let response = match serde_json::from_str::<IntegrationCommand>(json) {
            Ok(cmd) => self.dispatch(cmd).await,
            Err(e) => CommandResponse::err(format!("Invalid command JSON: {}", e)),
        };

        serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"success":false,"error":"Serialization failed"}"#.to_string()
        })
    }

    // ========================================================================
    // Command Dispatch
    // ========================================================================

    async fn dispatch(&self, cmd: IntegrationCommand) -> CommandResponse {
        match cmd {
            IntegrationCommand::Start { scope_type, scope_id, integration_type, token, config } => {
                self.cmd_start(&scope_type, &scope_id, &integration_type, &token, config).await
            }
            IntegrationCommand::Stop { scope_id, integration_type } => {
                self.cmd_stop(&scope_id, &integration_type).await
            }
            IntegrationCommand::RemoveBinding { scope_id, integration_type } => {
                self.cmd_remove_binding(&scope_id, &integration_type).await
            }
            IntegrationCommand::GetBindings { scope_type, scope_id } => {
                self.cmd_get_bindings(scope_type.as_deref(), &scope_id).await
            }
            IntegrationCommand::IsRunning { scope_id, integration_type } => {
                self.cmd_is_running(&scope_id, &integration_type).await
            }
            IntegrationCommand::ListSlackChannels { scope_id } => {
                self.cmd_list_slack_channels(&scope_id).await
            }
        }
    }

    // ========================================================================
    // Command Implementations
    // ========================================================================

    async fn cmd_start(
        &self,
        scope_type: &str,
        scope_id: &str,
        integration_type: &str,
        token: &str,
        config: serde_json::Value,
    ) -> CommandResponse {
        // Validate integration type
        let i_type = match IntegrationType::from_str(integration_type) {
            Some(t) => t,
            None => return CommandResponse::err(format!("Unknown integration type: {}", integration_type)),
        };

        // Validate scope type
        if scope_type != "group" && scope_type != "workspace" {
            return CommandResponse::err("scope_type must be 'group' or 'workspace'");
        }

        let actor_key = format!("{}:{}", scope_id, integration_type);

        // Check if already running
        if self.running.read().await.contains_key(&actor_key) {
            return CommandResponse::err("Integration already running for this scope");
        }

        // Start the integration via IntegrationManager
        let result = match i_type {
            IntegrationType::Slack => {
                let channels: Vec<String> = config["channels"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                self.manager.start_slack(
                    scope_id.to_string(),
                    token.to_string(),
                    channels,
                ).await
            }
            IntegrationType::Jira => {
                let domain = config["domain"].as_str().unwrap_or("").to_string();
                let email = config["email"].as_str().unwrap_or("").to_string();
                let projects: Vec<String> = config["projects"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                self.manager.start_jira(
                    domain,
                    email,
                    token.to_string(),
                    scope_id.to_string(),
                    projects,
                ).await
            }
            IntegrationType::GitHub => {
                let repos: Vec<(String, String)> = config["repos"]
                    .as_array()
                    .map(|arr| {
                        arr.iter().filter_map(|v| {
                            let owner = v["owner"].as_str()?;
                            let repo = v["repo"].as_str()?;
                            Some((owner.to_string(), repo.to_string()))
                        }).collect()
                    })
                    .unwrap_or_default();

                self.manager.start_github(
                    token.to_string(),
                    scope_id.to_string(),
                    repos,
                ).await
            }
            IntegrationType::Confluence => {
                let domain = config["domain"].as_str().unwrap_or("").to_string();
                let email = config["email"].as_str().unwrap_or("").to_string();
                let spaces: Vec<String> = config["spaces"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                self.manager.start_confluence(
                    domain,
                    email,
                    token.to_string(),
                    scope_id.to_string(),
                    spaces,
                ).await
            }
            IntegrationType::GoogleDocs => {
                let document_ids: Vec<String> = config["document_ids"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                self.manager.start_googledocs(
                    token.to_string(),
                    scope_id.to_string(),
                    document_ids,
                ).await
            }
        };

        if let Err(e) = result {
            return CommandResponse::err(format!("Failed to start integration: {}", e));
        }

        // Track as running
        self.running.write().await.insert(actor_key, RunningIntegration {
            scope_type: scope_type.to_string(),
            scope_id: scope_id.to_string(),
            integration_type: i_type.clone(),
        });

        // Save binding to DB (no token)
        if let Err(e) = self.save_binding(scope_type, scope_id, integration_type, &config) {
            return CommandResponse::err(format!("Failed to save binding: {}", e));
        }

        // Send status event
        let _ = self.event_tx.send(SwiftEvent::IntegrationStatus {
            scope_id: format!("{}:{}", scope_type, scope_id),
            integration_type: integration_type.to_string(),
            status: "connected".to_string(),
            message: None,
        });

        CommandResponse::ok()
    }

    async fn cmd_stop(&self, scope_id: &str, integration_type: &str) -> CommandResponse {
        let actor_key = format!("{}:{}", scope_id, integration_type);

        // Check if running
        let was_running = self.running.write().await.remove(&actor_key).is_some();

        if !was_running {
            return CommandResponse::err("Integration not running");
        }

        // Stop via IntegrationManager
        let result = match integration_type {
            "slack" => self.manager.stop_slack(scope_id).await,
            "jira" => self.manager.stop_jira(scope_id).await,
            "github" => self.manager.stop_github(scope_id).await,
            "confluence" => self.manager.stop_confluence(scope_id).await,
            "googledocs" => self.manager.stop_googledocs(scope_id).await,
            _ => Err("Unknown integration type".to_string()),
        };

        if let Err(e) = result {
            tracing::warn!("Error stopping integration: {}", e);
        }

        // Send status event
        let _ = self.event_tx.send(SwiftEvent::IntegrationStatus {
            scope_id: scope_id.to_string(),
            integration_type: integration_type.to_string(),
            status: "stopped".to_string(),
            message: None,
        });

        CommandResponse::ok()
    }

    async fn cmd_remove_binding(&self, scope_id: &str, integration_type: &str) -> CommandResponse {
        // Stop actor first
        let _ = self.cmd_stop(scope_id, integration_type).await;

        // Delete from DB
        let result = {
            let db = self.db.lock().unwrap();
            db.execute(
                "DELETE FROM integration_bindings WHERE scope_id = ?1 AND integration_type = ?2",
                params![scope_id, integration_type],
            )
        };

        match result {
            Ok(_) => CommandResponse::ok(),
            Err(e) => CommandResponse::err(format!("DB error: {}", e)),
        }
    }

    async fn cmd_get_bindings(&self, scope_type: Option<&str>, scope_id: &str) -> CommandResponse {
        let bindings = {
            let db = self.db.lock().unwrap();

            let query = match scope_type {
                Some(st) => {
                    let mut stmt = db.prepare(
                        "SELECT id, scope_type, scope_id, integration_type, config_json, created_at
                         FROM integration_bindings
                         WHERE scope_type = ?1 AND scope_id = ?2"
                    ).unwrap();

                    stmt.query_map(params![st, scope_id], Self::row_to_binding)
                        .unwrap()
                        .filter_map(|r| r.ok())
                        .collect::<Vec<_>>()
                }
                None => {
                    let mut stmt = db.prepare(
                        "SELECT id, scope_type, scope_id, integration_type, config_json, created_at
                         FROM integration_bindings
                         WHERE scope_id = ?1"
                    ).unwrap();

                    stmt.query_map(params![scope_id], Self::row_to_binding)
                        .unwrap()
                        .filter_map(|r| r.ok())
                        .collect::<Vec<_>>()
                }
            };

            query
        };

        CommandResponse::ok_with_data(serde_json::json!(bindings))
    }

    async fn cmd_is_running(&self, scope_id: &str, integration_type: &str) -> CommandResponse {
        let actor_key = format!("{}:{}", scope_id, integration_type);
        let running = self.running.read().await.contains_key(&actor_key);

        CommandResponse::ok_with_data(serde_json::json!({ "running": running }))
    }

    async fn cmd_list_slack_channels(&self, scope_id: &str) -> CommandResponse {
        // This would require storing the SlackClient reference
        // For now, return error - channels should be selected during OAuth flow
        CommandResponse::err("List channels via OAuth flow in Swift, not here")
    }

    // ========================================================================
    // Database Helpers
    // ========================================================================

    fn ensure_schema(db: &Arc<Mutex<Connection>>) {
        let db = db.lock().unwrap();
        let _ = db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS integration_bindings (
                id TEXT PRIMARY KEY,
                scope_type TEXT NOT NULL,
                scope_id TEXT NOT NULL,
                integration_type TEXT NOT NULL,
                config_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_integration_scope ON integration_bindings(scope_type, scope_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_integration_unique ON integration_bindings(scope_id, integration_type);
            "#
        );
    }

    fn save_binding(
        &self,
        scope_type: &str,
        scope_id: &str,
        integration_type: &str,
        config: &serde_json::Value,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let binding_id = blake3::hash(
            format!("binding:{}:{}:{}", scope_type, scope_id, integration_type).as_bytes()
        ).to_hex().to_string();

        let config_str = serde_json::to_string(config).map_err(|e| e.to_string())?;

        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO integration_bindings (id, scope_type, scope_id, integration_type, config_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![binding_id, scope_type, scope_id, integration_type, config_str, now],
        ).map_err(|e| e.to_string())?;

        Ok(())
    }

    fn row_to_binding(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?,
            "scope_type": row.get::<_, String>(1)?,
            "scope_id": row.get::<_, String>(2)?,
            "integration_type": row.get::<_, String>(3)?,
            "config": serde_json::from_str::<serde_json::Value>(
                &row.get::<_, String>(4)?
            ).unwrap_or(serde_json::Value::Null),
            "created_at": row.get::<_, i64>(5)?
        }))
    }

    // ========================================================================
    // App Restart: Restore integrations from bindings
    // ========================================================================

    /// Called on app startup to restore integrations.
    /// Swift must call this with tokens retrieved from Keychain.
    pub async fn restore_integration(
        &self,
        scope_type: &str,
        scope_id: &str,
        integration_type: &str,
        token: &str,
    ) -> Result<(), String> {
        // Get binding from DB
        let config = {
            let db = self.db.lock().unwrap();
            let config_str: Option<String> = db.query_row(
                "SELECT config_json FROM integration_bindings WHERE scope_id = ?1 AND integration_type = ?2",
                params![scope_id, integration_type],
                |row| row.get(0),
            ).ok();

            match config_str {
                Some(s) => serde_json::from_str(&s).unwrap_or(serde_json::json!({})),
                None => return Err("Binding not found".to_string()),
            }
        };

        // Start the integration
        let cmd_json = serde_json::json!({
            "cmd": "start",
            "scope_type": scope_type,
            "scope_id": scope_id,
            "integration_type": integration_type,
            "token": token,
            "config": config,
        });

        let response = self.handle_command(&cmd_json.to_string()).await;
        let parsed: CommandResponse = serde_json::from_str(&response).map_err(|e| e.to_string())?;

        if parsed.success {
            Ok(())
        } else {
            Err(parsed.error.unwrap_or("Unknown error".to_string()))
        }
    }
}

// Put this somewhere common, maybe in lib.rs or a utils module
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
