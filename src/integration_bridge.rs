// src/integration_bridge.rs
//
// Bridges cyan-backend-integrations crate into Cyan's FFI layer.
//
// This module:
// 1. Wraps IntegrationManager from cyan-backend-integrations
// 2. Persists integration bindings to SQLite (no tokens)
// 3. Transforms IntegrationEvent → SwiftEvent::IntegrationGraph (force graph format)
// 4. Provides single FFI entry point: cyan_integration_command(json)
// 5. Prepares events for future QUIC broadcast to Cyan Lens

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    NodeStatus as CrateNodeStatus,
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
// Force Graph Types (for visualization)
// ============================================================================

/// Node in the force-directed graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub external_id: String,        // PROJ-123, #channel, PR #42
    pub kind: String,               // slack, jira, github_pr, confluence, gdocs
    pub title: Option<String>,
    pub summary: Option<String>,
    pub url: Option<String>,
    pub status: String,             // real, ephemeral, not_found
    pub metadata: HashMap<String, String>,
    pub mention_count: u32,
    pub last_activity: u64,
    pub created_at: u64,
}

/// Edge connecting two nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relation: String,           // mentions, implements, fixes, documents, links
    pub weight: f32,                // 0.0-1.0, affects line thickness
    pub context: Option<String>,    // Snippet showing connection
    pub created_at: u64,
}

/// The full graph payload sent to Swift
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationGraph {
    pub scope_id: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub focus_node_id: Option<String>,
    pub connected_integrations: Vec<String>,
    pub stats: GraphStats,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub total_nodes: u32,
    pub real_nodes: u32,
    pub ephemeral_nodes: u32,
    pub total_edges: u32,
    pub unlinked_messages: u32,
}

// ============================================================================
// Legacy Types (for backward compatibility during migration)
// ============================================================================

/// A mention edge - connects a source node to an anchor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionEdge {
    pub source_node_id: String,
    pub source_kind: String,
    pub author: String,
    pub summary: String,
    pub url: Option<String>,
    pub ts: u64,
    pub relation: String,
}

/// Legacy graph entry format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEntry {
    pub anchor_id: String,
    pub anchor_kind: String,
    pub anchor_status: String,
    pub anchor_title: Option<String>,
    pub anchor_url: Option<String>,
    pub mention_count: u32,
    pub mentions: Vec<MentionEdge>,
    pub first_seen: u64,
    pub last_activity: u64,
}

// ============================================================================
// Broadcast Event (for future QUIC to Cyan Lens)
// ============================================================================

