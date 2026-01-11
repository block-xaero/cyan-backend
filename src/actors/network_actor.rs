// src/actors/network_actor.rs
//
// NetworkActor - Central network coordinator
//
// Responsibilities:
// - Owns endpoint, gossip
// - Spawns and manages DiscoveryActor
// - Spawns and manages TopicActors (one per group)
// - Handles DM streams (peer-to-peer QUIC)
// - Routes NetworkCommand from FFI layer
// - Receives TopicNetworkCmd from TopicActors (snapshot coordination)
// - Broadcasts profile on startup

use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use iroh::discovery::mdns::MdnsDiscovery;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey,
};
use iroh_gossip::net::Gossip;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::models::protocol::FileTransferMsg;
use crate::{
    actors::{
        discovery_actor::{DiscoveryActor, DiscoveryCommand, DiscoveryNetworkCmd},
        topic_actor::{TopicActor, TopicCommand},
        ActorHandle, ActorMessage, SystemCommand, TopicNetworkCmd,
    },
    bootstrap_node_id,
    models::{
        commands::NetworkCommand,
        events::{NetworkEvent, SwiftEvent},
    },
    storage, DISCOVERY_KEY, RELAY_URL,
};
// ═══════════════════════════════════════════════════════════════════════════
// ALPN PROTOCOLS
// ═══════════════════════════════════════════════════════════════════════════

pub const FILE_TRANSFER_ALPN: &[u8] = b"cyan-file-v2";
pub const SNAPSHOT_ALPN: &[u8] = b"cyan-snapshot-v1";
pub const DM_ALPN: &[u8] = b"cyan-dm-v1";

// ═══════════════════════════════════════════════════════════════════════════
// PROTOCOL HANDLERS (for Router integration)
// ═══════════════════════════════════════════════════════════════════════════

/// Snapshot protocol handler - accepts incoming snapshot requests
#[derive(Debug, Clone)]
pub struct SnapshotHandler {
    node_id: String,
    event_tx: UnboundedSender<SwiftEvent>,
}

impl ProtocolHandler for SnapshotHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let peer_id = conn.remote_id().to_string();
        eprintln!("═══════════════════════════════════════════════════════════════════");
        eprintln!("📥 [SNAPSHOT] Incoming request from {}...", &peer_id[..16]);
        eprintln!("═══════════════════════════════════════════════════════════════════");
        tracing::info!("📥 [SNAPSHOT] Incoming snapshot request from {}", &peer_id[..16]);

        if let Err(e) = handle_snapshot_server(conn, self.node_id.clone(), self.event_tx.clone()).await {
            eprintln!("🔴 [SNAPSHOT] Transfer error: {}", e);
            tracing::error!("🔴 Snapshot transfer error: {}", e);
        }
        Ok(())
    }
}

/// File transfer protocol handler - accepts incoming file requests
#[derive(Debug, Clone)]
pub struct FileTransferHandler {
    event_tx: UnboundedSender<SwiftEvent>,
}

impl ProtocolHandler for FileTransferHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let peer_id = conn.remote_id().to_string();
        eprintln!("📥 [FILE] Transfer request from {}...", &peer_id[..16]);
        tracing::info!("📥 [FILE] Incoming file transfer from {}", &peer_id[..16]);

        if let Err(e) = handle_file_transfer_server(conn, self.event_tx.clone()).await {
            eprintln!("🔴 [FILE] Transfer error: {}", e);
            tracing::error!("🔴 File transfer error: {}", e);
        }
        Ok(())
    }
}

/// DM protocol handler - accepts incoming DM connections
#[derive(Debug, Clone)]
pub struct DmHandler {
    dm_senders: Arc<std::sync::Mutex<HashMap<String, UnboundedSender<DirectMessage>>>>,
    event_tx: UnboundedSender<SwiftEvent>,
}

