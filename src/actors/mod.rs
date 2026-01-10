// src/actors/mod.rs
//
// Actor system for Cyan networking
//
// Architecture:
//   NetworkActor (coordinator)
//   ├── DiscoveryActor (peer discovery, mesh formation)
//   ├── TopicActor[group_id] (gossip per group)
//   └── DM streams (peer-to-peer QUIC)

use iroh_gossip::proto::TopicId;

pub mod discovery_actor;
pub mod network_actor;
pub mod topic_actor;

// Re-exports
pub use discovery_actor::{DiscoveryActor, DiscoveryCommand, DiscoveryNetworkCmd};
pub use network_actor::{DirectMessage, DmAttachment, NetworkActor, DM_ALPN, FILE_TRANSFER_ALPN, SNAPSHOT_ALPN};
pub use topic_actor::{TopicActor, TopicCommand};

// ═══════════════════════════════════════════════════════════════════════════
// COMMON ACTOR TYPES
// ═══════════════════════════════════════════════════════════════════════════

/// System-level commands that all actors understand
#[derive(Debug, Clone)]
pub enum SystemCommand {
    /// Graceful shutdown
    PoisonPill,
    /// Dump diagnostics to logs
    DumpDiagnostics,
    /// Pull in-flight operations (for debugging)
    PullInFlight,
}

/// Wrapper for actor messages - either system or domain-specific
#[derive(Debug)]
pub enum ActorMessage<C> {
    System(SystemCommand),
    Domain(C),
}

/// Handle to a spawned actor
pub struct ActorHandle<C> {
    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<ActorMessage<C>>,
    pub join_handle: tokio::task::JoinHandle<()>,
}

// ═══════════════════════════════════════════════════════════════════════════
// TOPIC ID HELPERS
// ═══════════════════════════════════════════════════════════════════════════

/// Create a TopicId from a string (group_id or topic name)
pub fn make_topic_id(topic: &str) -> anyhow::Result<TopicId> {
    let hash = blake3::hash(topic.as_bytes());
    let bytes: [u8; 32] = hash.as_bytes()[..32].try_into()?;
    Ok(TopicId::from_bytes(bytes))
}

/// Create a group topic ID following the cyan naming convention
pub fn make_group_topic_id(group_id: &str) -> anyhow::Result<TopicId> {
    let topic_str = format!("cyan/group/{}", group_id);
    make_topic_id(&topic_str)
}

/// Create a discovery topic ID
pub fn make_discovery_topic_id(discovery_key: &str) -> anyhow::Result<TopicId> {
    let topic_str = format!("cyan/discovery/{}", discovery_key);
    make_topic_id(&topic_str)
}

// ═══════════════════════════════════════════════════════════════════════════
// TOPIC → NETWORK COMMANDS
// ═══════════════════════════════════════════════════════════════════════════

/// Commands from TopicActor to NetworkActor (for snapshot coordination)
#[derive(Debug, Clone)]
pub enum TopicNetworkCmd {
    /// TopicActor needs a snapshot for this group
    NeedSnapshot { group_id: String },
    /// Snapshot download completed successfully
    SnapshotComplete { group_id: String },
    /// Snapshot download failed (will retry)
    SnapshotFailed { group_id: String, reason: String },
}

// ═══════════════════════════════════════════════════════════════════════════
// TYPE ALIASES FOR CONVENIENCE
// ═══════════════════════════════════════════════════════════════════════════

pub type DiscoveryActorHandle = ActorHandle<DiscoveryCommand>;
pub type TopicActorHandle = ActorHandle<TopicCommand>;