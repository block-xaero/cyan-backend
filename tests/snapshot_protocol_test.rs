// tests/snapshot_protocol_test.rs
//
// FULL PROTOCOL TEST - Discovery → Snapshot flow WITHOUT actors
//
// This tests the complete invite/sync protocol:
//   1. Both peers join discovery topic via bootstrap
//   2. GroupsExchange finds shared groups
//   3. PeerIntroduction announces peers
//   4. Both subscribe to group topic
//   5. RequestSnapshot → SnapshotAvailable → Direct QUIC transfer
//
// Build:  cargo build --release --bin snapshot_test
// Run:
//   Machine A:  ./target/release/snapshot_test host
//   Machine B:  ./target/release/snapshot_test join
//
// No NODE_ID passing needed - both discover via production bootstrap!

use anyhow::Result;
use bytes::Bytes;
use futures_lite::StreamExt;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey,
};
use iroh_gossip::{
    net::Gossip,
    proto::TopicId,
};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

// ═══════════════════════════════════════════════════════════════════════════
// CONSTANTS (match cyan-backend production values)
// ═══════════════════════════════════════════════════════════════════════════

const RELAY_URL: &str = "https://quic.dev.cyan.blockxaero.io";
const DISCOVERY_KEY: &str = "cyan-dev";
const BOOTSTRAP_NODE_ID: &str = "f992aa3b5409410b373605002a47e5521f1f2a9d10d2910544c3b37f4d6ed618";

const SNAPSHOT_ALPN: &[u8] = b"cyan-snapshot-v1";

const TEST_GROUP_ID: &str = "test-group-snap-1111-2222-3333-444444444444";
const TEST_WORKSPACE_ID: &str = "test-ws-snap-1111-2222-3333-444444444444";
const TEST_BOARD_ID: &str = "test-board-snap-1111-2222-3333-444444444444";

// ═══════════════════════════════════════════════════════════════════════════
// SNAPSHOT PROTOCOL HANDLER (for Router integration)
// ═══════════════════════════════════════════════════════════════════════════

/// Channel sender for notifying main loop of snapshot requests
type SnapshotNotifySender = mpsc::UnboundedSender<String>;

#[derive(Debug, Clone)]
struct SnapshotHandler {
    notify_tx: SnapshotNotifySender,
}

impl ProtocolHandler for SnapshotHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let peer_id = conn.remote_id().to_string();
        println!("\n═══════════════════════════════════════════════════════════════════");
        println!("📥 [SNAPSHOT-HANDLER] Incoming request from {}...", &peer_id[..16]);
        println!("═══════════════════════════════════════════════════════════════════");

        // Notify main loop (optional, for logging)
        let _ = self.notify_tx.send(peer_id.clone());

        // Handle the snapshot request - log errors but don't fail the handler
        if let Err(e) = handle_snapshot_server(conn).await {
            eprintln!("🔴 [SNAPSHOT-HANDLER] Error: {}", e);
        }

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DISCOVERY PROTOCOL MESSAGES
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg_type")]
enum DiscoveryMessage {
    #[serde(rename = "groups_exchange")]
    GroupsExchange {
        node_id: String,
        local_groups: Vec<String>,
    },
    #[serde(rename = "peer_introduction")]
    PeerIntroduction {
        group_id: String,
        peers: Vec<String>,
    },
}

// ═══════════════════════════════════════════════════════════════════════════
// GROUP TOPIC MESSAGES
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum GroupMessage {
    #[serde(rename = "request_snapshot")]
    RequestSnapshot { from_peer: String },
    #[serde(rename = "snapshot_available")]
    SnapshotAvailable { source: String, group_id: String },
}