impl ProtocolHandler for DmHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let peer_id = conn.remote_id().to_string();
        eprintln!("💬 [DM] Incoming connection from {}...", &peer_id[..16]);
        tracing::info!("💬 [DM] Incoming connection from {}", &peer_id[..16]);

        if let Err(e) = handle_dm_stream(conn, peer_id.clone(), self.dm_senders.clone(), self.event_tx.clone()).await {
            eprintln!("🔴 [DM] Connection error with {}: {}", &peer_id[..16], e);
            tracing::error!("🔴 DM connection error: {}", e);
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DM WIRE PROTOCOL
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessage {
    pub id: String,
    pub workspace_id: Option<String>,
    pub message: String,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub attachment: Option<DmAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmAttachment {
    pub file_id: String,
    pub name: String,
    pub hash: String,
    pub size: u64,
}

// ═══════════════════════════════════════════════════════════════════════════
// NETWORK ACTOR
// ═══════════════════════════════════════════════════════════════════════════

pub struct NetworkActor {
    node_id: String,
    endpoint: Endpoint,
    gossip: Arc<Gossip>,
    #[allow(dead_code)]
    router: Router,

    /// Discovery actor handle
    discovery_handle: Option<ActorHandle<DiscoveryCommand>>,

    /// Topic actors: group_id → handle
    topics: HashMap<String, ActorHandle<TopicCommand>>,

    /// DM stream senders: peer_id → sender (Arc<Mutex> for sharing with acceptor)
    dm_senders: Arc<std::sync::Mutex<HashMap<String, UnboundedSender<DirectMessage>>>>,

    /// Peers per group (shared with FFI for queries)
    peers_per_group: Arc<std::sync::Mutex<HashMap<String, HashSet<PublicKey>>>>,

    /// Event channel to Swift
    event_tx: UnboundedSender<SwiftEvent>,

    /// Channel to receive commands from DiscoveryActor
    discovery_rx: UnboundedReceiver<DiscoveryNetworkCmd>,

    /// Sender half (clone given to DiscoveryActor)
    #[allow(dead_code)]
    discovery_tx: UnboundedSender<DiscoveryNetworkCmd>,

    /// Channel to receive snapshot coordination from TopicActors
    topic_network_rx: UnboundedReceiver<TopicNetworkCmd>,

    /// Sender half (clone given to TopicActors)
    topic_network_tx: UnboundedSender<TopicNetworkCmd>,

    /// Groups currently needing snapshot sync
    groups_needing_snapshot: HashSet<String>,
}

impl NetworkActor {
    pub async fn new(
        secret_key: SecretKey,
        event_tx: UnboundedSender<SwiftEvent>,
        peers_per_group: Arc<std::sync::Mutex<HashMap<String, HashSet<PublicKey>>>>,
    ) -> Result<Self> {
        let node_id = secret_key.public().to_string();
        tracing::info!("🌐 [NET] Creating NetworkActor for node {}", &node_id[..16]);

        // Configure relay mode
        let relay_mode = if let Some(url_str) = RELAY_URL.get() {
            match RelayUrl::from_str(url_str) {
                Ok(url) => {
                    tracing::info!("🌐 [NET] Using custom relay: {}", url);
                    RelayMode::Custom(RelayMap::from(url))
                }
                Err(e) => {
                    tracing::warn!("⚠️ [NET] Invalid relay URL '{}': {}, using default", url_str, e);
                    RelayMode::Default
                }
            }
        } else {
            tracing::info!("🌐 [NET] Using default Iroh relays");
            RelayMode::Default
        };

        // DM senders map (shared between struct and DM handler)
        let dm_senders = Arc::new(std::sync::Mutex::new(HashMap::new()));

        // Build endpoint with all ALPNs
        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .alpns(vec![
                iroh_gossip::ALPN.to_vec(),
                FILE_TRANSFER_ALPN.to_vec(),
                SNAPSHOT_ALPN.to_vec(),
                DM_ALPN.to_vec(),
            ])
            .relay_mode(relay_mode)
            .discovery(MdnsDiscovery::builder())
            .bind()
            .await?;

        tracing::info!("✅ [NET] Endpoint bound: {}", &node_id[..16]);

        // Create gossip
        let gossip = Arc::new(Gossip::builder().spawn(endpoint.clone()));

        // Create protocol handlers
        let snapshot_handler = SnapshotHandler {
            node_id: node_id.clone(),
            event_tx: event_tx.clone(),
        };

        let file_handler = FileTransferHandler {
            event_tx: event_tx.clone(),
        };

        let dm_handler = DmHandler {
            dm_senders: dm_senders.clone(),
            event_tx: event_tx.clone(),
        };

        // Setup router with ALL protocols
        let router = Router::builder(endpoint.clone())
            .accept(iroh_gossip::ALPN, gossip.clone())
            .accept(SNAPSHOT_ALPN, snapshot_handler)
            .accept(FILE_TRANSFER_ALPN, file_handler)
            .accept(DM_ALPN, dm_handler)
            .spawn();

        tracing::info!("✅ [NET] Router spawned with gossip + snapshot + file + dm");

        // Create channel for DiscoveryActor → NetworkActor communication
        let (discovery_tx, discovery_rx) = mpsc::unbounded_channel();

        // Create channel for TopicActor → NetworkActor communication (snapshot coordination)
        let (topic_network_tx, topic_network_rx) = mpsc::unbounded_channel();

        Ok(Self {
            node_id,
            endpoint,
            gossip,
            router,
            discovery_handle: None,
            topics: HashMap::new(),
            dm_senders,
            peers_per_group,
            event_tx,
            discovery_rx,
            discovery_tx,
            topic_network_rx,
            topic_network_tx,
            groups_needing_snapshot: HashSet::new(),
        })
    }

    /// Start the network actor - spawns discovery and runs command loop
    pub async fn start(mut self, mut cmd_rx: UnboundedReceiver<NetworkCommand>) {
        eprintln!("🚀 [NET] ════════════════════════════════════════════════════════════");
        eprintln!("🚀 [NET] NetworkActor STARTING - node: {}", &self.node_id[..16]);
        eprintln!("🚀 [NET] ════════════════════════════════════════════════════════════");
        tracing::info!("🚀 [NET] Starting NetworkActor");

        // Spawn DiscoveryActor
        let discovery_key = DISCOVERY_KEY
            .get()
            .cloned()
            .unwrap_or_else(|| "cyan-dev".to_string());

        eprintln!("🔍 [NET] Spawning DiscoveryActor with key: {}", discovery_key);

        match DiscoveryActor::spawn(
            self.node_id.clone(),
            discovery_key,
            self.gossip.clone(),
            self.discovery_tx.clone(),
            self.event_tx.clone(),
        ).await {
            Ok(handle) => {
                eprintln!("✅ [NET] DiscoveryActor spawned successfully");
                tracing::info!("✅ [NET] DiscoveryActor spawned");
                self.discovery_handle = Some(handle);
            }
            Err(e) => {
                eprintln!("🔴 [NET] FAILED to spawn DiscoveryActor: {}", e);
                tracing::error!("🔴 [NET] Failed to spawn DiscoveryActor: {}", e);
            }
        }

        // Load existing groups and spawn topic actors
        let existing_groups = storage::group_list_ids();
        eprintln!("📂 [NET] Loading {} existing groups from DB", existing_groups.len());
        for group_id in existing_groups {
            eprintln!("   → Spawning TopicActor for group: {}...", &group_id[..16.min(group_id.len())]);
            if let Err(e) = self.spawn_topic_actor(&group_id, vec![]).await {
                eprintln!("🔴 [NET] FAILED to spawn TopicActor for {}: {}", &group_id[..16.min(group_id.len())], e);
                tracing::error!(
                    "🔴 [NET] Failed to spawn TopicActor for {}: {}",
                    &group_id[..16.min(group_id.len())],
                    e
                );
            }
        }

        // NOTE: Router handles DM, file, and snapshot acceptance now
        // No need for spawn_dm_acceptor or spawn_protocol_acceptor

        // Broadcast our profile on startup
        self.broadcast_profile_to_all_groups();

        eprintln!("🟢 [NET] NetworkActor READY - entering main loop");

        // Main run loop
        self.run_loop(cmd_rx).await;
    }

    async fn run_loop(&mut self, mut cmd_rx: UnboundedReceiver<NetworkCommand>) {
        tracing::info!("🔄 [NET] Entering main run loop");

        loop {
            tokio::select! {
                // Commands from FFI/app layer
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(network_cmd) => {
                            self.handle_network_command(network_cmd).await;
                        }
                        None => {
                            tracing::info!("🛑 [NET] Command channel closed");
                            break;
                        }
                    }
                }

                // Commands from DiscoveryActor
                disc_cmd = self.discovery_rx.recv() => {
                    match disc_cmd {
                        Some(cmd) => {
                            self.handle_discovery_cmd(cmd).await;
                        }
                        None => {
                            tracing::warn!("⚠️ [NET] Discovery channel closed");
                        }
                    }
                }

                // Commands from TopicActors (snapshot coordination)
                topic_cmd = self.topic_network_rx.recv() => {
                    match topic_cmd {
                        Some(cmd) => {
                            self.handle_topic_network_cmd(cmd);
                        }
                        None => {
                            tracing::warn!("⚠️ [NET] Topic network channel closed");
                        }
                    }
                }
            }
        }

        tracing::info!("🛑 [NET] NetworkActor stopped");
    }

    fn handle_topic_network_cmd(&mut self, cmd: TopicNetworkCmd) {
        match cmd {
            TopicNetworkCmd::NeedSnapshot { group_id } => {
                eprintln!("📥 [NET] TopicActor needs snapshot for {}...", &group_id[..16.min(group_id.len())]);
                self.groups_needing_snapshot.insert(group_id);
            }
            TopicNetworkCmd::SnapshotComplete { group_id } => {
                eprintln!("✅ [NET] Snapshot complete for {}...", &group_id[..16.min(group_id.len())]);
                self.groups_needing_snapshot.remove(&group_id);
            }
            TopicNetworkCmd::SnapshotFailed { group_id, reason } => {
                eprintln!("⚠️ [NET] Snapshot failed for {}...: {}", &group_id[..16.min(group_id.len())], reason);
                // Keep in set for retry
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // NETWORK COMMAND HANDLING (from FFI)
    // ═══════════════════════════════════════════════════════════════════════

    async fn handle_network_command(&mut self, cmd: NetworkCommand) {
        // Log every command received
        eprintln!("📥 [NET] handle_network_command: {:?}", std::mem::discriminant(&cmd));

        match cmd {
            NetworkCommand::JoinGroup { group_id, bootstrap_peer } => {
                // SIGNPOST: JoinGroup received from FFI
                eprintln!("═══════════════════════════════════════════════════════════════════");
                eprintln!("🔗 [NET-JOIN-1] NetworkActor received JoinGroup command");
                eprintln!("   group_id: {}...", &group_id[..16.min(group_id.len())]);
                eprintln!("   bootstrap_peer: {:?}", bootstrap_peer.as_ref().map(|p| format!("{}...", &p[..16.min(p.len())])));
                eprintln!("═══════════════════════════════════════════════════════════════════");

                tracing::info!(
                    "🔗 [NET-JOIN-1] JoinGroup: {} (bootstrap: {:?})",
                    &group_id[..16.min(group_id.len())],
                    bootstrap_peer.as_ref().map(|p| &p[..16.min(p.len())])
                );

                // Parse bootstrap peer if provided
                let mut initial_peers = vec![];
                if let Some(ref peer_str) = bootstrap_peer {
                    match PublicKey::from_str(peer_str) {
                        Ok(pk) => {
                            eprintln!("🔗 [NET-JOIN-2] ✓ Parsed bootstrap peer: {}...", &peer_str[..16]);
                            initial_peers.push(pk);
                        }
                        Err(e) => {
                            eprintln!("🔗 [NET-JOIN-2] ✗ Failed to parse bootstrap peer: {}", e);
                        }
                    }
                }

                // Always add bootstrap node
                if let Ok(bootstrap_pk) = PublicKey::from_str(bootstrap_node_id()) {
                    if !initial_peers.contains(&bootstrap_pk) {
                        eprintln!("🔗 [NET-JOIN-3] Adding bootstrap node: {}...", &bootstrap_node_id()[..16]);
                        initial_peers.push(bootstrap_pk);
                    }
                } else {
                    eprintln!("🔗 [NET-JOIN-3] ⚠️ Failed to parse BOOTSTRAP_NODE_ID");
                }

                eprintln!("🔗 [NET-JOIN-4] Spawning TopicActor with {} initial peers", initial_peers.len());

                // Spawn topic actor
                match self.spawn_topic_actor(&group_id, initial_peers).await {
                    Ok(_) => {
                        eprintln!("🔗 [NET-JOIN-5] ✓ TopicActor spawned successfully");
                        tracing::info!("🔗 [NET-JOIN-5] TopicActor spawned for {}", &group_id[..16.min(group_id.len())]);

                        // Trigger snapshot request - this also helps establish the gossip mesh
                        eprintln!("🔗 [NET-JOIN-6] Triggering snapshot request...");
                        if let Some(handle) = self.topics.get(&group_id) {
                            eprintln!("🔗 [NET-JOIN-6a] Sending RequestSnapshot to TopicActor");
                            let _ = handle.cmd_tx.send(ActorMessage::Domain(
                                TopicCommand::RequestSnapshot
                            ));
                            eprintln!("🔗 [NET-JOIN-6b] ✓ RequestSnapshot command sent");
                        } else {
                            eprintln!("🔗 [NET-JOIN-6a] ⚠️ TopicActor handle not found!");
                        }
                    }
                    Err(e) => {
                        eprintln!("🔗 [NET-JOIN-5] 🔴 FAILED to spawn TopicActor: {}", e);
                        tracing::error!("🔴 [NET-JOIN-5] Failed to spawn TopicActor: {}", e);
                    }
                }

                // Tell discovery actor about new group
                if let Some(ref handle) = self.discovery_handle {
                    eprintln!("🔗 [NET-JOIN-7] Announcing group to DiscoveryActor");
                    let _ = handle.cmd_tx.send(ActorMessage::Domain(
                        DiscoveryCommand::AnnounceGroup(group_id.clone())
                    ));
                } else {
                    eprintln!("🔗 [NET-JOIN-7] ⚠️ No DiscoveryActor handle");
                }

                eprintln!("═══════════════════════════════════════════════════════════════════");
                eprintln!("✅ [NET-JOIN-8] JoinGroup complete for {}...", &group_id[..16.min(group_id.len())]);
                eprintln!("═══════════════════════════════════════════════════════════════════");
            }

            NetworkCommand::Broadcast { group_id, event } => {
                eprintln!("📤 [NET] Broadcast command received:");
                eprintln!("   group_id: {}...", &group_id[..16.min(group_id.len())]);
                eprintln!("   event: {:?}", std::mem::discriminant(&event));
                if let Some(handle) = self.topics.get(&group_id) {
                    eprintln!("📤 [NET] ✓ TopicActor found, forwarding...");
                    let _ = handle.cmd_tx.send(ActorMessage::Domain(
                        TopicCommand::Broadcast(event)
                    ));
                } else {
                    eprintln!("📤 [NET] ⚠️ No TopicActor for group {}...", &group_id[..16.min(group_id.len())]);
                    tracing::warn!(
                        "⚠️ [NET] No TopicActor for group {}",
                        &group_id[..16.min(group_id.len())]
                    );
                }
            }

            NetworkCommand::RequestSnapshot { from_peer } => {
                tracing::info!("🗂️ [NET] RequestSnapshot from {}", &from_peer[..16]);
                // This is handled by TopicActor via gossip - the request is broadcast
                // and peers respond with SnapshotAvailable
            }

            NetworkCommand::RequestFileDownload { file_id, hash, source_peer, resume_offset } => {
                tracing::info!(
                    "📥 [NET] RequestFileDownload: {} from {}",
                    &file_id[..16.min(file_id.len())],
                    &source_peer[..16]
                );

                // Look up which group this file belongs to
                if let Some(group_id) = storage::file_get_group_id(&file_id) {
                    if let Some(handle) = self.topics.get(&group_id) {
                        let _ = handle.cmd_tx.send(ActorMessage::Domain(
                            TopicCommand::DownloadFile {
                                file_id,
                                hash,
                                source_peer,
                                resume_offset,
                            }
                        ));
                    } else {
                        tracing::warn!(
                            "⚠️ [NET] No TopicActor for file's group {}",
                            &group_id[..16.min(group_id.len())]
                        );
                    }
                } else {
                    tracing::warn!(
                        "⚠️ [NET] File {} not found in DB, cannot route download",
                        &file_id[..16.min(file_id.len())]
                    );
                }
            }

            NetworkCommand::StartChatStream { peer_id, workspace_id } => {
                tracing::info!("💬 [NET] StartChatStream with {}", &peer_id[..16]);

                match self.ensure_dm_stream(&peer_id).await {
                    Ok(_) => {
                        let _ = self.event_tx.send(SwiftEvent::ChatStreamReady {
                            peer_id,
                            workspace_id,
                        });
                    }
                    Err(e) => {
                        tracing::error!("🔴 [NET] Failed to start DM stream: {}", e);
                    }
                }
            }

            NetworkCommand::SendDirectChat { peer_id, workspace_id, message, parent_id } => {
                eprintln!("💬 [NET] SendDirectChat:");
                eprintln!("   peer_id: {}...", &peer_id[..16.min(peer_id.len())]);
                eprintln!("   message: {}...", &message[..50.min(message.len())]);
                tracing::info!("💬 [NET] SendDirectChat to {}", &peer_id[..16]);

                let dm = DirectMessage {
                    id: blake3::hash(
                        format!("dm:{}-{}-{}", &peer_id, &message, chrono::Utc::now()).as_bytes()
                    ).to_hex().to_string(),
                    workspace_id: Some(workspace_id.clone()),
                    message: message.clone(),
                    parent_id,
                    timestamp: chrono::Utc::now().timestamp(),
                    attachment: None,
                };

                eprintln!("💬 [NET] Created DM id: {}...", &dm.id[..16]);

                match self.ensure_dm_stream(&peer_id).await {
                    Ok(sender) => {
                        eprintln!("💬 [NET] ✓ DM stream established, sending...");
                        if let Err(e) = sender.send(dm.clone()) {
                            eprintln!("💬 [NET] 🔴 Failed to send DM to channel: {}", e);
                            tracing::error!("🔴 [NET] Failed to send DM: {}", e);
                        } else {
                            eprintln!("💬 [NET] ✓ DM sent to channel");
                            // Store locally and emit event
                            let _ = storage::dm_insert(
                                &dm.id,
                                &peer_id,
                                &dm.message,
                                dm.timestamp,
                                false, // outgoing
                            );

                            let _ = self.event_tx.send(SwiftEvent::DirectMessageReceived {
                                id: dm.id,
                                peer_id,
                                message,
                                timestamp: dm.timestamp,
                                is_incoming: false,
                            });
                            eprintln!("💬 [NET] ✓ DM stored and event emitted");
                        }
                    }
                    Err(e) => {
                        eprintln!("💬 [NET] 🔴 Failed to establish DM stream: {}", e);
                        tracing::error!("🔴 [NET] Failed to establish DM stream: {}", e);
                    }
                }
            }

            NetworkCommand::DissolveGroup { id } => {
                // Owner dissolved group - topic actor already broadcast dissolution
                // Now clean up local state

                // Remove topic actor
                if let Some(handle) = self.topics.remove(&id) {
                    let _ = handle.cmd_tx.send(ActorMessage::System(SystemCommand::PoisonPill));
                }

                // Remove from snapshot tracking
                self.groups_needing_snapshot.remove(&id);

                // Tell discovery
                if let Some(ref handle) = self.discovery_handle {
                    let _ = handle.cmd_tx.send(ActorMessage::Domain(
                        DiscoveryCommand::LeaveGroup(id)
                    ));
                }
            }

            NetworkCommand::LeaveGroup { id } => {
                // Non-owner left group - local cleanup only, no broadcast needed

                // Remove topic actor
                if let Some(handle) = self.topics.remove(&id) {
                    let _ = handle.cmd_tx.send(ActorMessage::System(SystemCommand::PoisonPill));
                }

                // Remove from snapshot tracking
                self.groups_needing_snapshot.remove(&id);

                // Tell discovery
                if let Some(ref handle) = self.discovery_handle {
                    let _ = handle.cmd_tx.send(ActorMessage::Domain(
                        DiscoveryCommand::LeaveGroup(id)
                    ));
                }
            }

            NetworkCommand::DissolveWorkspace { id, group_id: _ } => {
                // Owner dissolved workspace - already broadcast via topic actor
                // Nothing to do at network level
            }

            NetworkCommand::LeaveWorkspace { id: _ } => {
                // Non-owner left workspace - local cleanup only
                // Nothing to do at network level
            }

            NetworkCommand::DissolveBoard { id, group_id: _ } => {
                // Owner dissolved board - already broadcast via topic actor
                // Nothing to do at network level
            }

            NetworkCommand::LeaveBoard { id: _ } => {
                // Non-owner left board - local cleanup only
                // Nothing to do at network level
            }

            NetworkCommand::ResumePendingTransfers => {
                tracing::info!("📥 [NET] ResumePendingTransfers");

                // Get all pending transfers from storage
                if let Ok(transfers) = storage::transfer_list_pending() {
                    for (file_id, hash, source_peer, bytes_received) in transfers {
                        // Route to appropriate topic actor
                        if let Some(group_id) = storage::file_get_group_id(&file_id) {
                            if let Some(handle) = self.topics.get(&group_id) {
                                let _ = handle.cmd_tx.send(ActorMessage::Domain(
                                    TopicCommand::DownloadFile {
                                        file_id,
                                        hash,
                                        source_peer,
                                        resume_offset: bytes_received,
                                    }
                                ));
                            }
                        }
                    }
                }
            }

            // Pass-through commands handled elsewhere
            NetworkCommand::UploadToGroup { .. } |
            NetworkCommand::UploadToWorkspace { .. } |
            NetworkCommand::DeleteChat { .. } => {
                // These are handled by CommandActor or TopicActor
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // DISCOVERY COMMAND HANDLING (from DiscoveryActor)
    // ═══════════════════════════════════════════════════════════════════════

    async fn handle_discovery_cmd(&mut self, cmd: DiscoveryNetworkCmd) {
        match cmd {
            DiscoveryNetworkCmd::JoinPeerToTopic { group_id, peer } => {
                tracing::info!(
                    "🔗 [NET] JoinPeerToTopic: {} → {}",
                    &peer.to_string()[..16],
                    &group_id[..16.min(group_id.len())]
                );

                if let Some(handle) = self.topics.get(&group_id) {
                    let _ = handle.cmd_tx.send(ActorMessage::Domain(
                        TopicCommand::JoinPeers(vec![peer])
                    ));

                    // Update peers_per_group
                    let mut peers = self.peers_per_group.lock().unwrap();
                    peers.entry(group_id).or_default().insert(peer);
                }
            }

            DiscoveryNetworkCmd::JoinPeersToTopic { group_id, peers } => {
                tracing::info!(
                    "🔗 [NET] JoinPeersToTopic: {} peers → {}",
                    peers.len(),
                    &group_id[..16.min(group_id.len())]
                );

                if let Some(handle) = self.topics.get(&group_id) {
                    let _ = handle.cmd_tx.send(ActorMessage::Domain(
                        TopicCommand::JoinPeers(peers.clone())
                    ));

                    // Update peers_per_group
                    let mut peers_map = self.peers_per_group.lock().unwrap();
                    let group_peers = peers_map.entry(group_id).or_default();
                    for p in peers {
                        group_peers.insert(p);
                    }
                }
            }

            DiscoveryNetworkCmd::EnsureTopicExists { group_id } => {
                if !self.topics.contains_key(&group_id) {
                    let _ = self.spawn_topic_actor(&group_id, vec![]).await;
                }
            }

            DiscoveryNetworkCmd::PeerDiscovered { peer_id, groups } => {
                tracing::info!(
                    "👋 [NET] PeerDiscovered: {} in {} shared groups",
                    &peer_id[..16],
                    groups.len()
                );
                // Could emit event to Swift here if needed
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TOPIC ACTOR MANAGEMENT
    // ═══════════════════════════════════════════════════════════════════════

    async fn spawn_topic_actor(
        &mut self,
        group_id: &str,
        initial_peers: Vec<PublicKey>,
    ) -> Result<()> {
        // Don't spawn duplicate
        if self.topics.contains_key(group_id) {
            eprintln!("🔍 [TOPIC-SPAWN] TopicActor already exists for {}...", &group_id[..16.min(group_id.len())]);
            tracing::debug!("🔍 [TOPIC-SPAWN] TopicActor already exists for {}", &group_id[..16.min(group_id.len())]);
            return Ok(());
        }

        eprintln!("🚀 [TOPIC-SPAWN-1] Spawning TopicActor for {}...", &group_id[..16.min(group_id.len())]);
        eprintln!("   Initial peers: {}", initial_peers.len());
        for (i, peer) in initial_peers.iter().enumerate() {
            eprintln!("   Peer {}: {}...", i, &peer.to_string()[..16]);
        }

        tracing::info!(
            "🚀 [TOPIC-SPAWN-1] Spawning TopicActor for {} with {} initial peers",
            &group_id[..16.min(group_id.len())],
            initial_peers.len()
        );

        let handle = TopicActor::spawn(
            self.node_id.clone(),
            group_id.to_string(),
            self.endpoint.clone(),
            self.gossip.clone(),
            initial_peers,
            self.topic_network_tx.clone(),
            self.event_tx.clone(),
        ).await?;

        eprintln!("🚀 [TOPIC-SPAWN-2] ✓ TopicActor spawned, inserting into topics map");
        self.topics.insert(group_id.to_string(), handle);

        // Initialize peers_per_group entry
        self.peers_per_group.lock().unwrap()
            .entry(group_id.to_string())
            .or_default();

        eprintln!("🚀 [TOPIC-SPAWN-3] ✓ State initialized for group");
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════
    // DM STREAM MANAGEMENT
    // ═══════════════════════════════════════════════════════════════════════

    /// Ensure we have a DM stream with a peer, creating one if needed
    async fn ensure_dm_stream(&mut self, peer_id: &str) -> Result<UnboundedSender<DirectMessage>> {
        eprintln!("💬 [DM] ensure_dm_stream for peer {}...", &peer_id[..16.min(peer_id.len())]);

        // Check existing
        {
            let senders = self.dm_senders.lock().unwrap();
            if let Some(sender) = senders.get(peer_id) {
                eprintln!("💬 [DM] ✓ Reusing existing stream");
                return Ok(sender.clone());
            }
        }

        eprintln!("💬 [DM] No existing stream, opening new connection...");

        // Open new connection
        let pk = PublicKey::from_str(peer_id)?;

        tracing::info!("💬 [NET] Opening DM stream to {}", &peer_id[..16]);

        let conn: Connection = tokio::time::timeout(
            Duration::from_secs(10),
            self.endpoint.connect(pk, DM_ALPN)
        ).await
            .map_err(|_| anyhow!("DM connection timeout"))?
            .map_err(|e| anyhow!("DM connect failed: {}", e))?;

        eprintln!("💬 [DM] ✓ QUIC connection established, opening bi-stream...");

        let (send_stream, recv_stream) = conn.open_bi().await?;

        eprintln!("💬 [DM] ✓ Bi-stream opened");

        // Create channel for outbound messages
        let (tx, rx) = mpsc::unbounded_channel();

        // Spawn bidirectional handler for OUTBOUND connection
        let peer_id_clone = peer_id.to_string();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            handle_dm_stream_with_streams(
                peer_id_clone,
                send_stream,
                recv_stream,
                rx,
                event_tx,
            ).await;
        });

        self.dm_senders.lock().unwrap().insert(peer_id.to_string(), tx.clone());

        eprintln!("💬 [DM] ✓ DM stream handler spawned and registered");

        Ok(tx)
    }

    // NOTE: spawn_dm_acceptor and spawn_protocol_acceptor removed
    // Router handles all protocol acceptance now via ProtocolHandler trait

    // ═══════════════════════════════════════════════════════════════════════
    // PROFILE BROADCAST
    // ═══════════════════════════════════════════════════════════════════════

    fn broadcast_profile_to_all_groups(&self) {
        // Get profile from storage
        if let Some(profile) = storage::profile_get(&self.node_id) {
            let event = NetworkEvent::ProfileUpdated {
                node_id: self.node_id.clone(),
                display_name: profile.0,
                avatar_hash: profile.1,
            };

            // Broadcast to all topics
            for (group_id, handle) in &self.topics {
                let _ = handle.cmd_tx.send(ActorMessage::Domain(
                    TopicCommand::Broadcast(event.clone())
                ));
                tracing::debug!(
                    "📤 [NET] Broadcast profile to group {}",
                    &group_id[..16.min(group_id.len())]
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DM STREAM HANDLER (called by DmHandler ProtocolHandler)
// ═══════════════════════════════════════════════════════════════════════════

/// Handle incoming DM connection from ProtocolHandler
async fn handle_dm_stream(
    conn: Connection,
    peer_id: String,
    dm_senders: Arc<std::sync::Mutex<HashMap<String, UnboundedSender<DirectMessage>>>>,
    event_tx: UnboundedSender<SwiftEvent>,
) -> Result<()> {
    tracing::info!("💬 [DM] Accepting bi-stream from {}", &peer_id[..16]);

    let (mut send, mut recv) = conn.accept_bi().await?;

    // Create outbound channel for this peer
    let (tx, mut outbound_rx) = mpsc::unbounded_channel();

    // Register sender for bidirectional communication
    dm_senders.lock().unwrap().insert(peer_id.clone(), tx);

    // Emit event that stream is ready
    let _ = event_tx.send(SwiftEvent::ChatStreamReady {
        peer_id: peer_id.clone(),
        workspace_id: String::new(),
    });

    tracing::info!("💬 [DM] Stream handler started for {}", &peer_id[..16]);

    loop {
        tokio::select! {
            // Incoming message
            result = read_dm_frame(&mut recv) => {
                match result {
                    Ok(dm) => {
                        tracing::info!(
                            "💬 [DM] Received from {}: {}",
                            &peer_id[..16],
                            &dm.message[..50.min(dm.message.len())]
                        );

                        // Handle file attachment if present
                        if let Some(ref attachment) = dm.attachment {
                            tracing::info!(
                                "📎 [DM] Message has attachment: {} ({} bytes)",
                                attachment.name,
                                attachment.size
                            );
                        }

                        // Store in DB
                        let _ = storage::dm_insert(
                            &dm.id,
                            &peer_id,
                            &dm.message,
                            dm.timestamp,
                            true, // incoming
                        );

                        // Emit event to Swift
                        let _ = event_tx.send(SwiftEvent::DirectMessageReceived {
                            id: dm.id,
                            peer_id: peer_id.clone(),
                            message: dm.message,
                            timestamp: dm.timestamp,
                            is_incoming: true,
                        });
                    }
                    Err(e) => {
                        tracing::info!("💬 [DM] Stream closed with {}: {}", &peer_id[..16], e);
                        break;
                    }
                }
            }

            // Outgoing message
            outbound = outbound_rx.recv() => {
                match outbound {
                    Some(dm) => {
                        if let Err(e) = write_dm_frame(&mut send, &dm).await {
                            tracing::error!("🔴 [DM] Failed to send to {}: {}", &peer_id[..16], e);
                            break;
                        }
                    }
                    None => {
                        tracing::info!("💬 [DM] Outbound channel closed for {}", &peer_id[..16]);
                        break;
                    }
                }
            }
        }
    }

    // Cleanup
    dm_senders.lock().unwrap().remove(&peer_id);
    let _ = event_tx.send(SwiftEvent::ChatStreamClosed { peer_id: peer_id.clone() });
    tracing::info!("💬 [DM] Stream handler stopped for {}", &peer_id[..16]);

    Ok(())
}

/// Handle OUTBOUND DM connection (we already have streams from open_bi)
async fn handle_dm_stream_with_streams(
    peer_id: String,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    mut outbound_rx: UnboundedReceiver<DirectMessage>,
    event_tx: UnboundedSender<SwiftEvent>,
) {
    tracing::info!("💬 [DM] Outbound stream handler started for {}", &peer_id[..16]);

    loop {
        tokio::select! {
            // Incoming message
            result = read_dm_frame(&mut recv) => {
                match result {
                    Ok(dm) => {
                        tracing::info!(
                            "💬 [DM] Received from {}: {}",
                            &peer_id[..16],
                            &dm.message[..50.min(dm.message.len())]
                        );

                        // Handle file attachment if present
                        if let Some(ref attachment) = dm.attachment {
                            tracing::info!(
                                "📎 [DM] Message has attachment: {} ({} bytes)",
                                attachment.name,
                                attachment.size
                            );
                        }

                        // Store in DB
                        let _ = storage::dm_insert(
                            &dm.id,
                            &peer_id,
                            &dm.message,
                            dm.timestamp,
                            true, // incoming
                        );

                        // Emit event to Swift
                        let _ = event_tx.send(SwiftEvent::DirectMessageReceived {
                            id: dm.id,
                            peer_id: peer_id.clone(),
                            message: dm.message,
                            timestamp: dm.timestamp,
                            is_incoming: true,
                        });
                    }
                    Err(e) => {
                        tracing::info!("💬 [DM] Stream closed with {}: {}", &peer_id[..16], e);
                        break;
                    }
                }
            }

            // Outgoing message
            outbound = outbound_rx.recv() => {
                match outbound {
                    Some(dm) => {
                        if let Err(e) = write_dm_frame(&mut send, &dm).await {
                            tracing::error!("🔴 [DM] Failed to send to {}: {}", &peer_id[..16], e);
                            break;
                        }
                    }
                    None => {
                        tracing::info!("💬 [DM] Outbound channel closed for {}", &peer_id[..16]);
                        break;
                    }
                }
            }
        }
    }

    let _ = event_tx.send(SwiftEvent::ChatStreamClosed { peer_id: peer_id.clone() });
    tracing::info!("💬 [DM] Outbound stream handler stopped for {}", &peer_id[..16]);
}

/// Read a length-prefixed JSON frame using read_chunk for efficiency
async fn read_dm_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<DirectMessage> {
    // Read length prefix (4 bytes)
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 10 * 1024 * 1024 {
        return Err(anyhow!("DM frame too large: {} bytes", len));
    }

    // Read data using read_chunk for efficiency
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        let remaining = len - data.len();
        match recv.read_chunk(remaining, true).await? {
            Some(chunk) => {
                data.extend_from_slice(&chunk.bytes);
            }
            None => {
                return Err(anyhow!("Stream ended before complete frame"));
            }
        }
    }

    Ok(serde_json::from_slice(&data)?)
}

/// Write a length-prefixed JSON frame using write_chunk for efficiency
async fn write_dm_frame(send: &mut iroh::endpoint::SendStream, dm: &DirectMessage) -> Result<()> {
    let data = serde_json::to_vec(dm)?;
    let len = (data.len() as u32).to_be_bytes();

    // Write length prefix
    send.write_chunk(Bytes::copy_from_slice(&len)).await?;

    // Write data
    send.write_chunk(Bytes::from(data)).await?;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// FILE TRANSFER SERVER (handles incoming file requests)
// ═══════════════════════════════════════════════════════════════════════════

async fn handle_file_transfer_server(
    conn: Connection,
    event_tx: UnboundedSender<SwiftEvent>,
) -> Result<()> {
    use crate::models::protocol::FileTransferMsg;

    let (mut send, mut recv) = conn.accept_bi().await?;

    // Read request
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut req_data = vec![0u8; len];
    recv.read_exact(&mut req_data).await?;

    let request: FileTransferMsg = serde_json::from_slice(&req_data)?;

    match request {
        FileTransferMsg::Request { file_id, hash, offset: resume_offset } => {
            tracing::info!(
                "📤 [FILE] Serving file {} from offset {}",
                &file_id[..16.min(file_id.len())],
                resume_offset
            );

            // Look up file in storage
            match storage::file_get_for_transfer(&file_id, &hash) {
                Some((name, local_path, total_size)) => {
                    if local_path.is_empty() {
                        // File exists but not downloaded locally
                        let not_found = FileTransferMsg::NotFound { file_id };
                        let data = serde_json::to_vec(&not_found)?;
                        let len = (data.len() as u32).to_be_bytes();
                        send.write_chunk(Bytes::copy_from_slice(&len)).await?;
                        send.write_chunk(Bytes::from(data)).await?;
                        return Ok(());
                    }

                    // Send header
                    let header = FileTransferMsg::Header {
                        file_id: file_id.clone(),
                        file_name: name,
                        total_size,
                        hash: hash.clone(),
                        byte_offset: resume_offset,
                        byte_length: total_size - resume_offset,
                    };

                    let header_data = serde_json::to_vec(&header)?;
                    let len = (header_data.len() as u32).to_be_bytes();
                    send.write_chunk(Bytes::copy_from_slice(&len)).await?;
                    send.write_chunk(Bytes::from(header_data)).await?;

                    // Stream file content using write_chunk
                    let mut file = tokio::fs::File::open(&local_path).await?;

                    if resume_offset > 0 {
                        use tokio::io::AsyncSeekExt;
                        file.seek(std::io::SeekFrom::Start(resume_offset)).await?;
                    }

                    use tokio::io::AsyncReadExt;
                    let mut buf = vec![0u8; 64 * 1024]; // 64KB chunks
                    let mut sent = 0u64;

                    loop {
                        let n = file.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }

                        send.write_chunk(Bytes::copy_from_slice(&buf[..n])).await?;
                        sent += n as u64;
                    }

                    tracing::info!("📤 [FILE] Sent {} bytes for {}", sent, &file_id[..16.min(file_id.len())]);

                    // Send complete acknowledgment
                    let complete = FileTransferMsg::Complete { file_id, hash };
                    let data = serde_json::to_vec(&complete)?;
                    let len = (data.len() as u32).to_be_bytes();
                    send.write_chunk(Bytes::copy_from_slice(&len)).await?;
                    send.write_chunk(Bytes::from(data)).await?;
                }
                None => {
                    let not_found = FileTransferMsg::NotFound { file_id };
                    let data = serde_json::to_vec(&not_found)?;
                    let len = (data.len() as u32).to_be_bytes();
                    send.write_chunk(Bytes::copy_from_slice(&len)).await?;
                    send.write_chunk(Bytes::from(data)).await?;
                }
            }
        }
        _ => {
            tracing::warn!("⚠️ [FILE] Unexpected message type");
        }
    }

    // CRITICAL: Signal end of stream and wait for peer to receive
    send.finish()?;
    let _ = send.stopped().await;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// SNAPSHOT SERVER (handles incoming snapshot requests)
// ═══════════════════════════════════════════════════════════════════════════

async fn handle_snapshot_server(
    conn: Connection,
    node_id: String,
    event_tx: UnboundedSender<SwiftEvent>,
) -> Result<()> {
    use crate::models::protocol::SnapshotFrame;

    let peer_id = conn.remote_id().to_string();
    eprintln!("📤 [SNAP] ════════════════════════════════════════════════════════");
    eprintln!("📤 [SNAP] SNAPSHOT SERVER - handling request from {}...", &peer_id[..16]);
    eprintln!("📤 [SNAP] ════════════════════════════════════════════════════════");

    let (mut send, mut recv) = conn.accept_bi().await?;
    eprintln!("   ✓ Bidirectional stream opened");

    // Read group_id request
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut group_id_bytes = vec![0u8; len];
    recv.read_exact(&mut group_id_bytes).await?;
    let group_id = String::from_utf8(group_id_bytes)?;

    eprintln!("   → Requested group: {}...", &group_id[..16.min(group_id.len())]);
    tracing::info!("📤 [SNAP] Sending snapshot for group {}", &group_id[..16.min(group_id.len())]);

    // Build snapshot from storage
    if let Some(group) = storage::group_get(&group_id)? {
        let workspaces = storage::workspace_list_by_group(&group_id)?;
        let workspace_ids: Vec<String> = workspaces.iter().map(|w| w.id.clone()).collect();
        let boards = storage::board_list_by_workspaces(&workspace_ids)?;

        eprintln!("   📊 Building snapshot:");
        eprintln!("      - group: {}", group.name);
        eprintln!("      - workspaces: {}", workspaces.len());
        eprintln!("      - boards: {}", boards.len());

        // Extract board_ids before moving boards
        let board_ids: Vec<String> = boards.iter().map(|b| b.id.clone()).collect();

        // Send Structure frame
        let structure = SnapshotFrame::Structure {
            group,
            workspaces,
            boards,
        };

        let data = serde_json::to_vec(&structure)?;
        eprintln!("   → Sending Structure frame ({} bytes)", data.len());
        let len = (data.len() as u32).to_be_bytes();
        send.write_chunk(Bytes::copy_from_slice(&len)).await?;
        send.write_chunk(Bytes::from(data)).await?;

        // Send Content frame (elements + cells)
        let elements = storage::element_list_by_boards(&board_ids)?;
        let cells = storage::cell_list_by_boards(&board_ids)?;

        eprintln!("      - elements: {}", elements.len());
        eprintln!("      - cells: {}", cells.len());

        let content = SnapshotFrame::Content { elements, cells };
        let data = serde_json::to_vec(&content)?;
        eprintln!("   → Sending Content frame ({} bytes)", data.len());
        let len = (data.len() as u32).to_be_bytes();
        send.write_chunk(Bytes::copy_from_slice(&len)).await?;
        send.write_chunk(Bytes::from(data)).await?;

        // Send Metadata frame
        let chats = storage::chat_list_by_workspaces(&workspace_ids)?;
        let files = storage::file_list_by_group(&group_id)?;
        let integrations = storage::integration_list_by_group(&group_id)?;
        let board_metadata = storage::board_metadata_list_by_boards(&board_ids)?;

        eprintln!("      - chats: {}", chats.len());
        eprintln!("      - files: {}", files.len());
        eprintln!("      - integrations: {}", integrations.len());
        eprintln!("      - board_metadata: {}", board_metadata.len());

        let metadata = SnapshotFrame::Metadata {
            chats,
            files,
            integrations,
            board_metadata,
        };
        let data = serde_json::to_vec(&metadata)?;
        eprintln!("   → Sending Metadata frame ({} bytes)", data.len());
        let len = (data.len() as u32).to_be_bytes();
        send.write_chunk(Bytes::copy_from_slice(&len)).await?;
        send.write_chunk(Bytes::from(data)).await?;

        // Send Complete frame
        let complete = SnapshotFrame::Complete;
        let data = serde_json::to_vec(&complete)?;
        eprintln!("   → Sending Complete frame");
        let len = (data.len() as u32).to_be_bytes();
        send.write_chunk(Bytes::copy_from_slice(&len)).await?;
        send.write_chunk(Bytes::from(data)).await?;

        // CRITICAL: Signal end of stream and wait for peer to receive
        send.finish()?;
        let _ = send.stopped().await;

        eprintln!("✅ [SNAP] Snapshot SENT for group {}...", &group_id[..16.min(group_id.len())]);
        tracing::info!("📤 [SNAP] Snapshot sent for group {}", &group_id[..16.min(group_id.len())]);
    } else {
        eprintln!("⚠️ [SNAP] Group not found in DB: {}...", &group_id[..16.min(group_id.len())]);
    }

    Ok(())
}