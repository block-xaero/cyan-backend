// src/lens_bridge.rs
//
// Bridges integration Nodes to Cyan Lens via XaeroFlux gossip.
// Converts Node → RawEvent JSON and sends via XaeroFlux event channel.
//
// Usage:
//   let bridge = LensBridge::new(group_id, xaeroflux_tx);
//   bridge.forward_node(&node)?;
//
// The RawEvent format matches what cyan-lens expects in its ingestion pipeline.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::UnboundedSender;

use cyan_backend_integrations::{Node, NodeKind, NodeStatus};

// ============================================================================
// RawEvent - matches cyan-lens/src/models/mod.rs
// ============================================================================

/// Source integration type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Slack,
    Jira,
    GitHub,
    Confluence,
    GoogleDocs,
}

/// Content type within a source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    SlackMessage,
    SlackThread,
    JiraTicket,
    JiraComment,
    GithubPr,
    GithubIssue,
    GithubComment,
    GithubCommit,
    ConfluencePage,
    GoogleDoc,
    GoogleSheet,
    GoogleSlide,
}

/// Raw event for Cyan Lens ingestion pipeline.
/// This is serialized to JSON and sent as the XaeroFlux Event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    /// Unique event ID (blake3 hash from Node)
    pub id: String,
    
    /// Group ID (maps to discovery_key / organization)
    pub group_id: String,
    
    /// Workspace ID (channel, project, repo)
    pub workspace_id: String,
    
    /// Source integration
    pub source: SourceKind,
    
    /// Type of content
    pub content_kind: ContentKind,
    
    /// External ID (Slack ts, Jira key, PR number)
    pub external_id: String,
    
    /// The actual content text - THIS IS WHAT LLM ANALYZES
    pub content: String,
    
    /// Author's integration-specific ID
    pub author_id: String,
    
    /// Author's display name
    pub author_name: String,
    
    /// URL to the content in the source system
    pub url: String,
    
    /// Title (for tickets, PRs, pages)
    pub title: Option<String>,
    
    /// Thread ID (for threaded messages)
    pub thread_id: Option<String>,
    
    /// Parent ID (for replies, comments)
    pub parent_id: Option<String>,
    
    /// Original timestamp from source
    pub ts: u64,
    
    /// When we captured this event
    pub captured_at: u64,
}

// ============================================================================
// XaeroFlux Event (compatible with xaeroflux::Event)
// ============================================================================

/// XaeroFlux-compatible event wrapper.
/// The payload contains serialized RawEvent JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XfEvent {
    pub id: String,
    pub payload: String,
    pub source: String,
    pub ts: u64,
}

// ============================================================================
// LensBridge
// ============================================================================

/// Bridge that forwards integration Nodes to Cyan Lens.
/// 
/// Converts Node → RawEvent and sends via XaeroFlux gossip channel.
pub struct LensBridge {
    group_id: String,
    xaeroflux_tx: UnboundedSender<XfEvent>,
    forwarded_count: AtomicU64,
    skipped_count: AtomicU64,
}

impl LensBridge {
    /// Create a new LensBridge.
    /// 
    /// # Arguments
    /// * `group_id` - The group/organization ID (maps to discovery_key)
    /// * `xaeroflux_tx` - Channel to send events to XaeroFlux
    pub fn new(group_id: String, xaeroflux_tx: UnboundedSender<XfEvent>) -> Self {
        tracing::info!("🌉 LensBridge created for group: {}", group_id);
        Self {
            group_id,
            xaeroflux_tx,
            forwarded_count: AtomicU64::new(0),
            skipped_count: AtomicU64::new(0),
        }
    }

