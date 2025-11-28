// src/integration_bridge.rs
//
// Siloed integration management for Cyan.
// All integration logic lives here - minimal changes needed to lib.rs.
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
//
//   // Single FFI function:
//   #[unsafe(no_mangle)]
//   pub extern "C" fn cyan_integration_command(json: *const c_char) -> *mut c_char { ... }

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

use crate::SwiftEvent;

// ============================================================================
// Public Types
// ============================================================================

/// Integration types supported
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
// Command/Response Types (JSON dispatch)
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
    /// Get events by IDs (for citation filtering)
    GetEventsByIds {
        event_ids: Vec<String>,
    },
    /// Check if integration is running
    IsRunning {
        scope_id: String,
        integration_type: String,
    },
}

#[derive(Debug, Serialize)]
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
// Actor Handle
// ============================================================================

struct ActorHandle {
    shutdown_tx: mpsc::Sender<()>,
    integration_type: IntegrationType,
}

// ============================================================================
// Integration Bridge (Main Entry Point)
// ============================================================================

pub struct IntegrationBridge {
    db: Arc<Mutex<Connection>>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    actors: RwLock<HashMap<String, ActorHandle>>,  // key: "{scope_id}:{integration_type}"
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
            actors: RwLock::new(HashMap::new()),
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
            IntegrationCommand::GetEventsByIds { event_ids } => {
                self.cmd_get_events_by_ids(&event_ids).await
            }
            IntegrationCommand::IsRunning { scope_id, integration_type } => {
                self.cmd_is_running(&scope_id, &integration_type).await
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

        let actor_key = format!("{}:{}:{}", scope_type, scope_id, integration_type);

        // Check if already running
        if self.actors.read().await.contains_key(&actor_key) {
            return CommandResponse::err("Integration already running for this scope");
        }

        // Start the actor
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
        let scope_key = format!("{}:{}", scope_type, scope_id);

        match self.spawn_actor(
            i_type.clone(),
            scope_key.clone(),
            token.to_string(),
            config.clone(),
            shutdown_rx,
        ).await {
            Ok(_) => {}
            Err(e) => return CommandResponse::err(e),
        }

        // Store handle
        self.actors.write().await.insert(actor_key, ActorHandle {
            shutdown_tx,
            integration_type: i_type,
        });

        // Save binding to DB (no token)
        if let Err(e) = self.save_binding(scope_type, scope_id, integration_type, &config) {
            return CommandResponse::err(format!("Failed to save binding: {}", e));
        }

        // Send status event
        let _ = self.event_tx.send(SwiftEvent::IntegrationStatus {
            scope_id: scope_key,
            integration_type: integration_type.to_string(),
            status: "connected".to_string(),
            message: None,
        });

        CommandResponse::ok()
    }