/// Event prepared for broadcast to Cyan Lens cloud
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastEvent {
    pub event_type: String,
    pub scope_id: String,
    pub timestamp: u64,
    pub payload: BroadcastPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BroadcastPayload {
    #[serde(rename = "node_created")]
    NodeCreated { node: GraphNode },
    #[serde(rename = "node_updated")]
    NodeUpdated { node: GraphNode },
    #[serde(rename = "edge_created")]
    EdgeCreated { edge: GraphEdge },
    #[serde(rename = "graph_snapshot")]
    GraphSnapshot { graph: IntegrationGraph },
}

// ============================================================================
// Command/Response Types (JSON dispatch from Swift)
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum IntegrationCommand {
    Start {
        scope_type: String,
        scope_id: String,
        integration_type: String,
        token: String,
        config: serde_json::Value,
    },
    Stop {
        scope_id: String,
        integration_type: String,
    },
    RemoveBinding {
        scope_id: String,
        integration_type: String,
    },
    GetBindings {
        scope_type: Option<String>,
        scope_id: String,
    },
    IsRunning {
        scope_id: String,
        integration_type: String,
    },
    ListSlackChannels {
        scope_id: String,
    },
    /// Get the current graph for a scope
    GetGraph {
        scope_id: String,
    },
    /// Set focus node for graph visualization
    SetFocus {
        scope_id: String,
        node_id: Option<String>,
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
    manager: Arc<IntegrationManager>,
    running: RwLock<HashMap<String, RunningIntegration>>,
    sent_ids: RwLock<HashSet<String>>,
    last_graph_hash: RwLock<HashMap<String, u64>>,
    /// Focus node per scope
    focus_nodes: RwLock<HashMap<String, String>>,
    /// Broadcast queue for Cyan Lens (future QUIC)
    broadcast_queue: RwLock<Vec<BroadcastEvent>>,
}

struct RunningIntegration {
    scope_type: String,
    scope_id: String,
    integration_type: IntegrationType,
}

impl IntegrationBridge {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        event_tx: mpsc::UnboundedSender<SwiftEvent>,
    ) -> Self {
        Self::ensure_schema(&db);

        Self {
            db,
            event_tx,
            manager: Arc::new(IntegrationManager::new()),
            running: RwLock::new(HashMap::new()),
            sent_ids: RwLock::new(HashSet::new()),
            last_graph_hash: RwLock::new(HashMap::new()),
            focus_nodes: RwLock::new(HashMap::new()),
            broadcast_queue: RwLock::new(Vec::new()),
        }
    }

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

    /// Poll IntegrationManager for events and forward graph to Swift
    async fn forward_events(&self) {
        let running = self.running.read().await;

        if running.is_empty() {
            return;
        }

        for (_key, info) in running.iter() {
            let events = self.manager.get_all_events(&info.scope_id).await;
            let nodes = self.manager.get_all_nodes(&info.scope_id).await;

            // Build force graph (new format)
            let graph = self.build_force_graph(&events, &nodes, &info.scope_type, &info.scope_id).await;

            if graph.nodes.is_empty() {
                continue;
            }

            // Compute hash to detect changes
            let graph_hash = self.compute_graph_hash_v2(&graph);

            {
                let last_hashes = self.last_graph_hash.read().await;
                if let Some(&last) = last_hashes.get(&graph.scope_id) {
                    if last == graph_hash {
                        continue;
                    }
                }
            }

            {
                let mut last_hashes = self.last_graph_hash.write().await;
                last_hashes.insert(graph.scope_id.clone(), graph_hash);
            }

            // Queue for broadcast (future Cyan Lens)
            self.queue_broadcast(BroadcastEvent {
                event_type: "graph_update".to_string(),
                scope_id: graph.scope_id.clone(),
                timestamp: graph.updated_at,
                payload: BroadcastPayload::GraphSnapshot { graph: graph.clone() },
            }).await;

            let graph_json = serde_json::to_string(&graph).unwrap_or_default();

            tracing::info!(
                "📊 Sending force graph: {} nodes, {} edges for {}",
                graph.nodes.len(),
                graph.edges.len(),
                graph.scope_id
            );

            if let Err(e) = self.event_tx.send(SwiftEvent::IntegrationGraph {
                scope_id: graph.scope_id.clone(),
                graph_json,
            }) {
                tracing::error!("❌ Failed to send graph: {}", e);
            }
        }
    }

    /// Queue event for future QUIC broadcast to Cyan Lens
    async fn queue_broadcast(&self, event: BroadcastEvent) {
        let mut queue = self.broadcast_queue.write().await;
        queue.push(event);

        // Keep queue bounded
        if queue.len() > 1000 {
            queue.drain(0..500);
        }
    }

    /// Get queued broadcast events (for future QUIC sender)
    pub async fn drain_broadcast_queue(&self) -> Vec<BroadcastEvent> {
        let mut queue = self.broadcast_queue.write().await;
        std::mem::take(&mut *queue)
    }

    fn compute_graph_hash_v2(&self, graph: &IntegrationGraph) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        graph.nodes.len().hash(&mut hasher);
        graph.edges.len().hash(&mut hasher);
        for node in &graph.nodes {
            node.id.hash(&mut hasher);
            node.last_activity.hash(&mut hasher);
        }
        for edge in &graph.edges {
            edge.id.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Build force-directed graph structure
    async fn build_force_graph(
        &self,
        events: &[CrateIntegrationEvent],
        nodes: &[Node],
        scope_type: &str,
        scope_id: &str,
    ) -> IntegrationGraph {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let scope_key = format!("{}:{}", scope_type, scope_id);

        // Convert crate nodes to graph nodes
        let mut graph_nodes: HashMap<String, GraphNode> = HashMap::new();
        let mut graph_edges: Vec<GraphEdge> = Vec::new();
        let mut connected_integrations: HashSet<String> = HashSet::new();

        // First pass: create nodes from all Node objects
        for node in nodes {
            let kind = match node.kind {
                NodeKind::SlackMessage | NodeKind::SlackThread => "slack",
                NodeKind::JiraTicket => "jira",
                NodeKind::GitHubPR => "github_pr",
                NodeKind::GitHubIssue => "github_issue",
                NodeKind::GitHubCommit => "github_commit",
                NodeKind::ConfluencePage => "confluence",
                NodeKind::GoogleDoc => "gdocs",
                _ => "unknown",
            };

            connected_integrations.insert(kind.split('_').next().unwrap_or(kind).to_string());

            let status = match node.status {
                CrateNodeStatus::Real => "real",
                CrateNodeStatus::Ephemeral => "ephemeral",
                CrateNodeStatus::Resolved => "resolved",
                CrateNodeStatus::NotFound => "not_found",
                CrateNodeStatus::Error(_) => "error",
            };

            let mut metadata = HashMap::new();
            metadata.insert("author".to_string(), node.metadata.author.clone());
            if let Some(ref title) = node.metadata.title {
                metadata.insert("title".to_string(), title.clone());
            }
            if let Some(ref status) = node.metadata.status {
                metadata.insert("status".to_string(), status.clone());
            }

            let graph_node = GraphNode {
                id: node.id.clone(),
                external_id: node.external_id.clone(),
                kind: kind.to_string(),
                title: node.metadata.title.clone(),
                summary: Some(truncate_str(&node.content, 200).to_string()),
                url: if node.metadata.url.is_empty() { None } else { Some(node.metadata.url.clone()) },
                status: status.to_string(),
                metadata,
                mention_count: node.metadata.mention_count,
                last_activity: node.last_updated,
                created_at: node.first_seen,
            };

            graph_nodes.insert(node.id.clone(), graph_node);
        }

        // Second pass: create edges from events
        for event in events {
            let payload: serde_json::Value = serde_json::from_str(&event.base.payload)
                .unwrap_or(serde_json::json!({}));

            let event_type = payload.get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            // Get target node info from event
            let (target_external_id, target_kind, relation) = match event_type {
                "jira_mention" | "jira_reference" => {
                    let jira_id = payload.get("jira_id")
                        .or_else(|| payload.get("jira_key"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (jira_id, "jira", "mentions")
                }
                "github_pr_reference" => {
                    let pr_num = payload.get("pr_number")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    (format!("PR #{}", pr_num), "github_pr", "links")
                }
                "github_issue_reference" => {
                    let issue_num = payload.get("issue_number")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    (format!("Issue #{}", issue_num), "github_issue", "links")
                }
                "commit_fixes_issue" => {
                    let jira_key = payload.get("jira_key")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    (jira_key.to_string(), "jira", "fixes")
                }
                "confluence_reference" => {
                    let page_id = payload.get("page_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    (page_id.to_string(), "confluence", "documents")
                }
                _ => continue,
            };

            if target_external_id.is_empty() {
                continue;
            }

            // Ensure target node exists (create ephemeral if needed)
            let target_node_id = blake3::hash(
                format!("{}:{}", target_kind, target_external_id).as_bytes()
            ).to_hex().to_string();

            if !graph_nodes.contains_key(&target_node_id) {
                // Create ephemeral node for the referenced entity
                let ephemeral_node = GraphNode {
                    id: target_node_id.clone(),
                    external_id: target_external_id.clone(),
                    kind: target_kind.to_string(),
                    title: None,
                    summary: None,
                    url: None,
                    status: "ephemeral".to_string(),
                    metadata: HashMap::new(),
                    mention_count: 1,
                    last_activity: event.base.ts,
                    created_at: event.discovered_at,
                };
                graph_nodes.insert(target_node_id.clone(), ephemeral_node);
            } else {
                // Update mention count
                if let Some(node) = graph_nodes.get_mut(&target_node_id) {
                    node.mention_count += 1;
                    node.last_activity = node.last_activity.max(event.base.ts);
                }
            }

            // Get context snippet
            let context = payload.get("context")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Create edge
            let edge_id = format!("{}:{}:{}", event.base.source, target_node_id, relation);
            let edge = GraphEdge {
                id: edge_id,
                source_id: event.base.source.clone(),
                target_id: target_node_id,
                relation: relation.to_string(),
                weight: event.confidence as f32,
                context,
                created_at: event.discovered_at,
            };

            graph_edges.push(edge);
        }

        // Deduplicate edges (increase weight for repeated references)
        let mut edge_map: HashMap<String, GraphEdge> = HashMap::new();
        for edge in graph_edges {
            let key = format!("{}:{}:{}", edge.source_id, edge.target_id, edge.relation);
            if let Some(existing) = edge_map.get_mut(&key) {
                existing.weight = (existing.weight + 0.1).min(1.0);
            } else {
                edge_map.insert(key, edge);
            }
        }

        // Count stats
        let nodes_vec: Vec<GraphNode> = graph_nodes.into_values().collect();
        let edges_vec: Vec<GraphEdge> = edge_map.into_values().collect();

        let real_count = nodes_vec.iter().filter(|n| n.status == "real").count() as u32;
        let ephemeral_count = nodes_vec.iter().filter(|n| n.status == "ephemeral").count() as u32;

        // Count unlinked slack messages
        let linked_node_ids: HashSet<String> = edges_vec.iter()
            .flat_map(|e| vec![e.source_id.clone(), e.target_id.clone()])
            .collect();
        let unlinked = nodes_vec.iter()
            .filter(|n| n.kind == "slack" && !linked_node_ids.contains(&n.id))
            .count() as u32;

        // Get focus node
        let focus_node_id = self.focus_nodes.read().await.get(&scope_key).cloned();
        let edges_vec_clone = edges_vec.clone();
        IntegrationGraph {
            scope_id: scope_key,
            nodes: nodes_vec,
            edges: edges_vec,
            focus_node_id,
            connected_integrations: connected_integrations.into_iter().collect(),
            stats: GraphStats {
                total_nodes: real_count + ephemeral_count,
                real_nodes: real_count,
                ephemeral_nodes: ephemeral_count,
                total_edges: edges_vec_clone.len() as u32,
                unlinked_messages: unlinked,
            },
            updated_at: now,
        }
    }

    pub async fn handle_command(&self, json: &str) -> String {
        let response = match serde_json::from_str::<IntegrationCommand>(json) {
            Ok(cmd) => self.dispatch(cmd).await,
            Err(e) => CommandResponse::err(format!("Invalid command JSON: {}", e)),
        };

        serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"success":false,"error":"Serialization failed"}"#.to_string()
        })
    }

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
            IntegrationCommand::GetGraph { scope_id } => {
                self.cmd_get_graph(&scope_id).await
            }
            IntegrationCommand::SetFocus { scope_id, node_id } => {
                self.cmd_set_focus(&scope_id, node_id).await
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
        let i_type = match IntegrationType::from_str(integration_type) {
            Some(t) => t,
            None => return CommandResponse::err(format!("Unknown integration type: {}", integration_type)),
        };

        if scope_type != "group" && scope_type != "workspace" {
            return CommandResponse::err("scope_type must be 'group' or 'workspace'");
        }

        let actor_key = format!("{}:{}", scope_id, integration_type);

        if self.running.read().await.contains_key(&actor_key) {
            return CommandResponse::err("Integration already running for this scope");
        }

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

        self.running.write().await.insert(actor_key, RunningIntegration {
            scope_type: scope_type.to_string(),
            scope_id: scope_id.to_string(),
            integration_type: i_type.clone(),
        });

        if let Err(e) = self.save_binding(scope_type, scope_id, integration_type, &config) {
            return CommandResponse::err(format!("Failed to save binding: {}", e));
        }

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

        let was_running = self.running.write().await.remove(&actor_key).is_some();

        if !was_running {
            return CommandResponse::err("Integration not running");
        }

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

        let _ = self.event_tx.send(SwiftEvent::IntegrationStatus {
            scope_id: scope_id.to_string(),
            integration_type: integration_type.to_string(),
            status: "stopped".to_string(),
            message: None,
        });

        CommandResponse::ok()
    }

    async fn cmd_remove_binding(&self, scope_id: &str, integration_type: &str) -> CommandResponse {
        let _ = self.cmd_stop(scope_id, integration_type).await;

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

    async fn cmd_list_slack_channels(&self, _scope_id: &str) -> CommandResponse {
        CommandResponse::err("List channels via OAuth flow in Swift, not here")
    }

    async fn cmd_get_graph(&self, scope_id: &str) -> CommandResponse {
        // Find running integration for this scope
        let running = self.running.read().await;

        let info = running.values()
            .find(|r| r.scope_id == scope_id || format!("{}:{}", r.scope_type, r.scope_id) == scope_id);

        match info {
            Some(info) => {
                let events = self.manager.get_all_events(&info.scope_id).await;
                let nodes = self.manager.get_all_nodes(&info.scope_id).await;
                let graph = self.build_force_graph(&events, &nodes, &info.scope_type, &info.scope_id).await;

                CommandResponse::ok_with_data(serde_json::to_value(&graph).unwrap_or_default())
            }
            None => CommandResponse::err("No integration running for this scope")
        }
    }

    async fn cmd_set_focus(&self, scope_id: &str, node_id: Option<String>) -> CommandResponse {
        let mut focus_nodes = self.focus_nodes.write().await;

        if let Some(id) = node_id {
            focus_nodes.insert(scope_id.to_string(), id);
        } else {
            focus_nodes.remove(scope_id);
        }

        CommandResponse::ok()
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

    /// Called on app startup to restore integrations.
    pub async fn restore_integration(
        &self,
        scope_type: &str,
        scope_id: &str,
        integration_type: &str,
        token: &str,
    ) -> Result<(), String> {
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