    /// Forward a Node to Cyan Lens via XaeroFlux.
    /// 
    /// Skips ephemeral nodes and nodes with empty content.
    /// Returns Ok(true) if forwarded, Ok(false) if skipped.
    pub fn forward_node(&self, node: &Node) -> Result<bool, String> {
        // Skip ephemeral nodes - they have no content yet
        if node.status == NodeStatus::Ephemeral {
            self.skipped_count.fetch_add(1, Ordering::Relaxed);
            tracing::debug!("Skipping ephemeral node: {}", node.external_id);
            return Ok(false);
        }

        // Skip empty content
        if node.content.trim().is_empty() {
            self.skipped_count.fetch_add(1, Ordering::Relaxed);
            tracing::debug!("Skipping empty content node: {}", node.external_id);
            return Ok(false);
        }

        let raw_event = self.node_to_raw_event(node);
        let payload = serde_json::to_string(&raw_event)
            .map_err(|e| format!("Serialize error: {}", e))?;

        let xf_event = XfEvent {
            id: raw_event.id.clone(),
            payload,
            source: "cyan-backend".to_string(),
            ts: raw_event.ts,
        };

        self.xaeroflux_tx.send(xf_event)
            .map_err(|e| format!("XaeroFlux send error: {}", e))?;

        self.forwarded_count.fetch_add(1, Ordering::Relaxed);
        
        tracing::debug!(
            "📤 Forwarded {} {} to Lens: {}",
            Self::kind_str(&node.kind),
            node.external_id,
            &raw_event.id[..8.min(raw_event.id.len())]
        );
        
        Ok(true)
    }

    /// Batch forward multiple nodes.
    /// Returns count of successfully forwarded nodes.
    pub fn forward_nodes(&self, nodes: &[Node]) -> usize {
        let mut count = 0;
        for node in nodes {
            match self.forward_node(node) {
                Ok(true) => count += 1,
                Ok(false) => {} // skipped
                Err(e) => tracing::warn!("Failed to forward node {}: {}", node.external_id, e),
            }
        }
        
        if count > 0 {
            tracing::info!("📤 Forwarded {} nodes to Cyan Lens", count);
        }
        
        count
    }

    /// Get count of forwarded events (for metrics)
    pub fn forwarded_count(&self) -> u64 {
        self.forwarded_count.load(Ordering::Relaxed)
    }

    /// Get count of skipped events (for metrics)
    pub fn skipped_count(&self) -> u64 {
        self.skipped_count.load(Ordering::Relaxed)
    }

    /// Convert a Node to RawEvent
    fn node_to_raw_event(&self, node: &Node) -> RawEvent {
        let (source, content_kind) = Self::map_node_kind(&node.kind);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        RawEvent {
            id: node.id.clone(),
            group_id: self.group_id.clone(),
            workspace_id: node.workspace_id.clone(),
            source,
            content_kind,
            external_id: node.external_id.clone(),
            content: node.content.clone(),
            author_id: node.metadata.author.clone(),
            author_name: node.metadata.author.clone(), // Use author as name for now
            url: node.metadata.url.clone(),
            title: node.metadata.title.clone(),
            thread_id: None, // TODO: extract from node metadata if available
            parent_id: None, // TODO: extract from node metadata if available
            ts: node.ts,
            captured_at: now,
        }
    }

    /// Map NodeKind to (SourceKind, ContentKind)
    fn map_node_kind(kind: &NodeKind) -> (SourceKind, ContentKind) {
        match kind {
            NodeKind::SlackMessage => (SourceKind::Slack, ContentKind::SlackMessage),
            NodeKind::SlackThread => (SourceKind::Slack, ContentKind::SlackThread),
            NodeKind::JiraTicket => (SourceKind::Jira, ContentKind::JiraTicket),
            NodeKind::JiraComment => (SourceKind::Jira, ContentKind::JiraComment),
            NodeKind::GitHubPR => (SourceKind::GitHub, ContentKind::GithubPr),
            NodeKind::GitHubIssue => (SourceKind::GitHub, ContentKind::GithubIssue),
            NodeKind::GitHubCommit => (SourceKind::GitHub, ContentKind::GithubCommit),
            NodeKind::GitHubRepo => (SourceKind::GitHub, ContentKind::GithubIssue), // fallback
            NodeKind::ConfluencePage => (SourceKind::Confluence, ContentKind::ConfluencePage),
            NodeKind::GoogleDoc => (SourceKind::GoogleDocs, ContentKind::GoogleDoc),
            NodeKind::GoogleSheet => (SourceKind::GoogleDocs, ContentKind::GoogleSheet),
            NodeKind::GoogleSlide => (SourceKind::GoogleDocs, ContentKind::GoogleSlide),
            NodeKind::GenericUrl => (SourceKind::Slack, ContentKind::SlackMessage), // fallback
        }
    }