// ═══════════════════════════════════════════════════════════════════════════
// SNAPSHOT FRAME TYPES
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub group_id: String,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhiteboardDTO {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhiteboardElementDTO {
    pub id: String,
    pub board_id: String,
    pub element_type: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub z_index: i32,
    pub style_json: Option<String>,
    pub content_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookCellDTO {
    pub id: String,
    pub board_id: String,
    pub cell_type: String,
    pub cell_order: i32,
    pub content: Option<String>,
    pub output: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
    pub height: Option<f64>,
    pub metadata_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDTO {
    pub id: String,
    pub workspace_id: String,
    pub message: String,
    pub author: String,
    pub parent_id: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDTO {
    pub id: String,
    pub group_id: Option<String>,
    pub workspace_id: Option<String>,
    pub board_id: Option<String>,
    pub name: String,
    pub hash: String,
    pub size: u64,
    pub source_peer: Option<String>,
    pub local_path: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationBindingDTO {
    pub id: String,
    pub scope_type: String,
    pub scope_id: String,
    pub integration_type: String,
    #[serde(default)]
    pub config: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardMetadataDTO {
    pub board_id: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub rating: i32,
    #[serde(default)]
    pub view_count: i32,
    pub contains_model: Option<String>,
    #[serde(default)]
    pub contains_skills: Vec<String>,
    #[serde(default = "default_board_type")]
    pub board_type: String,
    #[serde(default)]
    pub last_accessed: i64,
    #[serde(default)]
    pub is_pinned: bool,
}

fn default_board_type() -> String {
    "canvas".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "frame_type")]
pub enum SnapshotFrame {
    Structure {
        group: Group,
        workspaces: Vec<Workspace>,
        boards: Vec<WhiteboardDTO>,
    },
    Content {
        elements: Vec<WhiteboardElementDTO>,
        cells: Vec<NotebookCellDTO>,
    },
    Metadata {
        chats: Vec<ChatDTO>,
        files: Vec<FileDTO>,
        integrations: Vec<IntegrationBindingDTO>,
        board_metadata: Vec<BoardMetadataDTO>,
    },
    Complete,
}

// ═══════════════════════════════════════════════════════════════════════════
// TOPIC ID HELPERS
// ═══════════════════════════════════════════════════════════════════════════

fn make_discovery_topic_id() -> TopicId {
    let topic_str = format!("cyan/discovery/{}", DISCOVERY_KEY);
    let hash = blake3::hash(topic_str.as_bytes());
    let bytes: [u8; 32] = hash.as_bytes()[..32].try_into().unwrap();
    TopicId::from_bytes(bytes)
}

fn make_group_topic_id(group_id: &str) -> TopicId {
    let topic_str = format!("cyan/group/{}", group_id);
    let hash = blake3::hash(topic_str.as_bytes());
    let bytes: [u8; 32] = hash.as_bytes()[..32].try_into().unwrap();
    TopicId::from_bytes(bytes)
}

// ═══════════════════════════════════════════════════════════════════════════
// MAIN
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("iroh_gossip=warn".parse()?)
                .add_directive("iroh=info".parse()?),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("host") => run_host().await,
        Some("join") => run_join().await,
        _ => {
            println!("╔═══════════════════════════════════════════════════════════════╗");
            println!("║     Full Protocol Test (Discovery → Snapshot)                 ║");
            println!("╠═══════════════════════════════════════════════════════════════╣");
            println!("║                                                               ║");
            println!("║  Machine A:  ./snapshot_test host                             ║");
            println!("║  Machine B:  ./snapshot_test join                             ║");
            println!("║                                                               ║");
            println!("║  No NODE_ID needed - peers discover via bootstrap!            ║");
            println!("║                                                               ║");
            println!("╚═══════════════════════════════════════════════════════════════╝");
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// HOST - Has data, responds to discovery + snapshot requests
// ═══════════════════════════════════════════════════════════════════════════

async fn run_host() -> Result<()> {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║              PROTOCOL TEST - HOST (with Router)               ║");
    println!("╠═══════════════════════════════════════════════════════════════╣");
    println!("║  Testing: Discovery → GroupsExchange → PeerIntro → Snapshot   ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Generate identity
    let mut rng = ChaCha8Rng::from_os_rng();
    let secret_key = SecretKey::generate(&mut rng);
    let node_id = secret_key.public().to_string();

    println!("📱 My node ID: {}", node_id);
    println!("📦 Test group: {}...", &TEST_GROUP_ID[..16]);

    // Setup relay
    let relay_url = RelayUrl::from_str(RELAY_URL)?;
    let relay_mode = RelayMode::Custom(RelayMap::from(relay_url));

    // Build endpoint
    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![
            iroh_gossip::ALPN.to_vec(),
            SNAPSHOT_ALPN.to_vec(),
        ])
        .relay_mode(relay_mode)
        .bind()
        .await?;

    println!("✅ Endpoint created");

    // Create gossip (wrapped in Arc for sharing with Router)
    let gossip = Arc::new(Gossip::builder()
        .spawn(endpoint.clone()));

    // Create snapshot handler with notification channel
    let (snapshot_notify_tx, mut snapshot_notify_rx) = mpsc::unbounded_channel();
    let snapshot_handler = SnapshotHandler {
        notify_tx: snapshot_notify_tx,
    };

    // Setup Router with ALL protocols
    let _router = Router::builder(endpoint.clone())
        .accept(iroh_gossip::ALPN, gossip.clone())
        .accept(SNAPSHOT_ALPN, snapshot_handler)
        .spawn();

    println!("✅ Router spawned (gossip + snapshot)");

    // Get bootstrap peer
    let bootstrap_pk = PublicKey::from_str(BOOTSTRAP_NODE_ID)?;

    // ─────────────────────────────────────────────────────────────────────
    // STEP 1: Subscribe to DISCOVERY topic
    // ─────────────────────────────────────────────────────────────────────

    let discovery_topic_id = make_discovery_topic_id();
    println!("\n📡 [STEP 1] Subscribing to discovery topic...");

    let discovery_topic = gossip
        .subscribe_and_join(discovery_topic_id, vec![bootstrap_pk]).await?;
    let (discovery_tx, mut discovery_rx) = discovery_topic.split();

    println!("   ✓ Subscribed to discovery topic");

    // Wait for relay connection
    println!("⏳ Waiting for relay connection...");
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("✅ Ready\n");

    // ─────────────────────────────────────────────────────────────────────
    // STEP 2: Broadcast our GroupsExchange
    // ─────────────────────────────────────────────────────────────────────

    println!("📤 [STEP 2] Broadcasting GroupsExchange...");
    let groups_exchange = DiscoveryMessage::GroupsExchange {
        node_id: node_id.clone(),
        local_groups: vec![TEST_GROUP_ID.to_string()],
    };
    let data = serde_json::to_vec(&groups_exchange)?;
    discovery_tx.broadcast(Bytes::from(data)).await?;
    println!("   ✓ GroupsExchange sent (groups: [{}...])", &TEST_GROUP_ID[..16]);

    // Track known peers and state
    let mut known_peers: HashSet<String> = HashSet::new();
    let mut group_topic_tx: Option<iroh_gossip::api::GossipSender> = None;
    let mut joiner_peer_id: Option<String> = None;

    // ─────────────────────────────────────────────────────────────────────
    // Main event loop
    // ─────────────────────────────────────────────────────────────────────

    println!("\n📡 Listening for peers...\n");

    loop {
        tokio::select! {
            // Discovery topic events
            event = discovery_rx.next() => {
                match event {
                    Some(Ok(iroh_gossip::api::Event::Received(msg))) => {
                        let from = msg.delivered_from.to_string();

                        if from == node_id {
                            continue; // Ignore own messages
                        }

                        if let Ok(disc_msg) = serde_json::from_slice::<DiscoveryMessage>(&msg.content) {
                            match disc_msg {
                                DiscoveryMessage::GroupsExchange { node_id: peer_node_id, local_groups } => {
                                    println!("📥 [DISCOVERY] GroupsExchange from {}...", &peer_node_id[..16]);
                                    println!("   Their groups: {:?}", local_groups.iter().map(|g| &g[..16.min(g.len())]).collect::<Vec<_>>());

                                    // Check for shared groups
                                    if local_groups.contains(&TEST_GROUP_ID.to_string()) {
                                        println!("   ✓ Shared group found: {}...", &TEST_GROUP_ID[..16]);

                                        if !known_peers.contains(&peer_node_id) {
                                            known_peers.insert(peer_node_id.clone());
                                            joiner_peer_id = Some(peer_node_id.clone());

                                            // ─────────────────────────────────────────
                                            // STEP 3: Send PeerIntroduction
                                            // ─────────────────────────────────────────

                                            println!("\n📤 [STEP 3] Sending PeerIntroduction...");
                                            let peer_intro = DiscoveryMessage::PeerIntroduction {
                                                group_id: TEST_GROUP_ID.to_string(),
                                                peers: vec![node_id.clone(), peer_node_id.clone()],
                                            };
                                            let data = serde_json::to_vec(&peer_intro)?;
                                            discovery_tx.broadcast(Bytes::from(data)).await?;
                                            println!("   ✓ PeerIntroduction sent");

                                            // ─────────────────────────────────────────
                                            // STEP 4: Subscribe to GROUP topic
                                            // ─────────────────────────────────────────

                                            if group_topic_tx.is_none() {
                                                println!("\n📡 [STEP 4] Subscribing to group topic...");
                                                let group_topic_id = make_group_topic_id(TEST_GROUP_ID);

                                                // subscribe_and_join with bootstrap (guaranteed on topic)
                                                let group_topic = gossip
                                                    .subscribe_and_join(group_topic_id, vec![bootstrap_pk]).await?;
                                                let (gtx, grx) = group_topic.split();
                                                println!("   ✓ Subscribed via bootstrap");

                                                // Add discovered peer via join_peers
                                                let peer_pk = PublicKey::from_str(&peer_node_id)?;
                                                let _ = gtx.join_peers(vec![peer_pk]).await;
                                                println!("   ✓ Added peer via join_peers");

                                                group_topic_tx = Some(gtx);

                                                // Spawn group topic listener
                                                let my_node_id = node_id.clone();
                                                let endpoint_clone = endpoint.clone();
                                                tokio::spawn(async move {
                                                    handle_group_topic_host(grx, my_node_id, endpoint_clone).await;
                                                });
                                            }
                                        }
                                    }
                                }
                                DiscoveryMessage::PeerIntroduction { group_id, peers } => {
                                    println!("📥 [DISCOVERY] PeerIntroduction for {}...", &group_id[..16.min(group_id.len())]);
                                    println!("   Peers: {:?}", peers.iter().map(|p| &p[..16.min(p.len())]).collect::<Vec<_>>());
                                }
                            }
                        }
                    }
                    Some(Ok(iroh_gossip::api::Event::NeighborUp(peer))) => {
                        println!("🟢 [DISCOVERY] Neighbor UP: {}...", &peer.to_string()[..16]);

                        // Rebroadcast GroupsExchange
                        let groups_exchange = DiscoveryMessage::GroupsExchange {
                            node_id: node_id.clone(),
                            local_groups: vec![TEST_GROUP_ID.to_string()],
                        };
                        let data = serde_json::to_vec(&groups_exchange)?;
                        discovery_tx.broadcast(Bytes::from(data)).await?;
                    }
                    Some(Ok(iroh_gossip::api::Event::NeighborDown(peer))) => {
                        println!("🔴 [DISCOVERY] Neighbor DOWN: {}...", &peer.to_string()[..16]);
                    }
                    _ => {}
                }
            }

            // Snapshot handled by Router - just log notifications
            Some(peer_id) = snapshot_notify_rx.recv() => {
                println!("📥 [INFO] Snapshot request processed for peer {}...", &peer_id[..16]);
            }

            _ = tokio::signal::ctrl_c() => {
                println!("\n👋 Shutting down...");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_group_topic_host(
    mut rx: iroh_gossip::api::GossipReceiver,
    my_node_id: String,
    endpoint: Endpoint,
) {
    println!("🎧 [GROUP] Listening on group topic...");

    while let Some(event) = rx.next().await {
        match event {
            Ok(iroh_gossip::api::Event::Received(msg)) => {
                let from = msg.delivered_from.to_string();

                if from == my_node_id {
                    continue;
                }

                if let Ok(group_msg) = serde_json::from_slice::<GroupMessage>(&msg.content) {
                    match group_msg {
                        GroupMessage::RequestSnapshot { from_peer } => {
                            println!("\n═══════════════════════════════════════════════════════════════════");
                            println!("📥 [STEP 5] RequestSnapshot from {}...", &from_peer[..16]);
                            println!("═══════════════════════════════════════════════════════════════════");

                            // We respond by accepting the SNAPSHOT_ALPN connection
                            // (handled in main loop above)
                            println!("   Waiting for direct QUIC connection...");
                        }
                        GroupMessage::SnapshotAvailable { source, group_id } => {
                            println!("📥 [GROUP] SnapshotAvailable from {}...", &source[..16]);
                        }
                    }
                }
            }
            Ok(iroh_gossip::api::Event::NeighborUp(peer)) => {
                println!("🟢 [GROUP] Peer joined: {}...", &peer.to_string()[..16]);
            }
            Ok(iroh_gossip::api::Event::NeighborDown(peer)) => {
                println!("🔴 [GROUP] Peer left: {}...", &peer.to_string()[..16]);
            }
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// JOIN - Empty group (invited), discovers host, requests snapshot
// ═══════════════════════════════════════════════════════════════════════════

async fn run_join() -> Result<()> {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║              PROTOCOL TEST - JOINER (with Router)             ║");
    println!("╠═══════════════════════════════════════════════════════════════╣");
    println!("║  Simulates: Invited to group → Discover host → Get snapshot   ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Generate identity
    let mut rng = ChaCha8Rng::from_os_rng();
    let secret_key = SecretKey::generate(&mut rng);
    let node_id = secret_key.public().to_string();

    println!("📱 My node ID: {}", node_id);
    println!("📦 Invited to group: {}... (empty, need sync)", &TEST_GROUP_ID[..16]);

    // Setup relay
    let relay_url = RelayUrl::from_str(RELAY_URL)?;
    let relay_mode = RelayMode::Custom(RelayMap::from(relay_url));

    // Build endpoint
    let endpoint = Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![
            iroh_gossip::ALPN.to_vec(),
            SNAPSHOT_ALPN.to_vec(),
        ])
        .relay_mode(relay_mode)
        .bind()
        .await?;

    println!("✅ Endpoint created");

    // Create gossip (wrapped in Arc for sharing with Router)
    let gossip = Arc::new(Gossip::builder()
        .spawn(endpoint.clone()));

    // Joiner doesn't serve snapshots, but still needs Router for gossip
    // Create a dummy handler that just logs (joiner won't receive snapshot requests)
    let (snapshot_notify_tx, _snapshot_notify_rx) = mpsc::unbounded_channel();
    let snapshot_handler = SnapshotHandler {
        notify_tx: snapshot_notify_tx,
    };

    // Setup Router
    let _router = Router::builder(endpoint.clone())
        .accept(iroh_gossip::ALPN, gossip.clone())
        .accept(SNAPSHOT_ALPN, snapshot_handler)
        .spawn();

    println!("✅ Router spawned (gossip + snapshot)");

    // Get bootstrap peer
    let bootstrap_pk = PublicKey::from_str(BOOTSTRAP_NODE_ID)?;

    // ─────────────────────────────────────────────────────────────────────
    // STEP 1: Subscribe to DISCOVERY topic
    // ─────────────────────────────────────────────────────────────────────

    let discovery_topic_id = make_discovery_topic_id();
    println!("\n📡 [STEP 1] Subscribing to discovery topic...");

    let discovery_topic = gossip
        .subscribe_and_join(discovery_topic_id, vec![bootstrap_pk]).await?;
    let (discovery_tx, mut discovery_rx) = discovery_topic.split();

    println!("   ✓ Subscribed to discovery topic");

    // Wait for relay connection
    println!("⏳ Waiting for relay connection...");
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("✅ Ready\n");

    // ─────────────────────────────────────────────────────────────────────
    // STEP 2: Broadcast our GroupsExchange
    // ─────────────────────────────────────────────────────────────────────

    println!("📤 [STEP 2] Broadcasting GroupsExchange...");
    let groups_exchange = DiscoveryMessage::GroupsExchange {
        node_id: node_id.clone(),
        local_groups: vec![TEST_GROUP_ID.to_string()],  // We have the group (invited)
    };
    let data = serde_json::to_vec(&groups_exchange)?;
    discovery_tx.broadcast(Bytes::from(data)).await?;
    println!("   ✓ GroupsExchange sent (groups: [{}...])", &TEST_GROUP_ID[..16]);

    // Track state
    let mut host_peer_id: Option<String> = None;
    let mut snapshot_complete = false;
    let start = Instant::now();
    let timeout = Duration::from_secs(60);

    // ─────────────────────────────────────────────────────────────────────
    // Main event loop - wait for host discovery
    // ─────────────────────────────────────────────────────────────────────

    println!("\n📡 Waiting to discover host...\n");

    loop {
        if start.elapsed() > timeout {
            println!("❌ Timeout waiting for sync");
            break;
        }

        if snapshot_complete {
            break;
        }

        tokio::select! {
            event = discovery_rx.next() => {
                match event {
                    Some(Ok(iroh_gossip::api::Event::Received(msg))) => {
                        let from = msg.delivered_from.to_string();

                        if from == node_id {
                            continue;
                        }

                        if let Ok(disc_msg) = serde_json::from_slice::<DiscoveryMessage>(&msg.content) {
                            match disc_msg {
                                DiscoveryMessage::GroupsExchange { node_id: peer_node_id, local_groups } => {
                                    println!("📥 [DISCOVERY] GroupsExchange from {}...", &peer_node_id[..16]);
                                    println!("   Their groups: {:?}", local_groups.iter().map(|g| &g[..16.min(g.len())]).collect::<Vec<_>>());

                                    if local_groups.contains(&TEST_GROUP_ID.to_string()) && host_peer_id.is_none() {
                                        println!("   ✓ Found host with our group!");
                                        host_peer_id = Some(peer_node_id.clone());
                                    }
                                }
                                DiscoveryMessage::PeerIntroduction { group_id, peers } => {
                                    println!("\n📥 [STEP 3] PeerIntroduction received!");
                                    println!("   Group: {}...", &group_id[..16.min(group_id.len())]);
                                    println!("   Peers: {:?}", peers.iter().map(|p| &p[..16.min(p.len())]).collect::<Vec<_>>());

                                    // Find the host peer (not us)
                                    for peer_str in &peers {
                                        if peer_str != &node_id && host_peer_id.is_none() {
                                            host_peer_id = Some(peer_str.clone());
                                            println!("   ✓ Host identified: {}...", &peer_str[..16]);
                                        }
                                    }

                                    // Now subscribe to group topic and request snapshot
                                    if let Some(ref host_id) = host_peer_id {
                                        // ─────────────────────────────────────────
                                        // STEP 4: Subscribe to GROUP topic
                                        // ─────────────────────────────────────────

                                        println!("\n📡 [STEP 4] Subscribing to group topic...");
                                        let group_topic_id = make_group_topic_id(TEST_GROUP_ID);

                                        // Subscribe with ONLY bootstrap (always online)
                                        let group_topic = gossip
                                            .subscribe_and_join(group_topic_id, vec![bootstrap_pk]).await?;
                                        let (group_tx, _group_rx) = group_topic.split();
                                        println!("   ✓ Subscribed via bootstrap");

                                        // NOW add host via join_peers (non-blocking)
                                        let host_pk = PublicKey::from_str(host_id)?;
                                        if let Err(e) = group_tx.join_peers(vec![host_pk]).await {
                                            println!("   ⚠️ join_peers: {} (will retry)", e);
                                        } else {
                                            println!("   ✓ Added host via join_peers");
                                        }

                                        // Give time for mesh to form
                                        tokio::time::sleep(Duration::from_millis(500)).await;

                                        // ─────────────────────────────────────────
                                        // STEP 5: Request snapshot via gossip
                                        // ─────────────────────────────────────────

                                        println!("\n📤 [STEP 5] Broadcasting RequestSnapshot...");
                                        let request = GroupMessage::RequestSnapshot {
                                            from_peer: node_id.clone(),
                                        };
                                        let data = serde_json::to_vec(&request)?;
                                        group_tx.broadcast(Bytes::from(data)).await?;
                                        println!("   ✓ RequestSnapshot sent");

                                        // ─────────────────────────────────────────
                                        // STEP 6: Direct QUIC connection for snapshot
                                        // ─────────────────────────────────────────

                                        println!("\n📡 [STEP 6] Connecting to host for snapshot...");

                                        match download_snapshot(&endpoint, host_id, TEST_GROUP_ID).await {
                                            Ok(_) => {
                                                snapshot_complete = true;
                                                println!("\n╔═══════════════════════════════════════════════════════════════╗");
                                                println!("║ ✅ FULL PROTOCOL TEST PASSED                                  ║");
                                                println!("╠═══════════════════════════════════════════════════════════════╣");
                                                println!("║ Discovery → GroupsExchange → PeerIntro → Snapshot ✓           ║");
                                                println!("║ Total time: {:>10?}                                      ║", start.elapsed());
                                                println!("╚═══════════════════════════════════════════════════════════════╝\n");
                                            }
                                            Err(e) => {
                                                println!("🔴 Snapshot download failed: {}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(iroh_gossip::api::Event::NeighborUp(peer))) => {
                        println!("🟢 [DISCOVERY] Neighbor UP: {}...", &peer.to_string()[..16]);

                        // Rebroadcast GroupsExchange
                        let groups_exchange = DiscoveryMessage::GroupsExchange {
                            node_id: node_id.clone(),
                            local_groups: vec![TEST_GROUP_ID.to_string()],
                        };
                        let data = serde_json::to_vec(&groups_exchange)?;
                        discovery_tx.broadcast(Bytes::from(data)).await?;
                    }
                    Some(Ok(iroh_gossip::api::Event::NeighborDown(peer))) => {
                        println!("🔴 [DISCOVERY] Neighbor DOWN: {}...", &peer.to_string()[..16]);
                    }
                    _ => {}
                }
            }

            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// SNAPSHOT DOWNLOAD (Direct QUIC)
// ═══════════════════════════════════════════════════════════════════════════

async fn download_snapshot(endpoint: &Endpoint, host_id: &str, group_id: &str) -> Result<()> {
    let host_pk = PublicKey::from_str(host_id)?;

    let conn = tokio::time::timeout(
        Duration::from_secs(30),
        endpoint.connect(host_pk, SNAPSHOT_ALPN),
    )
        .await
        .map_err(|_| anyhow::anyhow!("Connection timeout"))?
        .map_err(|e| anyhow::anyhow!("Connection failed: {}", e))?;

    println!("   ✓ Connected to host");

    let (mut send, mut recv) = conn.open_bi().await?;

    // Send group_id request
    let group_id_bytes = group_id.as_bytes();
    let len = (group_id_bytes.len() as u32).to_be_bytes();
    send.write_all(&len).await?;
    send.write_all(group_id_bytes).await?;
    send.finish()?;

    println!("   ✓ Requested snapshot for group");
    println!("\n📥 Receiving frames...\n");

    // Receive frames
    let mut frame_count = 0;
    loop {
        frame_count += 1;

        let mut len_buf = [0u8; 4];
        match recv.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) => {
                if frame_count == 1 {
                    return Err(anyhow::anyhow!("Failed to read first frame: {}", e));
                }
                break;
            }
        }
        let frame_len = u32::from_be_bytes(len_buf) as usize;

        let mut frame_data = vec![0u8; frame_len];
        recv.read_exact(&mut frame_data).await?;

        let frame: SnapshotFrame = serde_json::from_slice(&frame_data)?;

        match &frame {
            SnapshotFrame::Structure { group, workspaces, boards } => {
                println!("   📦 Frame {}: STRUCTURE", frame_count);
                println!("      - Group: {} ({}...)", group.name, &group.id[..16]);
                println!("      - Workspaces: {}", workspaces.len());
                println!("      - Boards: {}", boards.len());
            }
            SnapshotFrame::Content { elements, cells } => {
                println!("   📦 Frame {}: CONTENT", frame_count);
                println!("      - Elements: {}", elements.len());
                println!("      - Cells: {}", cells.len());
            }
            SnapshotFrame::Metadata { chats, files, integrations, board_metadata } => {
                println!("   📦 Frame {}: METADATA", frame_count);
                println!("      - Chats: {}", chats.len());
                println!("      - Files: {}", files.len());
                println!("      - Integrations: {}", integrations.len());
                println!("      - Board metadata: {}", board_metadata.len());
            }
            SnapshotFrame::Complete => {
                println!("   📦 Frame {}: COMPLETE ✓", frame_count);
                break;
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// SNAPSHOT SERVER (Host serves snapshot via direct QUIC)
// ═══════════════════════════════════════════════════════════════════════════

async fn handle_snapshot_server(conn: Connection) -> Result<()> {
    let peer_id = conn.remote_id();
    println!("   📥 Connection from: {}...", &peer_id.to_string()[..16]);

    let (mut send, mut recv) = conn.accept_bi().await?;

    // Read group_id request
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut group_id_bytes = vec![0u8; len];
    recv.read_exact(&mut group_id_bytes).await?;
    let group_id = String::from_utf8(group_id_bytes)?;

    println!("   📋 Requested group: {}...", &group_id[..16.min(group_id.len())]);

    // Send frames
    send_frame(&mut send, &create_test_structure()).await?;
    println!("   📤 Sent Structure frame");

    send_frame(&mut send, &create_test_content()).await?;
    println!("   📤 Sent Content frame");

    send_frame(&mut send, &create_test_metadata()).await?;
    println!("   📤 Sent Metadata frame");

    send_frame(&mut send, &SnapshotFrame::Complete).await?;
    println!("   📤 Sent Complete frame");

    // CRITICAL: Wait for peer to receive
    send.finish()?;
    let _ = send.stopped().await;

    println!("   ✅ Snapshot transfer complete!\n");

    Ok(())
}

async fn send_frame(send: &mut iroh::endpoint::SendStream, frame: &SnapshotFrame) -> Result<()> {
    let data = serde_json::to_vec(frame)?;
    let len = (data.len() as u32).to_be_bytes();
    send.write_all(&len).await?;
    send.write_all(&data).await?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// TEST DATA
// ═══════════════════════════════════════════════════════════════════════════

fn create_test_structure() -> SnapshotFrame {
    SnapshotFrame::Structure {
        group: Group {
            id: TEST_GROUP_ID.to_string(),
            name: "Test Sync Group".to_string(),
            icon: "folder.fill".to_string(),
            color: "#00AEEF".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        },
        workspaces: vec![Workspace {
            id: TEST_WORKSPACE_ID.to_string(),
            group_id: TEST_GROUP_ID.to_string(),
            name: "Main Workspace".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }],
        boards: vec![WhiteboardDTO {
            id: TEST_BOARD_ID.to_string(),
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            name: "Test Canvas".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }],
    }
}

fn create_test_content() -> SnapshotFrame {
    let mut elements = Vec::new();
    for i in 0..5 {
        elements.push(WhiteboardElementDTO {
            id: format!("elem-{:03}", i),
            board_id: TEST_BOARD_ID.to_string(),
            element_type: "rectangle".to_string(),
            x: (i * 100) as f64,
            y: (i * 50) as f64,
            width: 200.0,
            height: 100.0,
            z_index: i,
            style_json: Some(format!("{{\"fill\":\"#FF{:02X}00\"}}", i * 50)),
            content_json: Some(format!("{{\"text\":\"Element {}\"}}", i)),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        });
    }

    let mut cells = Vec::new();
    for i in 0..3 {
        cells.push(NotebookCellDTO {
            id: format!("cell-{:03}", i),
            board_id: TEST_BOARD_ID.to_string(),
            cell_type: "code".to_string(),
            cell_order: i,
            content: Some(format!("# Cell {}\nprint('hello')", i)),
            output: Some("hello".to_string()),
            collapsed: false,
            height: Some(100.0),
            metadata_json: None,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        });
    }

    SnapshotFrame::Content { elements, cells }
}

fn create_test_metadata() -> SnapshotFrame {
    SnapshotFrame::Metadata {
        chats: vec![
            ChatDTO {
                id: "chat-001".to_string(),
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                message: "Welcome!".to_string(),
                author: "host".to_string(),
                parent_id: None,
                timestamp: chrono::Utc::now().timestamp(),
            },
        ],
        files: vec![FileDTO {
            id: "file-001".to_string(),
            group_id: Some(TEST_GROUP_ID.to_string()),
            workspace_id: Some(TEST_WORKSPACE_ID.to_string()),
            board_id: Some(TEST_BOARD_ID.to_string()),
            name: "test.pdf".to_string(),
            hash: "abc123".to_string(),
            size: 1024,
            source_peer: None,
            local_path: None,
            created_at: chrono::Utc::now().timestamp(),
        }],
        integrations: vec![],
        board_metadata: vec![BoardMetadataDTO {
            board_id: TEST_BOARD_ID.to_string(),
            labels: vec!["test".to_string()],
            rating: 5,
            view_count: 1,
            contains_model: None,
            contains_skills: vec![],
            board_type: "canvas".to_string(),
            last_accessed: chrono::Utc::now().timestamp(),
            is_pinned: false,
        }],
    }
}