    async fn cmd_stop(&self, scope_id: &str, integration_type: &str) -> CommandResponse {
        // Try both scope types
        let keys = vec![
            format!("group:{}:{}", scope_id, integration_type),
            format!("workspace:{}:{}", scope_id, integration_type),
        ];

        let mut stopped = false;
        for key in keys {
            if let Some(handle) = self.actors.write().await.remove(&key) {
                let _ = handle.shutdown_tx.send(()).await;
                stopped = true;

                // Send status event
                let _ = self.event_tx.send(SwiftEvent::IntegrationStatus {
                    scope_id: scope_id.to_string(),
                    integration_type: integration_type.to_string(),
                    status: "stopped".to_string(),
                    message: None,
                });

                break;
            }
        }

        if stopped {
            CommandResponse::ok()
        } else {
            CommandResponse::err("Integration not running")
        }
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

    async fn cmd_get_events_by_ids(&self, event_ids: &[String]) -> CommandResponse {
        // This would search through recent events
        // For now, return empty - events are in Swift's local buffer
        CommandResponse::ok_with_data(serde_json::json!([]))
    }

    async fn cmd_is_running(&self, scope_id: &str, integration_type: &str) -> CommandResponse {
        let keys = vec![
            format!("group:{}:{}", scope_id, integration_type),
            format!("workspace:{}:{}", scope_id, integration_type),
        ];

        let running = {
            let actors = self.actors.read().await;
            keys.iter().any(|k| actors.contains_key(k))
        };

        CommandResponse::ok_with_data(serde_json::json!({ "running": running }))
    }

    // ========================================================================
    // Actor Spawning
    // ========================================================================

    async fn spawn_actor(
        &self,
        integration_type: IntegrationType,
        scope_id: String,
        token: String,
        config: serde_json::Value,
        mut shutdown_rx: mpsc::Receiver<()>,
    ) -> Result<(), String> {
        let event_tx = self.event_tx.clone();

        match integration_type {
            IntegrationType::Slack => {
                let channels: Vec<String> = config["channels"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                tokio::spawn(async move {
                    slack_actor_loop(scope_id, token, channels, event_tx, shutdown_rx).await;
                });
            }
            IntegrationType::Jira => {
                let domain = config["domain"].as_str().unwrap_or("").to_string();
                let email = config["email"].as_str().unwrap_or("").to_string();
                let projects: Vec<String> = config["projects"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                tokio::spawn(async move {
                    jira_actor_loop(scope_id, token, domain, email, projects, event_tx, shutdown_rx).await;
                });
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

                tokio::spawn(async move {
                    github_actor_loop(scope_id, token, repos, event_tx, shutdown_rx).await;
                });
            }
            IntegrationType::Confluence => {
                let domain = config["domain"].as_str().unwrap_or("").to_string();
                let email = config["email"].as_str().unwrap_or("").to_string();
                let spaces: Vec<String> = config["spaces"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                tokio::spawn(async move {
                    confluence_actor_loop(scope_id, token, domain, email, spaces, event_tx, shutdown_rx).await;
                });
            }
            IntegrationType::GoogleDocs => {
                let document_ids: Vec<String> = config["document_ids"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                tokio::spawn(async move {
                    googledocs_actor_loop(scope_id, token, document_ids, event_tx, shutdown_rx).await;
                });
            }
        }

        Ok(())
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
}

// ============================================================================
// Actor Loops (Replace with actual client implementations)
// ============================================================================

async fn slack_actor_loop(
    scope_id: String,
    token: String,
    channels: Vec<String>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    // TODO: Import and use actual SlackClient from cyan-integrations
    // let client = SlackClient::new(token);

    let mut interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Poll Slack for each channel
                for channel in &channels {
                    // TODO: Actual polling logic
                    // let messages = client.get_messages(channel, ...).await;
                    // for msg in messages {
                    //     let _ = event_tx.send(SwiftEvent::IntegrationEvent {
                    //         id: generate_event_id(&msg),
                    //         scope_id: scope_id.clone(),
                    //         source: "slack".to_string(),
                    //         summary: format_slack_summary(&msg),
                    //         context: format!("#{}", channel_name),
                    //         url: Some(msg.permalink),
                    //         ts: msg.ts,
                    //     });
                    // }

                    tracing::debug!("Polling Slack channel {} for scope {}", channel, scope_id);
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Slack actor shutting down for {}", scope_id);
                break;
            }
        }
    }
}

async fn jira_actor_loop(
    scope_id: String,
    token: String,
    domain: String,
    email: String,
    projects: Vec<String>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    // TODO: Import and use actual JiraClient from cyan-integrations
    // let client = JiraClient::new(domain, email, token);

    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for project in &projects {
                    // TODO: Actual polling logic
                    // let issues = client.search_issues(&format!("project = {}", project)).await;
                    // for issue in issues {
                    //     let _ = event_tx.send(SwiftEvent::IntegrationEvent {
                    //         id: generate_event_id(&issue),
                    //         scope_id: scope_id.clone(),
                    //         source: "jira".to_string(),
                    //         summary: format!("{} {}", issue.key, issue.fields.summary),
                    //         context: format!("@{}", issue.fields.assignee),
                    //         url: Some(issue.url),
                    //         ts: issue.updated_ts,
                    //     });
                    // }

                    tracing::debug!("Polling Jira project {} for scope {}", project, scope_id);
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Jira actor shutting down for {}", scope_id);
                break;
            }
        }
    }
}

async fn github_actor_loop(
    scope_id: String,
    token: String,
    repos: Vec<(String, String)>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    // TODO: Import and use actual GitHubClient from cyan-integrations
    // let client = GitHubClient::new(token);

    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for (owner, repo) in &repos {
                    // TODO: Actual polling logic
                    // let commits = client.list_commits(owner, repo, None).await;
                    // for commit in commits {
                    //     let _ = event_tx.send(SwiftEvent::IntegrationEvent {
                    //         id: commit.sha.clone(),
                    //         scope_id: scope_id.clone(),
                    //         source: "github".to_string(),
                    //         summary: commit.commit.message.lines().next().unwrap_or(""),
                    //         context: format!("PR #{}", pr_number),
                    //         url: Some(commit.html_url),
                    //         ts: commit.ts,
                    //     });
                    // }

                    tracing::debug!("Polling GitHub repo {}/{} for scope {}", owner, repo, scope_id);
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("GitHub actor shutting down for {}", scope_id);
                break;
            }
        }
    }
}

async fn confluence_actor_loop(
    scope_id: String,
    token: String,
    domain: String,
    email: String,
    spaces: Vec<String>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    // TODO: Import and use actual ConfluenceClient from cyan-integrations
    // let client = ConfluenceClient::new(domain, email, token);

    let mut interval = tokio::time::interval(Duration::from_secs(120));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for space in &spaces {
                    // TODO: Actual polling logic
                    tracing::debug!("Polling Confluence space {} for scope {}", space, scope_id);
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Confluence actor shutting down for {}", scope_id);
                break;
            }
        }
    }
}

async fn googledocs_actor_loop(
    scope_id: String,
    token: String,
    document_ids: Vec<String>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    mut shutdown_rx: mpsc::Receiver<()>,
) {
    // TODO: Import and use actual GoogleDocsClient from cyan-integrations
    // let client = GoogleDocsClient::new(token);

    let mut interval = tokio::time::interval(Duration::from_secs(120));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for doc_id in &document_ids {
                    // TODO: Actual polling logic
                    tracing::debug!("Polling Google Doc {} for scope {}", doc_id, scope_id);
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Google Docs actor shutting down for {}", scope_id);
                break;
            }
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}