    /// Get string representation of NodeKind
    fn kind_str(kind: &NodeKind) -> &'static str {
        match kind {
            NodeKind::SlackMessage => "slack_message",
            NodeKind::SlackThread => "slack_thread",
            NodeKind::JiraTicket => "jira_ticket",
            NodeKind::JiraComment => "jira_comment",
            NodeKind::GitHubPR => "github_pr",
            NodeKind::GitHubIssue => "github_issue",
            NodeKind::GitHubCommit => "github_commit",
            NodeKind::GitHubRepo => "github_repo",
            NodeKind::ConfluencePage => "confluence_page",
            NodeKind::GoogleDoc => "google_doc",
            NodeKind::GoogleSheet => "google_sheet",
            NodeKind::GoogleSlide => "google_slide",
            NodeKind::GenericUrl => "generic_url",
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyan_backend_integrations::NodeMetadata;

    fn make_test_node() -> Node {
        Node {
            id: "test123abc".to_string(),
            workspace_id: "ws_1".to_string(),
            external_id: "1706123456.789".to_string(),
            name: "Test Message".to_string(),
            kind: NodeKind::SlackMessage,
            content: "Hey @bob can you review PROJ-123?".to_string(),
            metadata: NodeMetadata {
                author: "alice".to_string(),
                url: "https://slack.com/archives/C123/p1706123456789".to_string(),
                title: Some("Hey @bob can you review...".to_string()),
                status: None,
                mentioned_by: vec![],
                mention_count: 0,
            },
            status: NodeStatus::Real,
            ts: 1706123456,
            first_seen: 1706123456,
            last_updated: 1706123456,
        }
    }

    #[test]
    fn test_node_to_raw_event() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = LensBridge::new("test-group".to_string(), tx);
        
        let node = make_test_node();
        let raw = bridge.node_to_raw_event(&node);

        assert_eq!(raw.id, "test123abc");
        assert_eq!(raw.group_id, "test-group");
        assert_eq!(raw.workspace_id, "ws_1");
        assert_eq!(raw.source, SourceKind::Slack);
        assert_eq!(raw.content_kind, ContentKind::SlackMessage);
        assert_eq!(raw.content, "Hey @bob can you review PROJ-123?");
        assert_eq!(raw.author_id, "alice");
    }

    #[test]
    fn test_skip_ephemeral_node() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = LensBridge::new("test-group".to_string(), tx);
        
        let mut node = make_test_node();
        node.status = NodeStatus::Ephemeral;
        node.content = "".to_string();

        let result = bridge.forward_node(&node);
        assert!(matches!(result, Ok(false)));
        assert_eq!(bridge.skipped_count(), 1);
    }

    #[test]
    fn test_skip_empty_content() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = LensBridge::new("test-group".to_string(), tx);
        
        let mut node = make_test_node();
        node.content = "   ".to_string(); // whitespace only

        let result = bridge.forward_node(&node);
        assert!(matches!(result, Ok(false)));
    }

    #[test]
    fn test_map_node_kinds() {
        assert_eq!(
            LensBridge::map_node_kind(&NodeKind::SlackMessage),
            (SourceKind::Slack, ContentKind::SlackMessage)
        );
        assert_eq!(
            LensBridge::map_node_kind(&NodeKind::JiraTicket),
            (SourceKind::Jira, ContentKind::JiraTicket)
        );
        assert_eq!(
            LensBridge::map_node_kind(&NodeKind::GitHubPR),
            (SourceKind::GitHub, ContentKind::GithubPr)
        );
    }
}
