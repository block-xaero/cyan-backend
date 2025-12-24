// src/lib.rs
#![allow(clippy::too_many_arguments)]

extern crate core; // Re-export xaeroid FFI functions for Swift
mod xaero_ffi {
    pub use xaeroid::xaero_create_pass_json;
    pub use xaeroid::xaero_create_pass_with_profile;
    pub use xaeroid::xaero_derive_identity;
    pub use xaeroid::xaero_free_string;
    pub use xaeroid::xaero_generate_json;
    pub use xaeroid::xaero_sign_with_key;
}
pub use xaero_ffi::*;

mod integration_bridge;

pub use integration_bridge::IntegrationBridge;

mod ai_bridge;
pub use ai_bridge::AIBridge;

use crate::NetworkCommand::RequestSnapshot;
use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures::StreamExt;
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey};
use iroh_blobs::store::fs::FsStore as BlobStore;
use iroh_gossip::{
    api::{Event as GossipEvent, GossipTopic},
    proto::state::TopicId,
    Gossip,
};
use once_cell::sync::OnceCell;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{c_char, CStr, CString},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, mpsc::error::SendError},
};

// ---------- Globals ----------
static RUNTIME: OnceCell<Runtime> = OnceCell::new();
static SYSTEM: OnceCell<Arc<CyanSystem>> = OnceCell::new();
static DISCOVERY_KEY: OnceCell<String> = OnceCell::new();
static DATA_DIR: OnceCell<PathBuf> = OnceCell::new();
static NODE_ID: OnceCell<String> = OnceCell::new();
static AI_RESPONSE_QUEUE: OnceCell<Mutex<VecDeque<String>>> = OnceCell::new();

// ---------- Core types ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub struct Group {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub struct Workspace {
    pub id: String,
    pub group_id: String,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Object {
    Whiteboard {
        id: String,
        workspace_id: String,
        name: String,
        created_at: i64,
    },
    File {
        id: String,
        group_id: Option<String>,
        workspace_id: Option<String>,
        name: String,
        hash: String,
        size: u64,
        created_at: i64,
    },
    Chat {
        id: String,
        workspace_id: String,
        message: String,
        author: String,
        parent_id: Option<String>,
        timestamp: i64,
    },
}

// ---------- Gossip events ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NetworkEvent {
    /// A group snapshot available sent as a
    /// network event from source/peer_id
    /// the current running peer who receives this ack then
    /// QUIC connects and receives the snapshot
    GroupSnapshotAvailable {
        source: String,
        group_id: String,
    },
    GroupCreated(Group),
    GroupRenamed {
        id: String,
        name: String,
    },
    GroupDeleted {
        id: String,
    },
    WorkspaceCreated(Workspace),
    WorkspaceRenamed {
        id: String,
        name: String,
    },
    WorkspaceDeleted {
        id: String,
    },
    BoardCreated {
        id: String,
        workspace_id: String,
        name: String,
        created_at: i64,
    },
    BoardRenamed {
        id: String,
        name: String,
    },
    BoardDeleted {
        id: String,
    },
    FileAvailable {
        id: String,
        group_id: Option<String>,
        workspace_id: Option<String>,
        board_id: Option<String>,
        name: String,
        hash: String,
        size: u64,
        source_peer: String,
        created_at: i64,
    },
    // ---- Chat events ----
    ChatSent {
        id: String,
        workspace_id: String,
        message: String,
        author: String,
        parent_id: Option<String>,
        timestamp: i64,
    },
    ChatDeleted {
        id: String,
    },
    // ---- Whiteboard element events ----
    WhiteboardElementAdded {
        id: String,
        board_id: String,
        element_type: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        z_index: i32,
        style_json: Option<String>,
        content_json: Option<String>,
        created_at: i64,
        updated_at: i64,
    },
    WhiteboardElementUpdated {
        id: String,
        board_id: String,
        element_type: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        z_index: i32,
        style_json: Option<String>,
        content_json: Option<String>,
        updated_at: i64,
    },
    WhiteboardElementDeleted {
        id: String,
        board_id: String,
    },
    WhiteboardCleared {
        board_id: String,
    },
    // ---- Notebook cell events ----
    NotebookCellAdded {
        id: String,
        board_id: String,
        cell_type: String,
        cell_order: i32,
        content: Option<String>,
    },
    NotebookCellUpdated {
        id: String,
        board_id: String,
        cell_type: String,
        cell_order: i32,
        content: Option<String>,
        output: Option<String>,
        collapsed: bool,
        height: Option<f64>,
        metadata_json: Option<String>,
    },
    NotebookCellDeleted {
        id: String,
        board_id: String,
    },
    NotebookCellsReordered {
        board_id: String,
        cell_ids: Vec<String>,
    },
    BoardModeChanged {
        board_id: String,
        mode: String,
    },
    // ---- Board metadata events ----
    BoardMetadataUpdated {
        board_id: String,
        labels: Vec<String>,
        rating: i32,
        contains_model: Option<String>,
        contains_skills: Vec<String>,
    },
    BoardLabelsUpdated {
        board_id: String,
        labels: Vec<String>,
    },
    BoardRated {
        board_id: String,
        rating: i32,
    },
    // ---- User profile events ----
    ProfileUpdated {
        node_id: String,
        display_name: String,
        avatar_hash: Option<String>,
    },
}

// ---------- Internal commands ----------
#[derive(Debug, Serialize, Deserialize)]
enum NetworkCommand {
    /// Requests group snapshot
    RequestSnapshot {
        from_peer: String,
    },
    JoinGroup {
        group_id: String,
    },
    Broadcast {
        group_id: String,
        event: NetworkEvent,
    },
    UploadToGroup {
        group_id: String,
        path: String,
    },
    UploadToWorkspace {
        workspace_id: String,
        path: String,
    },
    DeleteGroup { id: String },
    DeleteWorkspace { id: String },
    DeleteBoard { id: String },
    DeleteChat { id: String },
    /// Start a direct QUIC chat stream with a peer
    StartChatStream {
        peer_id: String,
        workspace_id: String,
    },
    /// Send a message on an existing direct chat stream
    SendDirectChat {
        peer_id: String,
        workspace_id: String,
        message: String,
        parent_id: Option<String>,
    },
    /// Request file download from peer
    RequestFileDownload {
        file_id: String,
        hash: String,
        source_peer: String,
    },
}

// ---------- Board Metadata ----------
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoardMetadata {
    pub board_id: String,
    pub labels: Vec<String>,
    pub rating: i32,
    pub view_count: i32,
    pub contains_model: Option<String>,
    pub contains_skills: Vec<String>,
    pub board_type: String,
    pub last_accessed: i64,
}

/// Message sent through channel to chat stream writer task
#[derive(Debug, Clone)]
struct DirectChatOutbound {
    workspace_id: String,
    message: String,
    parent_id: Option<String>,
}

/// File transfer protocol messages (length-prefixed JSON over QUIC)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "msg_type")]
pub enum FileTransferMsg {
    Request { file_id: String, hash: String },
    Metadata { file_id: String, size: u64, name: String },
    NotFound { file_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum CommandMsg {
    CreateGroup {
        name: String,
        icon: String,
        color: String,
    },
    RenameGroup {
        id: String,
        name: String,
    },
    DeleteGroup {
        id: String,
    },

    CreateWorkspace {
        group_id: String,
        name: String,
    },
    RenameWorkspace {
        id: String,
        name: String,
    },
    DeleteWorkspace {
        id: String,
    },

    CreateBoard {
        workspace_id: String,
        name: String,
    },
    RenameBoard {
        id: String,
        name: String,
    },
    DeleteBoard {
        id: String,
    },

    // ---- Chat commands ----
    SendChat {
        workspace_id: String,
        message: String,
        parent_id: Option<String>,
    },
    DeleteChat {
        id: String,
    },
    LoadChatHistory {
        workspace_id: String,
    },

    // ---- Direct Message commands ----
    StartDirectChat {
        peer_id: String,
        workspace_id: String,
    },
    SendDirectMessage {
        peer_id: String,
        workspace_id: String,
        message: String,
        parent_id: Option<String>,
    },
    LoadDirectMessageHistory {
        peer_id: String,
    },

    Snapshot {},
    SeedDemoIfEmpty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SwiftEvent {
    Network(NetworkEvent),
    TreeLoaded(String), // The JSON tree
    GroupDeleted { id: String },
    WorkspaceDeleted { id: String },
    BoardDeleted { id: String },
    ChatDeleted { id: String },
    /// Direct chat stream established with peer
    ChatStreamReady { peer_id: String, workspace_id: String },
    /// Direct chat stream closed
    ChatStreamClosed { peer_id: String },
    /// Peer joined a group topic
    PeerJoined { group_id: String, peer_id: String },
    /// Peer left a group topic
    PeerLeft { group_id: String, peer_id: String },
    /// Status update for UI (syncing, downloading, etc.)
    StatusUpdate { message: String },
    /// File download progress (0.0 to 1.0)
    FileDownloadProgress { file_id: String, progress: f64 },
    /// File download completed
    FileDownloaded { file_id: String, local_path: String },
    /// File download failed
    FileDownloadFailed { file_id: String, error: String },
    /// Integration event for console display
    IntegrationEvent {
        id: String,
        scope_id: String,
        source: String,
        summary: String,
        context: String,
        url: Option<String>,
        ts: u64,
    },
    /// Integration status change
    IntegrationStatus {
        scope_id: String,
        integration_type: String,
        status: String,
        message: Option<String>,
    },
    /// Integration graph for console tree display
    IntegrationGraph {
        scope_id: String,
        graph_json: String,  // Serialized IntegrationGraph from integration_bridge
    },
    /// AI proactive insight generated
    AIInsight {
        insight_json: String,
    },
    /// Direct message received from peer
    DirectMessageReceived {
        id: String,
        peer_id: String,
        message: String,
        timestamp: i64,
        is_incoming: bool,
    },
}

impl SwiftEvent {
    /// Returns true if this is an integration-related event that should go to the integration buffer
    fn is_integration_event(&self) -> bool {
        matches!(
            self,
            SwiftEvent::IntegrationEvent { .. }
                | SwiftEvent::AIInsight { .. }
                | SwiftEvent::IntegrationStatus { .. }
                | SwiftEvent::IntegrationGraph { .. }
        )
    }
}

// ---------- System ----------
pub struct CyanSystem {
    pub node_id: String,
    pub secret_key: SecretKey,
    pub command_tx: mpsc::UnboundedSender<CommandMsg>,
    pub event_tx: mpsc::UnboundedSender<SwiftEvent>,
    pub network_tx: mpsc::UnboundedSender<NetworkCommand>,
    /// Buffer for general events (FileTree, Network, etc.) - polled by cyan_poll_events
    pub event_ffi_buffer: Arc<Mutex<VecDeque<String>>>,
    /// Buffer for integration events only - polled by cyan_poll_integration_events
    pub integration_event_buffer: Arc<Mutex<VecDeque<String>>>,
    pub db: Arc<Mutex<Connection>>,
    /// Peers per group, shared with NetworkActor for FFI queries
    pub peers_per_group: Arc<Mutex<HashMap<String, HashSet<PublicKey>>>>,
    /// Integration bridge for managing external integrations
    pub integration_bridge: Arc<IntegrationBridge>,
    /// AI bridge for XaeroAI integration
    pub ai_bridge: Arc<AIBridge>,
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            icon TEXT NOT NULL,
            color TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspaces (
            id TEXT PRIMARY KEY,
            group_id TEXT NOT NULL,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(group_id) REFERENCES groups(id)
        );
        CREATE TABLE IF NOT EXISTS objects (
            id TEXT PRIMARY KEY,
            group_id TEXT,
            workspace_id TEXT,
            board_id TEXT,
            type TEXT NOT NULL,
            name TEXT,
            hash TEXT,
            size INTEGER,
            source_peer TEXT,
            local_path TEXT,
            data BLOB,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(group_id) REFERENCES groups(id),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(id),
            FOREIGN KEY(board_id) REFERENCES objects(id)
        );
        CREATE TABLE IF NOT EXISTS whiteboard_elements (
            id TEXT PRIMARY KEY,
            board_id TEXT NOT NULL,
            element_type TEXT NOT NULL,
            x REAL NOT NULL,
            y REAL NOT NULL,
            width REAL NOT NULL,
            height REAL NOT NULL,
            z_index INTEGER DEFAULT 0,
            style_json TEXT,
            content_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(board_id) REFERENCES objects(id)
        );
        CREATE INDEX IF NOT EXISTS idx_whiteboard_elements_board ON whiteboard_elements(board_id);
        CREATE TABLE IF NOT EXISTS notebook_cells (
            id TEXT PRIMARY KEY,
            board_id TEXT NOT NULL,
            cell_type TEXT NOT NULL,
            cell_order INTEGER NOT NULL,
            content TEXT,
            output TEXT,
            collapsed INTEGER DEFAULT 0,
            height REAL,
            metadata_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(board_id) REFERENCES objects(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_notebook_cells_order ON notebook_cells(board_id, cell_order);
        CREATE TABLE IF NOT EXISTS board_metadata (
            board_id TEXT PRIMARY KEY,
            labels TEXT DEFAULT '[]',
            rating INTEGER DEFAULT 0,
            view_count INTEGER DEFAULT 0,
            contains_model TEXT,
            contains_skills TEXT DEFAULT '[]',
            board_type TEXT DEFAULT 'canvas',
            last_accessed INTEGER DEFAULT 0,
            FOREIGN KEY (board_id) REFERENCES objects(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_board_rating ON board_metadata(rating DESC);
        "#,
    )?;
    Ok(())
}

/// Run schema migrations for existing databases
fn run_migrations(conn: &Connection) -> Result<()> {
    // Check and add board_id column
    if conn.prepare("SELECT board_id FROM objects LIMIT 1").is_err() {
        tracing::info!("Migration: adding board_id column");
        let _ = conn.execute("ALTER TABLE objects ADD COLUMN board_id TEXT", []);
    }
    // Check and add source_peer column
    if conn.prepare("SELECT source_peer FROM objects LIMIT 1").is_err() {
        tracing::info!("Migration: adding source_peer column");
        let _ = conn.execute("ALTER TABLE objects ADD COLUMN source_peer TEXT", []);
    }
    // Check and add local_path column
    if conn.prepare("SELECT local_path FROM objects LIMIT 1").is_err() {
        tracing::info!("Migration: adding local_path column");
        let _ = conn.execute("ALTER TABLE objects ADD COLUMN local_path TEXT", []);
    }
    // Check and add cell_id column to whiteboard_elements
    if conn.prepare("SELECT cell_id FROM whiteboard_elements LIMIT 1").is_err() {
        tracing::info!("Migration: adding cell_id column to whiteboard_elements");
        let _ = conn.execute("ALTER TABLE whiteboard_elements ADD COLUMN cell_id TEXT", []);
    }
    // Check and add board_mode column to objects
    if conn.prepare("SELECT board_mode FROM objects LIMIT 1").is_err() {
        tracing::info!("Migration: adding board_mode column to objects");
        let _ = conn.execute("ALTER TABLE objects ADD COLUMN board_mode TEXT DEFAULT 'canvas'", []);
    }

    // Migrate existing 'freeform' values to 'canvas'
    let _ = conn.execute("UPDATE objects SET board_mode = 'canvas' WHERE board_mode = 'freeform' OR board_mode IS NULL", []);

    // Check and create board_metadata table if not exists
    if conn.prepare("SELECT board_id FROM board_metadata LIMIT 1").is_err() {
        tracing::info!("Migration: creating board_metadata table");
        let _ = conn.execute("CREATE TABLE IF NOT EXISTS board_metadata (board_id TEXT PRIMARY KEY, labels TEXT DEFAULT '[]', rating INTEGER DEFAULT 0, view_count INTEGER DEFAULT 0, contains_model TEXT, contains_skills TEXT DEFAULT '[]', board_type TEXT DEFAULT 'canvas', last_accessed INTEGER DEFAULT 0)", []);
        let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_board_rating ON board_metadata(rating DESC)", []);
    }

    // Check and create user_profiles table if not exists
    if conn.prepare("SELECT node_id FROM user_profiles LIMIT 1").is_err() {
        tracing::info!("Migration: creating user_profiles table");
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS user_profiles (
                node_id TEXT PRIMARY KEY,
                display_name TEXT,
                avatar_hash TEXT,
                status TEXT DEFAULT 'offline',
                last_seen INTEGER,
                updated_at INTEGER
            )",
            [],
        );
    }

    // Check and create direct_messages table if not exists
    if conn.prepare("SELECT id FROM direct_messages LIMIT 1").is_err() {
        tracing::info!("Migration: creating direct_messages table");
        let _ = conn.execute(
            "CREATE TABLE IF NOT EXISTS direct_messages (
                id TEXT PRIMARY KEY,
                peer_id TEXT NOT NULL,
                workspace_id TEXT,
                message TEXT NOT NULL,
                parent_id TEXT,
                is_outgoing INTEGER DEFAULT 0,
                timestamp INTEGER NOT NULL
            )",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_dm_peer ON direct_messages(peer_id, timestamp)",
            [],
        );
    }

    Ok(())
}

impl CyanSystem {
    /// Create system with optional provided secret_key.
    /// If None, generates ephemeral key (for testing - different each launch).
    /// If Some, uses provided key from Swift Keychain (persistent identity).
    async fn new(db_path: String, provided_secret_key: Option<[u8; 32]>) -> Result<Self> {
        let secret_key = match provided_secret_key {
            Some(bytes) => {
                // Use provided key from Swift Keychain - persistent identity
                SecretKey::from_bytes(&bytes)
            }
            None => {
                // Ephemeral key for testing - DIFFERENT EVERY LAUNCH
                let mut rng = ChaCha8Rng::from_os_rng();
                SecretKey::generate(&mut rng)
            }
        };
        let node_id = secret_key.public().to_string();

        println!("🔑 Node ID: {} (persistent={})", &node_id[..16], provided_secret_key.is_some());
        let db_path_clone = db_path.clone();
        let db = Connection::open(db_path)?;
        ensure_schema(&db)?;
        run_migrations(&db)?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<CommandMsg>();
        let (net_tx, net_rx) = mpsc::unbounded_channel::<NetworkCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwiftEvent>();
        // Two separate buffers for different event types
        let event_ffi_buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(Default::default()));
        let integration_event_buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(Default::default()));
        let peers_per_group: Arc<Mutex<HashMap<String, HashSet<PublicKey>>>> = Arc::new(Mutex::new(HashMap::new()));

        let event_ffi_buffer_clone = event_ffi_buffer.clone();
        let integration_event_buffer_clone = integration_event_buffer.clone();
        let node_id_clone = node_id.clone();
        let secret_key_clone = secret_key.clone();
        let peers_per_group_clone = peers_per_group.clone();

        let db_arc = Arc::new(Mutex::new(db));
        let integration_bridge = Arc::new(IntegrationBridge::new(
            db_arc.clone(),
            event_tx.clone(),
        ));

        // Create AI bridge
        let ai_bridge = Arc::new(AIBridge::new(
            db_arc.clone(),
            event_tx.clone(),
        ));
        ai_bridge.set_cyan_db_path(PathBuf::from(db_path_clone)).await;
        ai_bridge.start_insight_generator();

        // Start background task to forward integration events to Swift
        integration_bridge.start_event_forwarder();

        let system = Self {
            node_id: node_id.clone(),
            secret_key: secret_key.clone(),
            event_tx: event_tx.clone(),
            command_tx: cmd_tx,
            network_tx: net_tx.clone(),
            event_ffi_buffer,
            integration_event_buffer,
            db: db_arc,
            peers_per_group,
            integration_bridge,
            ai_bridge,
        };

        let db_clone = system.db.clone();
        let event_tx_clone = event_tx.clone();
        let command_actor_node_id_clone = node_id_clone.clone();
        RUNTIME.get().unwrap().spawn(async move {
            CommandActor {
                db: db_clone,
                rx: cmd_rx,
                network_tx: net_tx,
                event_tx: event_tx_clone,
                node_id: command_actor_node_id_clone.clone(),
            }.run().await;
        });

        let db_clone = system.db.clone();
        let node_id_network_actor_clone = node_id_clone.clone();
        RUNTIME.get().unwrap().spawn(async move {
            match NetworkActor::new(node_id_network_actor_clone, secret_key_clone, db_clone, net_rx, event_tx, peers_per_group_clone).await {
                Ok(n) => n.start().await,
                Err(e) => eprintln!("Network actor failed: {e}"),
            }
        });

        // Event router: routes events to appropriate buffer based on type
        RUNTIME.get().unwrap().spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match serde_json::to_string(&event) {
                    Ok(event_json) => {
                        // Route integration events to their dedicated buffer
                        if event.is_integration_event() {
                            integration_event_buffer_clone.lock().unwrap().push_back(event_json);
                        } else {
                            event_ffi_buffer_clone.lock().unwrap().push_back(event_json);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to serialize event: {e:?}");
                    }
                }
            }
        });

        Ok(system)
    }
}

struct CommandActor {
    db: Arc<Mutex<Connection>>,
    rx: mpsc::UnboundedReceiver<CommandMsg>,
    network_tx: mpsc::UnboundedSender<NetworkCommand>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    node_id: String,
}

impl CommandActor {
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                CommandMsg::CreateGroup { name, icon, color } => {
                    let id = blake3::hash(format!("{}-{}", name, chrono::Utc::now()).as_bytes()).to_hex().to_string();
                    let now = chrono::Utc::now().timestamp();
                    let g = Group {
                        id: id.clone(),
                        name: name.clone(),
                        icon: icon.clone(),
                        color: color.clone(),
                        created_at: now,
                    };

                    {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute(
                            "INSERT INTO groups (id, name, icon, color, created_at) VALUES (?1, \
                             ?2, ?3, ?4, ?5)",
                            params![g.id, g.name, g.icon, g.color, g.created_at],
                        );
                    }

                    let _ = self.network_tx.send(NetworkCommand::JoinGroup {
                        group_id: id.clone(),
                    });
                    let _ = self.network_tx.send(NetworkCommand::Broadcast {
                        group_id: id.clone(),
                        event: NetworkEvent::GroupCreated(g.clone()),
                    });

                    let _ = self.event_tx.send(SwiftEvent::Network(NetworkEvent::GroupCreated(g)));
                }

                CommandMsg::RenameGroup { id, name } => {
                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute("UPDATE groups SET name=?1 WHERE id=?2", params![
                            name.clone(),
                            id.clone()
                        ]).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::Network(NetworkEvent::GroupRenamed {
                            id: id.clone(),
                            name: name.clone(),
                        }));

                        let _ = self.network_tx.send(NetworkCommand::Broadcast {
                            group_id: id.clone(),
                            event: NetworkEvent::GroupRenamed { id, name },
                        });
                    }
                }

                CommandMsg::DeleteGroup { id } => {
                    let ok = {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute("DELETE FROM objects WHERE group_id=?1", params![id]);
                        let _ = db.execute("DELETE FROM workspaces WHERE group_id=?1", params![id]);
                        db.execute("DELETE FROM groups WHERE id=?1", params![id]).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::GroupDeleted { id: id.clone() });
                        let _ = self.network_tx.send(NetworkCommand::DeleteGroup { id });
                    }
                }

                CommandMsg::CreateWorkspace { group_id, name } => {
                    let id = blake3::hash(format!("ws:{}-{}", &group_id, name).as_bytes()).to_hex().to_string();
                    let now = chrono::Utc::now().timestamp();
                    let ws = Workspace {
                        id: id.clone(),
                        group_id: group_id.clone(),
                        name: name.clone(),
                        created_at: now,
                    };

                    {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute(
                            "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) \
                             VALUES (?1, ?2, ?3, ?4)",
                            params![ws.id, ws.group_id, ws.name, ws.created_at],
                        );
                    }

                    let _ = self.network_tx.send(NetworkCommand::JoinGroup {
                        group_id: group_id.clone(),
                    });
                    let _ = self.network_tx.send(NetworkCommand::Broadcast {
                        group_id,
                        event: NetworkEvent::WorkspaceCreated(ws.clone()),
                    });

                    let _ = self.event_tx.send(SwiftEvent::Network(NetworkEvent::WorkspaceCreated(ws)));
                }

                CommandMsg::RenameWorkspace { id, name } => {
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1").unwrap();
                        stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute("UPDATE workspaces SET name=?1 WHERE id=?2", params![
                            name.clone(),
                            id.clone()
                        ]).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::Network(
                            NetworkEvent::WorkspaceRenamed {
                                id: id.clone(),
                                name: name.clone(),
                            },
                        ));

                        if let Some(gid) = group_id {
                            let _ = self.network_tx.send(NetworkCommand::Broadcast {
                                group_id: gid,
                                event: NetworkEvent::WorkspaceRenamed { id, name },
                            });
                        }
                    }
                }

                CommandMsg::DeleteWorkspace { id } => {
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1").unwrap();
                        stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let ok = {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute("DELETE FROM objects WHERE workspace_id=?1", params![id]);
                        db.execute("DELETE FROM workspaces WHERE id=?1", params![id]).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::WorkspaceDeleted { id: id.clone() });

                        if let Some(gid) = group_id {
                            let _ = self.network_tx.send(NetworkCommand::DeleteWorkspace { id });
                        }
                    }
                }

                CommandMsg::CreateBoard { workspace_id, name } => {
                    let id = blake3::hash(format!("wb:{}-{}", &workspace_id, name).as_bytes()).to_hex().to_string();
                    let now = chrono::Utc::now().timestamp();

                    {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute(
                            "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, \
                             created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
                            params![id.clone(), workspace_id.clone(), name.clone(), now],
                        );
                    }

                    // Get group_id for broadcast
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1").unwrap();
                        stmt.query_row(params![workspace_id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let _ = self.event_tx.send(SwiftEvent::Network(NetworkEvent::BoardCreated {
                        id: id.clone(),
                        workspace_id: workspace_id.clone(),
                        name: name.clone(),
                        created_at: now,
                    }));

                    if let Some(gid) = group_id {
                        let _ = self.network_tx.send(NetworkCommand::Broadcast {
                            group_id: gid,
                            event: NetworkEvent::BoardCreated {
                                id: id.clone(),
                                workspace_id,
                                name,
                                created_at: now,
                            },
                        });
                    }
                }

                CommandMsg::RenameBoard { id, name } => {
                    // Get group_id for broadcast
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare(
                            "SELECT w.group_id FROM objects o
                                 JOIN workspaces w ON o.workspace_id = w.id
                                 WHERE o.id=?1 AND o.type='whiteboard' LIMIT 1"
                        ).unwrap();
                        stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute(
                            "UPDATE objects SET name=?1 WHERE id=?2 AND type='whiteboard'",
                            params![name.clone(), id.clone()],
                        ).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::Network(NetworkEvent::BoardRenamed {
                            id: id.clone(),
                            name: name.clone(),
                        }));

                        if let Some(gid) = group_id {
                            let _ = self.network_tx.send(NetworkCommand::Broadcast {
                                group_id: gid,
                                event: NetworkEvent::BoardRenamed { id, name },
                            });
                        }
                    }
                }

                CommandMsg::DeleteBoard { id } => {
                    // Get group_id for broadcast
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare(
                            "SELECT w.group_id FROM objects o
                                 JOIN workspaces w ON o.workspace_id = w.id
                                 WHERE o.id=?1 AND o.type='whiteboard' LIMIT 1"
                        ).unwrap();
                        stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute(
                            "DELETE FROM objects WHERE id=?1 AND type='whiteboard'",
                            params![id],
                        ).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::BoardDeleted { id: id.clone() });

                        if let Some(gid) = group_id {
                            let _ = self.network_tx.send(NetworkCommand::DeleteBoard { id });
                        }
                    }
                }

                // ---- Chat commands ----
                CommandMsg::SendChat { workspace_id, message, parent_id } => {
                    let id = blake3::hash(
                        format!("chat:{}-{}-{}", &workspace_id, &message, chrono::Utc::now()).as_bytes()
                    ).to_hex().to_string();
                    let now = chrono::Utc::now().timestamp();
                    let author = self.node_id.clone();

                    {
                        let db = self.db.lock().unwrap();
                        // Store: type='chat', name=message, hash=author, data=parent_id
                        let _ = db.execute(
                            "INSERT INTO objects (id, workspace_id, type, name, hash, data, created_at) \
                             VALUES (?1, ?2, 'chat', ?3, ?4, ?5, ?6)",
                            params![
                                id.clone(),
                                workspace_id.clone(),
                                message.clone(),
                                author.clone(),
                                parent_id.as_ref().map(|s| s.as_bytes()),
                                now
                            ],
                        );
                    }

                    // Get group_id for broadcast
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1").unwrap();
                        stmt.query_row(params![workspace_id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let evt = NetworkEvent::ChatSent {
                        id: id.clone(),
                        workspace_id: workspace_id.clone(),
                        message: message.clone(),
                        author: author.clone(),
                        parent_id: parent_id.clone(),
                        timestamp: now,
                    };

                    let _ = self.event_tx.send(SwiftEvent::Network(evt.clone()));

                    if let Some(gid) = group_id {
                        let _ = self.network_tx.send(NetworkCommand::Broadcast {
                            group_id: gid,
                            event: evt,
                        });
                    }
                }

                CommandMsg::DeleteChat { id } => {
                    // Get group_id for broadcast
                    let group_id: Option<String> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare(
                            "SELECT w.group_id FROM objects o
                                 JOIN workspaces w ON o.workspace_id = w.id
                                 WHERE o.id=?1 AND o.type='chat' LIMIT 1"
                        ).unwrap();
                        stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                    };

                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute(
                            "DELETE FROM objects WHERE id=?1 AND type='chat'",
                            params![id],
                        ).unwrap_or(0) > 0
                    };

                    if ok {
                        let _ = self.event_tx.send(SwiftEvent::ChatDeleted { id: id.clone() });

                        if let Some(gid) = group_id {
                            let _ = self.network_tx.send(NetworkCommand::DeleteChat { id });
                        }
                    }
                }

                CommandMsg::LoadChatHistory { workspace_id } => {
                    tracing::info!("Loading chat history for workspace: {workspace_id}");

                    let messages: Vec<(String, String, String, String, Option<Vec<u8>>, i64)> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare(
                            "SELECT id, workspace_id, name, hash, data, created_at
                             FROM objects
                             WHERE workspace_id = ?1 AND type = 'chat'
                             ORDER BY created_at ASC"
                        ).unwrap();

                        stmt.query_map(params![workspace_id], |row| {
                            Ok((
                                row.get::<_, String>(0)?,  // id
                                row.get::<_, String>(1)?,  // workspace_id
                                row.get::<_, String>(2)?,  // message (stored in name)
                                row.get::<_, String>(3)?,  // author (stored in hash)
                                row.get::<_, Option<Vec<u8>>>(4)?,  // parent_id (stored in data)
                                row.get::<_, i64>(5)?,     // created_at
                            ))
                        }).unwrap().filter_map(|r| r.ok()).collect()
                    };

                    tracing::info!("Found {} chat messages for workspace {}", messages.len(), workspace_id);

                    for (id, ws_id, message, author, parent_bytes, timestamp) in messages {
                        let parent_id = parent_bytes.and_then(|b| String::from_utf8(b).ok());

                        let _ = self.event_tx.send(SwiftEvent::Network(NetworkEvent::ChatSent {
                            id,
                            workspace_id: ws_id,
                            message,
                            author,
                            parent_id,
                            timestamp,
                        }));
                    }
                }

                CommandMsg::StartDirectChat { peer_id, workspace_id } => {
                    tracing::info!("Starting direct chat with peer: {peer_id}");
                    let _ = self.network_tx.send(NetworkCommand::StartChatStream {
                        peer_id: peer_id.clone(),
                        workspace_id: workspace_id.clone(),
                    });
                }

                CommandMsg::SendDirectMessage { peer_id, workspace_id, message, parent_id } => {
                    let id = blake3::hash(
                        format!("dm:{}-{}-{}-{}", &peer_id, &workspace_id, &message, chrono::Utc::now()).as_bytes()
                    ).to_hex().to_string();
                    let now = chrono::Utc::now().timestamp();

                    // Store in local DB
                    {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute(
                            "INSERT INTO direct_messages (id, peer_id, workspace_id, message, parent_id, is_outgoing, timestamp) \
                             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                            params![id, peer_id, workspace_id, message, parent_id, now],
                        );
                    }

                    // Forward to network layer
                    let _ = self.network_tx.send(NetworkCommand::SendDirectChat {
                        peer_id: peer_id.clone(),
                        workspace_id: workspace_id.clone(),
                        message: message.clone(),
                        parent_id: parent_id.clone(),
                    });

                    // Emit event for Swift UI
                    let _ = self.event_tx.send(SwiftEvent::DirectMessageReceived {
                        id,
                        peer_id,
                        message,
                        timestamp: now,
                        is_incoming: false,
                    });
                }

                CommandMsg::LoadDirectMessageHistory { peer_id } => {
                    tracing::info!("Loading DM history with peer: {peer_id}");

                    let messages: Vec<(String, String, String, Option<String>, i32, i64)> = {
                        let db = self.db.lock().unwrap();
                        let mut stmt = db.prepare(
                            "SELECT id, peer_id, message, parent_id, is_outgoing, timestamp \
                             FROM direct_messages WHERE peer_id = ?1 ORDER BY timestamp ASC LIMIT 100"
                        ).unwrap();

                        stmt.query_map(params![peer_id], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, i32>(4)?,
                                row.get::<_, i64>(5)?,
                            ))
                        }).unwrap().filter_map(|r| r.ok()).collect()
                    };

                    tracing::info!("Found {} DM messages with peer {}", messages.len(), peer_id);

                    for (id, peer, msg, _parent, is_outgoing, timestamp) in messages {
                        let _ = self.event_tx.send(SwiftEvent::DirectMessageReceived {
                            id,
                            peer_id: peer,
                            message: msg,
                            timestamp,
                            is_incoming: is_outgoing == 0,
                        });
                    }
                }

                CommandMsg::Snapshot {} => {
                    let json = dump_tree_json(&self.db);
                    let _ = self.event_tx.send(SwiftEvent::TreeLoaded(json.clone()));
                }

                CommandMsg::SeedDemoIfEmpty => {
                    seed_demo_if_empty(&self.db);
                    let json = dump_tree_json(&self.db);
                    let _ = self.event_tx.send(SwiftEvent::TreeLoaded(json));
                }
            }
        }
    }
}

// ---------- Direct QUIC Chat Helpers ----------

/// Write a length-prefixed message to a QUIC send stream
async fn write_chat_frame(tx: &mut iroh::endpoint::SendStream, msg: &[u8]) -> Result<()> {
    let len = (msg.len() as u32).to_be_bytes();
    tx.write_all(&len).await?;
    tx.write_all(msg).await?;
    Ok(())
}

/// Read a length-prefixed message from a QUIC receive stream
async fn read_chat_frame(rx: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    rx.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    // Sanity check - don't allocate huge buffers
    if len > 1024 * 1024 {
        return Err(anyhow!("Chat message too large: {} bytes", len));
    }

    let mut buf = vec![0u8; len];
    rx.read_exact(&mut buf).await?;
    Ok(buf)
}

/// First frame sent on chat stream to establish context
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatStreamInit {
    workspace_id: String,
}

/// Message format for direct chat
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirectChatMessage {
    message: String,
    parent_id: Option<String>,
}

/// Handle a direct chat stream - runs as spawned task
/// Reads from QUIC stream and writes to DB, reads from channel and writes to QUIC stream
async fn handle_chat_stream(
    db: Arc<Mutex<Connection>>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    node_id: String,
    peer_id: String,
    workspace_id: String,
    mut tx: iroh::endpoint::SendStream,
    mut rx: iroh::endpoint::RecvStream,
    mut outbound_rx: mpsc::UnboundedReceiver<DirectChatOutbound>,
) {
    loop {
        tokio::select! {
            // Incoming message from peer
            read_result = read_chat_frame(&mut rx) => {
                match read_result {
                    Ok(buf) => {
                        match serde_json::from_slice::<DirectChatMessage>(&buf) {
                            Ok(chat_msg) => {
                                let id = blake3::hash(
                                    format!("chat:{}-{}-{}", &peer_id, &chat_msg.message, chrono::Utc::now()).as_bytes()
                                ).to_hex().to_string();
                                let now = chrono::Utc::now().timestamp();

                                // Write to DB
                                {
                                    let db = db.lock().unwrap();
                                    let _ = db.execute(
                                        "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, hash, data, created_at) \
                                         VALUES (?1, ?2, 'chat', ?3, ?4, ?5, ?6)",
                                        params![
                                            id.clone(),
                                            workspace_id.clone(),
                                            chat_msg.message.clone(),
                                            peer_id.clone(),
                                            chat_msg.parent_id.as_ref().map(|s| s.as_bytes()),
                                            now
                                        ],
                                    );
                                }

                                // Also store in direct_messages table
                                {
                                    let db = db.lock().unwrap();
                                    let _ = db.execute(
                                        "INSERT OR IGNORE INTO direct_messages (id, peer_id, workspace_id, message, parent_id, is_outgoing, timestamp) \
                                         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                                        params![
                                            id.clone(),
                                            peer_id.clone(),
                                            workspace_id.clone(),
                                            chat_msg.message.clone(),
                                            chat_msg.parent_id.clone(),
                                            now
                                        ],
                                    );
                                }

                                // Notify Swift via ChatSent (for backward compat)
                                let _ = event_tx.send(SwiftEvent::Network(NetworkEvent::ChatSent {
                                    id: id.clone(),
                                    workspace_id: workspace_id.clone(),
                                    message: chat_msg.message.clone(),
                                    author: peer_id.clone(),
                                    parent_id: chat_msg.parent_id.clone(),
                                    timestamp: now,
                                }));

                                // Also emit DirectMessageReceived for DM-specific handling
                                let _ = event_tx.send(SwiftEvent::DirectMessageReceived {
                                    id,
                                    peer_id: peer_id.clone(),
                                    message: chat_msg.message,
                                    timestamp: now,
                                    is_incoming: true,
                                });
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse chat message: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::info!("Chat stream closed with peer {peer_id}: {e}");
                        break;
                    }
                }
            }

            // Outgoing message to peer
            outbound = outbound_rx.recv() => {
                match outbound {
                    Some(msg) => {
                        let chat_msg = DirectChatMessage {
                            message: msg.message.clone(),
                            parent_id: msg.parent_id.clone(),
                        };

                        match serde_json::to_vec(&chat_msg) {
                            Ok(bytes) => {
                                if let Err(e) = write_chat_frame(&mut tx, &bytes).await {
                                    tracing::error!("Failed to send chat to {peer_id}: {e}");
                                    break;
                                }

                                // Also store locally and notify Swift
                                let id = blake3::hash(
                                    format!("chat:{}-{}-{}", &node_id, &msg.message, chrono::Utc::now()).as_bytes()
                                ).to_hex().to_string();
                                let now = chrono::Utc::now().timestamp();

                                {
                                    let db = db.lock().unwrap();
                                    let _ = db.execute(
                                        "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, hash, data, created_at) \
                                         VALUES (?1, ?2, 'chat', ?3, ?4, ?5, ?6)",
                                        params![
                                            id.clone(),
                                            msg.workspace_id.clone(),
                                            msg.message.clone(),
                                            node_id.clone(),
                                            msg.parent_id.as_ref().map(|s| s.as_bytes()),
                                            now
                                        ],
                                    );
                                }

                                let _ = event_tx.send(SwiftEvent::Network(NetworkEvent::ChatSent {
                                    id,
                                    workspace_id: msg.workspace_id,
                                    message: msg.message,
                                    author: node_id.clone(),
                                    parent_id: msg.parent_id,
                                    timestamp: now,
                                }));
                            }
                            Err(e) => {
                                tracing::error!("Failed to serialize chat message: {e}");
                            }
                        }
                    }
                    None => {
                        tracing::info!("Outbound channel closed for peer {peer_id}");
                        break;
                    }
                }
            }
        }
    }

    tracing::info!("Chat stream handler exiting for peer {peer_id}");
}

struct NetworkActor {
    node_id: String,
    db: Arc<Mutex<Connection>>,
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<NetworkCommand>>>,
    endpoint: Endpoint,
    gossip: Arc<Gossip>,
    blob_store: BlobStore,
    topics: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<GossipTopic>>>>>,
    peers_per_group: Arc<Mutex<HashMap<String, HashSet<PublicKey>>>>,
    groups: Arc<Mutex<HashSet<String>>>,
    event_tx: mpsc::UnboundedSender<SwiftEvent>,
    discovery_topic: Arc<tokio::sync::Mutex<GossipTopic>>,
    need_snapshot: Arc<Mutex<HashMap<String, AtomicBool>>>,
    /// Channel senders for each active direct chat stream, keyed by peer_id
    chat_senders: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<DirectChatOutbound>>>>,
}

impl NetworkActor {
    async fn new(
        node_id: String,
        secret_key: SecretKey,
        db: Arc<Mutex<Connection>>,
        rx: mpsc::UnboundedReceiver<NetworkCommand>,
        event_tx: mpsc::UnboundedSender<SwiftEvent>,
        peers_per_group: Arc<Mutex<HashMap<String, HashSet<PublicKey>>>>,
    ) -> Result<Self> {
        let builder = Endpoint::builder().secret_key(secret_key.clone()).alpns(vec![b"xsp-1.0".to_vec()]).relay_mode(RelayMode::Default);
        let endpoint = builder.bind().await?;
        tracing::info!("Node ID: {}", endpoint.id());
        let gossip = Arc::new(Gossip::builder().spawn(endpoint.clone()));

        let root = DATA_DIR.get().cloned().unwrap_or_else(|| PathBuf::from(".")).join("blobs");
        let blob_store = BlobStore::load(root).await?;

        // Subscribe to discovery topic
        let discovery_topic_id = TopicId::from_bytes(
            blake3::hash(
                format!(
                    "cyan/discovery/{}",
                    DISCOVERY_KEY.get().unwrap_or(&"cyan-dev".to_string())
                ).as_bytes(),
            ).as_bytes()[..32].try_into()?,
        );
        let discovery_topic = gossip.subscribe_and_join(discovery_topic_id, vec![]).await?;
        Ok(Self {
            node_id,
            db,
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            endpoint,
            gossip,
            blob_store,
            topics: Arc::new(Mutex::new(HashMap::new())),
            peers_per_group,
            groups: Arc::new(Mutex::new(HashSet::new())),
            event_tx,
            discovery_topic: Arc::new(tokio::sync::Mutex::new(discovery_topic)),
            need_snapshot: Arc::new(Mutex::new(HashMap::new())),
            chat_senders: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn start(self) {
        let self_arc = Arc::new(self);
        // Setup discovery
        self_arc.clone().setup_discovery().await;
        tracing::info!("Start a snapshot requestor for groups == handles sending snapshot requests");
        tokio::spawn(self_arc.clone().snapshot_requestor());
        // Start polling network events
        tokio::spawn(self_arc.clone().poll_network_events());
        // Run command handler
        self_arc.clone().poll_network_commands().await;
    }

    pub async fn snapshot_requestor(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            let requests: Vec<String> = {
                let snapshot_requests = self.need_snapshot.lock().unwrap();
                snapshot_requests.iter().filter_map(|(group_id, flag)| {
                    if flag.load(Ordering::Relaxed) {
                        Some(group_id.clone())
                    } else {
                        None
                    }
                }).collect()
            };

            for group_id in requests {
                let topic = {
                    let topics = self.topics.lock().unwrap();
                    topics.get(&group_id).cloned()
                };

                if let Some(topic) = topic {
                    let request_msg = NetworkCommand::RequestSnapshot {
                        from_peer: self.node_id.clone()
                    };

                    // Emit status update
                    let _ = self.event_tx.send(SwiftEvent::StatusUpdate {
                        message: format!("Requesting sync for group..."),
                    });

                    let mut guard = topic.lock().await;
                    let _ = guard.broadcast(
                        Bytes::from(serde_json::to_vec(&request_msg).unwrap())
                    ).await;
                }
            }
        }
    }
    fn get_groups_from_db(db: Arc<Mutex<Connection>>) -> HashSet<String> {
        let groups = {
            let db = db.lock().unwrap();
            let mut s = db.prepare("SELECT id FROM groups").unwrap();
            let mut rows = s.query([]).unwrap();
            let mut out = vec![];
            while let Some(r) = rows.next().unwrap() {
                out.push(r.get::<_, String>(0).unwrap());
            }
            out
        };
        HashSet::from_iter(groups)
    }

    async fn poll_network_events(self: Arc<Self>) {
        loop {
            let topics = self.clone().topics.lock().unwrap().clone();
            for (group_id, topic) in topics.iter() {
                let topic = topic.clone();
                let db = self.db.clone();
                let event_tx = self.event_tx.clone();
                let self_clone = self.clone();
                let group_id_for_cmd = group_id.clone();

                tokio::spawn(async move {
                    if let Ok(mut topic_guard) = topic.try_lock() {
                        if let Some(Ok(ev)) = topic_guard.next().await {
                            match ev {
                                GossipEvent::NeighborUp(peer_id) => {
                                    let peer_str = peer_id.to_string();
                                    tracing::info!("Peer joined group {}: {}", group_id_for_cmd, peer_str);

                                    // Add to peers_per_group
                                    {
                                        let mut peers = self_clone.peers_per_group.lock().unwrap();
                                        peers.entry(group_id_for_cmd.clone())
                                            .or_insert_with(HashSet::new)
                                            .insert(peer_id);
                                    }

                                    // Emit PeerJoined event
                                    let _ = event_tx.send(SwiftEvent::PeerJoined {
                                        group_id: group_id_for_cmd.clone(),
                                        peer_id: peer_str,
                                    });
                                }
                                GossipEvent::NeighborDown(peer_id) => {
                                    let peer_str = peer_id.to_string();
                                    tracing::info!("Peer left group {}: {}", group_id_for_cmd, peer_str);

                                    // Remove from peers_per_group
                                    {
                                        let mut peers = self_clone.peers_per_group.lock().unwrap();
                                        if let Some(group_peers) = peers.get_mut(&group_id_for_cmd) {
                                            group_peers.remove(&peer_id);
                                        }
                                    }

                                    // Emit PeerLeft event
                                    let _ = event_tx.send(SwiftEvent::PeerLeft {
                                        group_id: group_id_for_cmd.clone(),
                                        peer_id: peer_str,
                                    });
                                }
                                GossipEvent::Received(msg) => {
                                    // Try parsing as NetworkEvent first
                                    if let Ok(evt) = serde_json::from_slice::<NetworkEvent>(&msg.content) {
                                        match &evt {
                                            NetworkEvent::GroupCreated(g) => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO groups (id, name, icon, \
                                             color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                                                    params![
                                                g.id,
                                                g.name,
                                                g.icon,
                                                g.color,
                                                g.created_at
                                            ],
                                                );
                                            }
                                            NetworkEvent::GroupRenamed { id, name } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "UPDATE groups SET name=?1 WHERE id=?2",
                                                    params![name, id],
                                                );
                                            }
                                            NetworkEvent::WorkspaceCreated(ws) => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO workspaces (id, group_id, \
                                             name, created_at) VALUES (?1, ?2, ?3, ?4)",
                                                    params![ws.id, ws.group_id, ws.name, ws.created_at],
                                                );
                                            }
                                            NetworkEvent::WorkspaceRenamed { id, name } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "UPDATE workspaces SET name=?1 WHERE id=?2",
                                                    params![name, id],
                                                );
                                            }
                                            NetworkEvent::WorkspaceDeleted { id } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "DELETE FROM objects WHERE workspace_id=?1",
                                                    params![id],
                                                );
                                                let _ = db.execute(
                                                    "DELETE FROM workspaces WHERE id=?1",
                                                    params![id],
                                                );
                                            }
                                            NetworkEvent::BoardCreated {
                                                id,
                                                workspace_id,
                                                name,
                                                created_at,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO objects (id, workspace_id, \
                                             type, name, created_at) VALUES (?1, ?2, \
                                             'whiteboard', ?3, ?4)",
                                                    params![id, workspace_id, name, created_at],
                                                );
                                            }
                                            NetworkEvent::BoardRenamed { id, name } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "UPDATE objects SET name=?1 WHERE id=?2 AND \
                                             type='whiteboard'",
                                                    params![name, id],
                                                );
                                            }
                                            NetworkEvent::BoardDeleted { id } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "DELETE FROM objects WHERE id=?1 AND type='whiteboard'",
                                                    params![id],
                                                );
                                            }
                                            NetworkEvent::FileAvailable {
                                                id,
                                                group_id,
                                                workspace_id,
                                                board_id,
                                                name,
                                                hash,
                                                size,
                                                source_peer,
                                                created_at,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO objects (id, group_id, workspace_id, board_id, type, name, hash, size, source_peer, created_at)
                                                     VALUES (?1, ?2, ?3, ?4, 'file', ?5, ?6, ?7, ?8, ?9)",
                                                    params![
                                                        id,
                                                        group_id,
                                                        workspace_id,
                                                        board_id,
                                                        name,
                                                        hash,
                                                        *size as i64,
                                                        source_peer,
                                                        created_at
                                                    ],
                                                );
                                            }
                                            NetworkEvent::GroupDeleted { id } => {
                                                let ok = {
                                                    let db = db.lock().unwrap();
                                                    let _ = db.execute(
                                                        "DELETE FROM objects WHERE group_id=?1",
                                                        params![id],
                                                    );
                                                    let _ = db.execute(
                                                        "DELETE FROM workspaces WHERE group_id=?1",
                                                        params![id],
                                                    );
                                                    db.execute(
                                                        "DELETE FROM groups WHERE id=?1",
                                                        params![id],
                                                    ).unwrap_or(0) > 0
                                                };
                                                if ok {
                                                    self_clone.topics.lock().unwrap().remove(id);
                                                    self_clone.groups.lock().unwrap().remove(id);
                                                    let _ = self_clone.event_tx.send(
                                                        SwiftEvent::GroupDeleted { id: id.clone() }
                                                    );
                                                }
                                            }
                                            NetworkEvent::GroupSnapshotAvailable { source, group_id } => {
                                                // Don't download from ourselves
                                                if *source != self_clone.node_id {
                                                    tracing::info!("Snapshot available for {group_id:?} from {source:?}");
                                                    // check if we need it?
                                                    let needs_snapshot = self_clone.need_snapshot.lock().unwrap()
                                                        .get(group_id)
                                                        .map(|flag| flag.load(Ordering::Relaxed))
                                                        .unwrap_or(false);

                                                    if needs_snapshot {
                                                        tracing::info!("Downloading snapshot for {group_id:?} from {source:?}");
                                                        if let Ok(pk) = PublicKey::from_str(&source) {
                                                            let self_for_spawn = self_clone.clone();
                                                            let source_clone = source.clone();
                                                            let group_id_clone = group_id.clone();
                                                            tokio::spawn(async move {
                                                                if let Err(e) = self_for_spawn.download_snapshot(source_clone, group_id_clone).await {
                                                                    tracing::error!("Snapshot download failed: {}", e);
                                                                }
                                                            });
                                                        } else {
                                                            tracing::warn!("Snapshot available from {source:?} cannot be downloaded because of invalid public key!")
                                                        }
                                                    }
                                                }
                                            }
                                            // ---- Chat events ----
                                            NetworkEvent::ChatSent {
                                                id,
                                                workspace_id,
                                                message,
                                                author,
                                                parent_id,
                                                timestamp,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO objects (id, workspace_id, \
                                                 type, name, hash, data, created_at) \
                                                 VALUES (?1, ?2, 'chat', ?3, ?4, ?5, ?6)",
                                                    params![
                                                    id,
                                                    workspace_id,
                                                    message,
                                                    author,
                                                    parent_id.as_ref().map(|s| s.as_bytes()),
                                                    timestamp
                                                ],
                                                );
                                            }
                                            NetworkEvent::ChatDeleted { id } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "DELETE FROM objects WHERE id=?1 AND type='chat'",
                                                    params![id],
                                                );
                                            }
                                            // ---- Whiteboard element events ----
                                            NetworkEvent::WhiteboardElementAdded {
                                                id,
                                                board_id,
                                                element_type,
                                                x, y, width, height, z_index,
                                                style_json,
                                                content_json,
                                                created_at,
                                                updated_at,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO whiteboard_elements
                                                     (id, board_id, element_type, x, y, width, height, z_index,
                                                      style_json, content_json, created_at, updated_at)
                                                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                                                    params![
                                                        id, board_id, element_type,
                                                        x, y, width, height, z_index,
                                                        style_json, content_json,
                                                        created_at, updated_at
                                                    ],
                                                );
                                            }
                                            NetworkEvent::WhiteboardElementUpdated {
                                                id,
                                                board_id,
                                                element_type,
                                                x, y, width, height, z_index,
                                                style_json,
                                                content_json,
                                                updated_at,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "UPDATE whiteboard_elements SET
                                                     board_id=?2, element_type=?3, x=?4, y=?5,
                                                     width=?6, height=?7, z_index=?8,
                                                     style_json=?9, content_json=?10, updated_at=?11
                                                     WHERE id=?1",
                                                    params![
                                                        id, board_id, element_type,
                                                        x, y, width, height, z_index,
                                                        style_json, content_json, updated_at
                                                    ],
                                                );
                                            }
                                            NetworkEvent::WhiteboardElementDeleted { id, board_id: _ } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "DELETE FROM whiteboard_elements WHERE id=?1",
                                                    params![id],
                                                );
                                            }
                                            NetworkEvent::WhiteboardCleared { board_id } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "DELETE FROM whiteboard_elements WHERE board_id=?1",
                                                    params![board_id],
                                                );
                                            }
                                            // ---- Notebook cell events ----
                                            NetworkEvent::NotebookCellAdded {
                                                id,
                                                board_id,
                                                cell_type,
                                                cell_order,
                                                content,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let now = std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .map(|d| d.as_secs() as i64)
                                                    .unwrap_or(0);
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO notebook_cells
                                                     (id, board_id, cell_type, cell_order, content, created_at, updated_at)
                                                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                                                    params![id, board_id, cell_type, cell_order, content, now, now],
                                                );
                                            }
                                            NetworkEvent::NotebookCellUpdated {
                                                id,
                                                board_id: _,
                                                cell_type,
                                                cell_order,
                                                content,
                                                output,
                                                collapsed,
                                                height,
                                                metadata_json,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let now = std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .map(|d| d.as_secs() as i64)
                                                    .unwrap_or(0);
                                                let _ = db.execute(
                                                    "UPDATE notebook_cells SET
                                                     cell_type=?2, cell_order=?3, content=?4, output=?5,
                                                     collapsed=?6, height=?7, metadata_json=?8, updated_at=?9
                                                     WHERE id=?1",
                                                    params![
                                                        id, cell_type, cell_order, content, output,
                                                        *collapsed as i32, height, metadata_json, now
                                                    ],
                                                );
                                            }
                                            NetworkEvent::NotebookCellDeleted { id, board_id: _ } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "DELETE FROM notebook_cells WHERE id=?1",
                                                    params![id],
                                                );
                                            }
                                            NetworkEvent::NotebookCellsReordered { board_id, cell_ids } => {
                                                let db = db.lock().unwrap();
                                                let now = std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .map(|d| d.as_secs() as i64)
                                                    .unwrap_or(0);
                                                for (idx, cell_id) in cell_ids.iter().enumerate() {
                                                    let _ = db.execute(
                                                        "UPDATE notebook_cells SET cell_order=?1, updated_at=?2
                                                         WHERE id=?3 AND board_id=?4",
                                                        params![idx as i32, now, cell_id, board_id],
                                                    );
                                                }
                                            }
                                            NetworkEvent::BoardModeChanged { board_id, mode } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "UPDATE objects SET board_mode=?1 WHERE id=?2",
                                                    params![mode, board_id],
                                                );
                                            }
                                            // ---- Board metadata events (P2P sync) ----
                                            NetworkEvent::BoardMetadataUpdated {
                                                board_id,
                                                labels,
                                                rating,
                                                contains_model,
                                                contains_skills,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".to_string());
                                                let skills_json = serde_json::to_string(&contains_skills).unwrap_or_else(|_| "[]".to_string());
                                                let _ = db.execute(
                                                    "INSERT INTO board_metadata (board_id, labels, rating, contains_model, contains_skills) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(board_id) DO UPDATE SET labels = ?2, rating = ?3, contains_model = ?4, contains_skills = ?5",
                                                    params![board_id, labels_json, rating, contains_model, skills_json],
                                                );
                                            }
                                            NetworkEvent::BoardLabelsUpdated { board_id, labels } => {
                                                let db = db.lock().unwrap();
                                                let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".to_string());
                                                let _ = db.execute(
                                                    "INSERT INTO board_metadata (board_id, labels) VALUES (?1, ?2) ON CONFLICT(board_id) DO UPDATE SET labels = ?2",
                                                    params![board_id, labels_json],
                                                );
                                            }
                                            NetworkEvent::BoardRated { board_id, rating } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT INTO board_metadata (board_id, rating) VALUES (?1, ?2) ON CONFLICT(board_id) DO UPDATE SET rating = ?2",
                                                    params![board_id, rating],
                                                );
                                            }
                                            NetworkEvent::ProfileUpdated { node_id, display_name, avatar_hash } => {
                                                // Store/update the peer's profile
                                                let now = chrono::Utc::now().timestamp();
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT INTO user_profiles (node_id, display_name, avatar_hash, status, updated_at)
                                                     VALUES (?1, ?2, ?3, 'online', ?4)
                                                     ON CONFLICT(node_id) DO UPDATE SET
                                                        display_name = excluded.display_name,
                                                        avatar_hash = COALESCE(excluded.avatar_hash, user_profiles.avatar_hash),
                                                        status = 'online',
                                                        updated_at = excluded.updated_at",
                                                    params![node_id, display_name, avatar_hash, now],
                                                );
                                            }
                                        }

                                        // Send to Swift

                                        // Send to Swift
                                        let _ = event_tx.send(SwiftEvent::Network(evt));
                                    }
                                    // Try parsing as NetworkCommand (for snapshot requests)
                                    else if let Ok(cmd) = serde_json::from_slice::<NetworkCommand>(&msg.content) {
                                        match cmd {
                                            NetworkCommand::RequestSnapshot { from_peer } => {
                                                // Don't respond to our own requests
                                                if from_peer != self_clone.node_id {
                                                    tracing::info!("Received snapshot request from {from_peer} for group {group_id_for_cmd}");

                                                    if let Ok(pk) = PublicKey::from_str(&from_peer) {
                                                        let self_for_spawn = self_clone.clone();
                                                        let db_clone = db.clone();
                                                        let gid = group_id_for_cmd.clone();
                                                        tokio::spawn(async move {
                                                            if let Err(e) = self_for_spawn.send_snapshot(pk, gid, db_clone).await {
                                                                tracing::error!("Failed to send snapshot: {}", e);
                                                            }
                                                        });
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                } // end GossipEvent::Received
                                _ => {} // Other gossip events
                            } // end match ev
                        }
                    }
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn download_snapshot(self: Arc<Self>, peer_address: String, group_id: String) -> Result<()> {
        // Emit status update
        let _ = self.event_tx.send(SwiftEvent::StatusUpdate {
            message: format!("Downloading snapshot from peer..."),
        });

        let end_pt_address = EndpointAddr::new(PublicKey::from_str(&peer_address)?);
        let connection = self.endpoint.connect(end_pt_address, b"xsp-1.0").await?;
        let (_send, mut recv) = connection.open_bi().await?;

        // FIXME: Note open a different stream for receiving files to afford some parallelization
        // to get snapshot quick
        // TODO: add zipping to get snapshot and save on bandwidth.

        tracing::info!("Reading snapshot data from {peer_address}...");
        let buf = recv.read_to_end(10 * 1024 * 1024).await?; // 10MB max

        tracing::info!("Downloaded {} bytes, deserializing...", buf.len());
        let tree_dto: TreeSnapshotDTO = serde_json::from_slice(&buf)?;

        tracing::info!("Storing tree snapshot for group: {group_id}");
        let db = self.db.lock().map_err(|e| anyhow!("Failed to lock db: {}", e))?;

        let group = tree_dto.groups;
        match group.get(0) {
            None => {
                tracing::warn!("No group found in snapshot, skipping");
                Err(anyhow!("No group in snapshot"))
            }
            Some(g) => {
                // Emit status update
                let _ = self.event_tx.send(SwiftEvent::StatusUpdate {
                    message: format!("Syncing group '{}'...", g.name),
                });

                tracing::info!("Received snapshot for {g:?}, inserting data");
                let _ = db.execute(
                    "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![g.id, g.name, g.icon, g.color, g.created_at],
                );
                tracing::debug!("Group successfully inserted");

                for w in tree_dto.workspaces {
                    let _ = db.execute(
                        "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![w.id, w.group_id, w.name, w.created_at],
                    );
                    tracing::debug!("Workspace {w:?} successfully inserted");
                }

                for whiteboard in tree_dto.whiteboards {
                    let _ = db.execute(
                        "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
                        params![whiteboard.id, whiteboard.workspace_id, whiteboard.name, whiteboard.created_at],
                    );
                    tracing::debug!("Whiteboard {whiteboard:?} successfully inserted");
                }

                // Restore chats from snapshot
                for chat in tree_dto.chats {
                    let _ = db.execute(
                        "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, hash, data, created_at) \
                         VALUES (?1, ?2, 'chat', ?3, ?4, ?5, ?6)",
                        params![
                            chat.id,
                            chat.workspace_id,
                            chat.message,
                            chat.author,
                            chat.parent_id.as_ref().map(|s| s.as_bytes()),
                            chat.timestamp
                        ],
                    );
                    tracing::debug!("Chat {chat:?} successfully inserted");
                }
                // Restore board metadata from snapshot
                for meta in tree_dto.board_metadata {
                    let labels_json = serde_json::to_string(&meta.labels).unwrap_or_else(|_| "[]".to_string());
                    let skills_json = serde_json::to_string(&meta.contains_skills).unwrap_or_else(|_| "[]".to_string());
                    let _ = db.execute(
                        "INSERT OR REPLACE INTO board_metadata (board_id, labels, rating, view_count, contains_model, contains_skills, board_type, last_accessed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            meta.board_id, labels_json, meta.rating, meta.view_count,
                            meta.contains_model, skills_json, meta.board_type, meta.last_accessed
                        ],
                    );
                }
                // Mark snapshot as received
                if let Some(flag) = self.need_snapshot.lock().unwrap().get(&group_id) {
                    flag.store(false, Ordering::Relaxed);
                    tracing::info!("Snapshot for {group_id} marked as received");
                }

                // Emit sync complete status
                let _ = self.event_tx.send(SwiftEvent::StatusUpdate {
                    message: format!("Sync complete"),
                });

                // Notify Swift of tree update
                let json = dump_tree_json(&self.db);
                let _ = self.event_tx.send(SwiftEvent::TreeLoaded(json));

                Ok(())
            }
        }
    }

    async fn send_snapshot(
        self: Arc<Self>,
        requester: PublicKey,
        group_id: String,
        db: Arc<Mutex<Connection>>,
    ) -> Result<()> {
        tracing::info!("Building snapshot for group {group_id} to send to {requester}");

        // Build snapshot from DB
        let snapshot = build_group_snapshot(&db, &group_id)?;

        // Serialize
        let data = serde_json::to_vec(&snapshot)?;
        tracing::info!("Snapshot serialized: {} bytes", data.len());

        // Connect to requester
        let addr = EndpointAddr::new(requester);
        let connection = self.endpoint.connect(addr, b"xsp-1.0").await?;
        let (mut send, _recv) = connection.open_bi().await?;

        // Send data
        send.write_all(&data).await?;
        send.finish()?;

        tracing::info!("Snapshot sent successfully to {requester}");

        // Broadcast that snapshot is available
        let evt = NetworkEvent::GroupSnapshotAvailable {
            source: self.node_id.clone(),
            group_id: group_id.clone(),
        };

        if let Some(topic) = self.topics.lock().unwrap().get(&group_id).cloned() {
            tokio::spawn(async move {
                let mut guard = topic.lock().await;
                let _ = guard.broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap())).await;
            });
        }

        Ok(())
    }


    async fn poll_network_commands(self: Arc<Self>) {
        loop {
            let mut rx_guard = self.rx.lock().await;

            tokio::select! {
                // Accept incoming QUIC connections for direct chat
                incoming = self.endpoint.accept() => {
                    if let Some(incoming) = incoming {
                        match incoming.await {
                            Ok(connection) => {
                                let peer_id = connection.remote_id().to_string();
                                tracing::info!("Accepted direct connection from peer: {peer_id}");

                                let db = self.db.clone();
                                let event_tx = self.event_tx.clone();
                                let node_id = self.node_id.clone();
                                let chat_senders = self.chat_senders.clone();
                                let peer_id_clone = peer_id.clone();

                                // Spawn task to accept streams on this connection
                                tokio::spawn(async move {
                                    // Accept the bi-directional stream
                                    match connection.accept_bi().await {
                                        Ok((tx, mut rx)) => {
                                            // Read workspace_id from first frame
                                            match read_chat_frame(&mut rx).await {
                                                Ok(init_bytes) => {
                                                    match serde_json::from_slice::<ChatStreamInit>(&init_bytes) {
                                                        Ok(init) => {
                                                            tracing::info!("Chat stream initialized for workspace: {}", init.workspace_id);

                                                            // Create channel for outbound messages
                                                            let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

                                                            // Store sender for this peer
                                                            {
                                                                let mut senders = chat_senders.lock().unwrap();
                                                                senders.insert(peer_id_clone.clone(), outbound_tx);
                                                            }

                                                            // Notify Swift that stream is ready
                                                            let _ = event_tx.send(SwiftEvent::ChatStreamReady {
                                                                peer_id: peer_id_clone.clone(),
                                                                workspace_id: init.workspace_id.clone(),
                                                            });

                                                            // Run chat stream handler
                                                            handle_chat_stream(
                                                                db,
                                                                event_tx.clone(),
                                                                node_id,
                                                                peer_id_clone.clone(),
                                                                init.workspace_id,
                                                                tx,
                                                                rx,
                                                                outbound_rx,
                                                            ).await;

                                                            // Clean up sender when done
                                                            {
                                                                let mut senders = chat_senders.lock().unwrap();
                                                                senders.remove(&peer_id_clone);
                                                            }

                                                            // Notify Swift that stream is closed
                                                            let _ = event_tx.send(SwiftEvent::ChatStreamClosed {
                                                                peer_id: peer_id_clone,
                                                            });
                                                        }
                                                        Err(e) => {
                                                            tracing::error!("Failed to parse chat init: {e}");
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::error!("Failed to read chat init frame: {e}");
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to accept bi stream: {e}");
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::error!("Failed to accept connection: {e}");
                            }
                        }
                    }
                }

                // Handle network commands
                cmd = rx_guard.recv() => {
                    if let Some(cmd) = cmd {
                        // Drop the guard before processing to avoid holding lock
                        drop(rx_guard);

                        match cmd {
                            NetworkCommand::JoinGroup { group_id } => {
                                let should_subscribe = {
                                    let topics = self.topics.lock().unwrap();
                                    !topics.contains_key(&group_id)
                                };

                                if should_subscribe {
                                    let topic_id = TopicId::from_bytes(
                                        blake3::hash(format!("cyan/group/{}", group_id).as_bytes()).as_bytes()[..32].try_into().unwrap(),
                                    );

                                    if let Ok(topic) = self.gossip.subscribe_and_join(topic_id, vec![]).await
                                    {
                                        self.topics.lock().unwrap().insert(
                                            group_id.clone(),
                                            Arc::new(tokio::sync::Mutex::new(topic)),
                                        );

                                        self.groups.lock().unwrap().insert(group_id);
                                    }
                                }
                            }

                            NetworkCommand::Broadcast { group_id, event } => {
                                let topic = self.topics.lock().unwrap().get(&group_id).cloned();
                                if let Some(topic) = topic {
                                    let data = serde_json::to_vec(&event).unwrap();
                                    tokio::spawn(async move {
                                        let mut guard = topic.lock().await;
                                        let _ = guard.broadcast(Bytes::from(data)).await;
                                    });
                                }
                            }

                            NetworkCommand::UploadToGroup { group_id, path } => {
                                if let Ok(bytes) = tokio::fs::read(&path).await {
                                    let _ = self.blob_store.add_bytes(bytes.clone()).await;
                                    let id = blake3::hash(format!("file:{}:{}", &group_id, &path).as_bytes()).to_hex().to_string();
                                    let hash = blake3::hash(&bytes).to_hex().to_string();
                                    let size = bytes.len() as u64;
                                    let created_at = chrono::Utc::now().timestamp();

                                    {
                                        let db = self.db.lock().unwrap();
                                        let name = Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("file");
                                        let _ = db.execute(
                                            "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, board_id, type, name, hash, size, source_peer, created_at)
                                             VALUES (?1, ?2, NULL, NULL, 'file', ?3, ?4, ?5, ?6, ?7)",
                                            params![id, group_id, name, hash, size as i64, self.node_id, created_at],
                                        );
                                    }

                                    let topic = self.topics.lock().unwrap().get(&group_id).cloned();
                                    if let Some(topic) = topic {
                                        let evt = NetworkEvent::FileAvailable {
                                            id: id.clone(),
                                            group_id: Some(group_id.clone()),
                                            workspace_id: None,
                                            board_id: None,
                                            name: Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string(),
                                            hash,
                                            size,
                                            source_peer: self.node_id.clone(),
                                            created_at,
                                        };
                                        tokio::spawn(async move {
                                            let mut guard = topic.lock().await;
                                            let _ = guard.broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap())).await;
                                        });
                                    }
                                }
                            }

                            NetworkCommand::UploadToWorkspace { workspace_id, path } => {
                                if let Ok(bytes) = tokio::fs::read(&path).await {
                                    let _ = self.blob_store.add_bytes(bytes.clone()).await;

                                    let gid: Option<String> = {
                                        let db = self.db.lock().unwrap();
                                        let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1").unwrap();
                                        stmt.query_row(params![workspace_id.clone()], |r| {
                                            r.get::<_, String>(0)
                                        }).optional().unwrap()
                                    };

                                    let id = blake3::hash(
                                        format!("file:{}:{}", &workspace_id, &path).as_bytes(),
                                    ).to_hex().to_string();
                                    let hash = blake3::hash(&bytes).to_hex().to_string();
                                    let size = bytes.len() as u64;
                                    let created_at = chrono::Utc::now().timestamp();

                                    {
                                        let db = self.db.lock().unwrap();
                                        let name = Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("file");
                                        let _ = db.execute(
                                            "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, board_id, type, name, hash, size, source_peer, created_at)
                                             VALUES (?1, ?2, ?3, NULL, 'file', ?4, ?5, ?6, ?7, ?8)",
                                            params![
                                                id,
                                                gid,
                                                workspace_id,
                                                name,
                                                hash,
                                                size as i64,
                                                self.node_id,
                                                created_at
                                            ],
                                        );
                                    }

                                    if let Some(group_id) = gid {
                                        let topic = self.topics.lock().unwrap().get(&group_id).cloned();
                                        if let Some(topic) = topic {
                                            let evt = NetworkEvent::FileAvailable {
                                                id: id.clone(),
                                                group_id: Some(group_id.clone()),
                                                workspace_id: Some(workspace_id.clone()),
                                                board_id: None,
                                                name: Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string(),
                                                hash,
                                                size,
                                                source_peer: self.node_id.clone(),
                                                created_at,
                                            };
                                            tokio::spawn(async move {
                                                let mut guard = topic.lock().await;
                                                let _ = guard.broadcast(Bytes::from(
                                                    serde_json::to_vec(&evt).unwrap(),
                                                )).await;
                                            });
                                        }
                                    }
                                }
                            }

                            NetworkCommand::DeleteGroup { id } => {
                                let msg = serde_json::json!({
                                    "msg_type": "group_deleted",
                                    "event": NetworkEvent::GroupDeleted { id: id.clone() }
                                });

                                let mut discovery_topic = self.discovery_topic.lock().await;
                                let _ = discovery_topic.broadcast(Bytes::from(serde_json::to_string(&msg).unwrap())).await;
                            }

                            NetworkCommand::DeleteWorkspace { id } => {
                                // Get group_id to find the right topic
                                let group_id: Option<String> = {
                                    let db = self.db.lock().unwrap();
                                    let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1").unwrap();
                                    stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                                };

                                if let Some(gid) = group_id {
                                    let topic = self.topics.lock().unwrap().get(&gid).cloned();
                                    if let Some(topic) = topic {
                                        let evt = NetworkEvent::WorkspaceDeleted { id: id.clone() };
                                        tokio::spawn(async move {
                                            let mut guard = topic.lock().await;
                                            let _ = guard.broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap())).await;
                                        });
                                    }
                                }
                            }

                            NetworkCommand::DeleteBoard { id } => {
                                // Get group_id through workspace join
                                let group_id: Option<String> = {
                                    let db = self.db.lock().unwrap();
                                    let mut stmt = db.prepare(
                                        "SELECT w.group_id FROM objects o
                                             JOIN workspaces w ON o.workspace_id = w.id
                                             WHERE o.id=?1 AND o.type='whiteboard' LIMIT 1"
                                    ).unwrap();
                                    stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                                };

                                if let Some(gid) = group_id {
                                    let topic = self.topics.lock().unwrap().get(&gid).cloned();
                                    if let Some(topic) = topic {
                                        let evt = NetworkEvent::BoardDeleted { id: id.clone() };
                                        tokio::spawn(async move {
                                            let mut guard = topic.lock().await;
                                            let _ = guard.broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap())).await;
                                        });
                                    }
                                }
                            }

                            NetworkCommand::DeleteChat { id } => {
                                // Get group_id through workspace join
                                let group_id: Option<String> = {
                                    let db = self.db.lock().unwrap();
                                    let mut stmt = db.prepare(
                                        "SELECT w.group_id FROM objects o
                                             JOIN workspaces w ON o.workspace_id = w.id
                                             WHERE o.id=?1 AND o.type='chat' LIMIT 1"
                                    ).unwrap();
                                    stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0)).optional().unwrap()
                                };

                                if let Some(gid) = group_id {
                                    let topic = self.topics.lock().unwrap().get(&gid).cloned();
                                    if let Some(topic) = topic {
                                        let evt = NetworkEvent::ChatDeleted { id: id.clone() };
                                        tokio::spawn(async move {
                                            let mut guard = topic.lock().await;
                                            let _ = guard.broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap())).await;
                                        });
                                    }
                                }
                            }

                            NetworkCommand::StartChatStream { peer_id, workspace_id } => {
                                tracing::info!("Starting direct chat stream to peer: {peer_id} for workspace: {workspace_id}");

                                match PublicKey::from_str(&peer_id) {
                                    Ok(pk) => {
                                        let addr = EndpointAddr::new(pk);
                                        match self.endpoint.connect(addr, b"xsp-1.0").await {
                                            Ok(conn) => {
                                                match conn.open_bi().await {
                                                    Ok((mut tx, rx)) => {
                                                        // Send workspace_id as first frame
                                                        let init = ChatStreamInit { workspace_id: workspace_id.clone() };
                                                        match serde_json::to_vec(&init) {
                                                            Ok(init_bytes) => {
                                                                if let Err(e) = write_chat_frame(&mut tx, &init_bytes).await {
                                                                    tracing::error!("Failed to send chat init: {e}");
                                                                    continue;
                                                                }

                                                                // Create channel for outbound messages
                                                                let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

                                                                // Store sender for this peer
                                                                {
                                                                    let mut senders = self.chat_senders.lock().unwrap();
                                                                    senders.insert(peer_id.clone(), outbound_tx);
                                                                }

                                                                // Notify Swift that stream is ready
                                                                let _ = self.event_tx.send(SwiftEvent::ChatStreamReady {
                                                                    peer_id: peer_id.clone(),
                                                                    workspace_id: workspace_id.clone(),
                                                                });

                                                                let db = self.db.clone();
                                                                let event_tx = self.event_tx.clone();
                                                                let node_id = self.node_id.clone();
                                                                let chat_senders = self.chat_senders.clone();
                                                                let peer_id_clone = peer_id.clone();
                                                                let workspace_id_clone = workspace_id.clone();

                                                                // Spawn chat stream handler
                                                                tokio::spawn(async move {
                                                                    handle_chat_stream(
                                                                        db,
                                                                        event_tx.clone(),
                                                                        node_id,
                                                                        peer_id_clone.clone(),
                                                                        workspace_id_clone,
                                                                        tx,
                                                                        rx,
                                                                        outbound_rx,
                                                                    ).await;

                                                                    // Clean up sender when done
                                                                    {
                                                                        let mut senders = chat_senders.lock().unwrap();
                                                                        senders.remove(&peer_id_clone);
                                                                    }

                                                                    // Notify Swift that stream is closed
                                                                    let _ = event_tx.send(SwiftEvent::ChatStreamClosed {
                                                                        peer_id: peer_id_clone,
                                                                    });
                                                                });
                                                            }
                                                            Err(e) => {
                                                                tracing::error!("Failed to serialize chat init: {e}");
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        tracing::error!("Failed to open bi stream: {e}");
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tracing::error!("Failed to connect to peer {peer_id}: {e}");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Invalid peer_id: {e}");
                                    }
                                }
                            }

                            NetworkCommand::SendDirectChat { peer_id, workspace_id, message, parent_id } => {
                                let sender = {
                                    let senders = self.chat_senders.lock().unwrap();
                                    senders.get(&peer_id).cloned()
                                };

                                match sender {
                                    Some(tx) => {
                                        let msg = DirectChatOutbound {
                                            workspace_id,
                                            message,
                                            parent_id,
                                        };
                                        if let Err(e) = tx.send(msg) {
                                            tracing::error!("Failed to send to chat channel for {peer_id}: {e}");
                                        }
                                    }
                                    None => {
                                        tracing::warn!("No active chat stream to peer {peer_id}");
                                    }
                                }
                            }

                            _ => {}
                        }

                        // Continue to next iteration without re-acquiring guard
                        continue;
                    }
                }
            }
        }
    }

    async fn setup_discovery(self: Arc<Self>) {
        // Load groups from DB
        *self.groups.lock().unwrap() = Self::get_groups_from_db(self.db.clone());

        // Listen for discovery messages
        let self_clone = self.clone();
        tokio::spawn(async move {
            let discovery_topic = self_clone.discovery_topic.clone();
            loop {
                let event = {
                    let mut topic_guard = discovery_topic.lock().await;
                    topic_guard.next().await
                };

                if let Some(Ok(event)) = event {
                    if let GossipEvent::Received(msg) = event {
                        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&msg.content)
                        {
                            if let Some(msg_type) = json.get("msg_type").and_then(|v| v.as_str()) {
                                // Handle group deletion from peers
                                if msg_type == "group_deleted" {
                                    if let Some(event_obj) = json.get("event") {
                                        if let Ok(evt) = serde_json::from_value::<NetworkEvent>(event_obj.clone()) {
                                            if let NetworkEvent::GroupDeleted { id } = evt {
                                                {
                                                    let db = self_clone.db.lock().unwrap();
                                                    let _ = db.execute("DELETE FROM objects WHERE group_id=?1", params![&id]);
                                                    let _ = db.execute("DELETE FROM workspaces WHERE group_id=?1", params![&id]);
                                                    let _ = db.execute("DELETE FROM groups WHERE id=?1", params![&id]);
                                                }

                                                self_clone.topics.lock().unwrap().remove(&id);
                                                self_clone.groups.lock().unwrap().remove(&id);

                                                let _ = self_clone.event_tx.send(
                                                    SwiftEvent::GroupDeleted { id }
                                                );
                                            }
                                        }
                                    }
                                }
                                // Handle group discovery
                                else if msg_type == "groups_exchange" {
                                    if let Some(groups) = json.get("local_groups").and_then(|v| v.as_array())
                                    {
                                        let new_groups: Vec<String> = {
                                            let mut my_groups = self_clone.groups.lock().unwrap();
                                            groups.iter().filter_map(|g| g.as_str()).filter(|gid| my_groups.insert(gid.to_string())).map(|s| s.to_string()).collect()
                                        };

                                        for gid in new_groups {
                                            let topic_id = TopicId::from_bytes(
                                                blake3::hash(
                                                    format!("cyan/group/{}", gid).as_bytes(),
                                                ).as_bytes()[..32].try_into().unwrap(),
                                            );

                                            if let Ok(topic) = self_clone.gossip.subscribe_and_join(topic_id, vec![]).await
                                            {
                                                self_clone.topics.lock().unwrap().insert(
                                                    gid.clone(),
                                                    Arc::new(tokio::sync::Mutex::new(topic)),
                                                );
                                                self_clone.need_snapshot.lock().unwrap().insert(gid, AtomicBool::new(true));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        // Periodically broadcast our groups
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let local_groups = self_clone.groups.lock().unwrap().clone();
                let list: Vec<String> = local_groups.into_iter().collect();

                let msg = serde_json::json!({
                    "msg_type": "groups_exchange",
                    "node_id": self_clone.node_id,
                    "local_groups": list,
                });

                let mut discovery_topic = self_clone.discovery_topic.lock().await;
                let _ = discovery_topic.broadcast(Bytes::from(serde_json::to_string(&msg).unwrap())).await;
            }
        });
    }
}

// ---------- JSON dump & seeding ----------
#[derive(Debug, Serialize, Deserialize)]
struct TreeSnapshotDTO {
    groups: Vec<Group>,
    workspaces: Vec<Workspace>,
    whiteboards: Vec<WhiteboardDTO>,
    files: Vec<FileDTO>,
    chats: Vec<ChatDTO>,
    #[serde(default)]
    integrations: Vec<IntegrationBindingDTO>,
    #[serde(default)]
    board_metadata: Vec<BoardMetadataDTO>,
}
#[derive(Debug, Serialize, Deserialize)]
struct BoardMetadataDTO {
    board_id: String,
    labels: Vec<String>,
    rating: i32,
    view_count: i32,
    contains_model: Option<String>,
    contains_skills: Vec<String>,
    board_type: String,
    last_accessed: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IntegrationBindingDTO {
    id: String,
    scope_type: String,
    scope_id: String,
    integration_type: String,
    config: serde_json::Value,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WhiteboardDTO {
    id: String,
    workspace_id: String,
    name: String,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileDTO {
    id: String,
    group_id: Option<String>,
    workspace_id: Option<String>,
    board_id: Option<String>,
    name: String,
    hash: String,
    size: u64,
    source_peer: Option<String>,
    local_path: Option<String>,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatDTO {
    id: String,
    workspace_id: String,
    message: String,
    author: String,
    parent_id: Option<String>,
    timestamp: i64,
}

fn build_group_snapshot(db: &Arc<Mutex<Connection>>, group_id: &str) -> Result<TreeSnapshotDTO> {
    let db = db.lock().map_err(|e| anyhow!("Failed to lock db: {}", e))?;

    // Get the specific group
    let group: Group = {
        let mut stmt = db.prepare("SELECT id, name, icon, color, created_at FROM groups WHERE id=?1")?;
        stmt.query_row(params![group_id], |r| {
            Ok(Group {
                id: r.get(0)?,
                name: r.get(1)?,
                icon: r.get(2)?,
                color: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
    };

    // Get workspaces for this group
    let workspaces: Vec<Workspace> = {
        let mut stmt = db.prepare("SELECT id, group_id, name, created_at FROM workspaces WHERE group_id=?1 ORDER BY name")?;
        let rows = stmt.query_map(params![group_id], |r| {
            Ok(Workspace {
                id: r.get(0)?,
                group_id: r.get(1)?,
                name: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        rows.filter_map(Result::ok).collect()
    };

    // Get workspace IDs for querying boards
    let workspace_ids: Vec<String> = workspaces.iter().map(|w| w.id.clone()).collect();

    // Get whiteboards for these workspaces
    let whiteboards: Vec<WhiteboardDTO> = if !workspace_ids.is_empty() {
        let placeholders = workspace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT id, workspace_id, name, created_at FROM objects WHERE type='whiteboard' AND workspace_id IN ({}) ORDER BY name",
            placeholders
        );
        let mut stmt = db.prepare(&query)?;
        let params: Vec<&dyn rusqlite::ToSql> = workspace_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok(WhiteboardDTO {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                name: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        rows.filter_map(Result::ok).collect()
    } else {
        vec![]
    };

    // Get files for this group and its workspaces
    let files: Vec<FileDTO> = {
        let mut all_files = vec![];

        // Group-level files
        let mut stmt = db.prepare("SELECT id, group_id, workspace_id, board_id, name, hash, size, source_peer, local_path, created_at FROM objects WHERE type='file' AND group_id=?1")?;
        let group_files = stmt.query_map(params![group_id], |r| {
            Ok(FileDTO {
                id: r.get(0)?,
                group_id: r.get(1)?,
                workspace_id: r.get(2)?,
                board_id: r.get(3)?,
                name: r.get(4)?,
                hash: r.get(5)?,
                size: r.get::<_, i64>(6)? as u64,
                source_peer: r.get(7)?,
                local_path: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?;
        all_files.extend(group_files.filter_map(Result::ok));

        // Workspace-level files
        if !workspace_ids.is_empty() {
            let placeholders = workspace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let query = format!(
                "SELECT id, group_id, workspace_id, board_id, name, hash, size, source_peer, local_path, created_at FROM objects WHERE type='file' AND workspace_id IN ({})",
                placeholders
            );
            let mut stmt = db.prepare(&query)?;
            let params: Vec<&dyn rusqlite::ToSql> = workspace_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let ws_files = stmt.query_map(params.as_slice(), |r| {
                Ok(FileDTO {
                    id: r.get(0)?,
                    group_id: r.get(1)?,
                    workspace_id: r.get(2)?,
                    board_id: r.get(3)?,
                    name: r.get(4)?,
                    hash: r.get(5)?,
                    size: r.get::<_, i64>(6)? as u64,
                    source_peer: r.get(7)?,
                    local_path: r.get(8)?,
                    created_at: r.get(9)?,
                })
            })?;
            all_files.extend(ws_files.filter_map(Result::ok));
        }

        all_files
    };

    // Get chats for these workspaces
    let chats: Vec<ChatDTO> = if !workspace_ids.is_empty() {
        let placeholders = workspace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT id, workspace_id, name, hash, data, created_at FROM objects WHERE type='chat' AND workspace_id IN ({}) ORDER BY created_at",
            placeholders
        );
        let mut stmt = db.prepare(&query)?;
        let params: Vec<&dyn rusqlite::ToSql> = workspace_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            let parent_bytes: Option<Vec<u8>> = r.get(4)?;
            let parent_id = parent_bytes.and_then(|b| String::from_utf8(b).ok());
            Ok(ChatDTO {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                message: r.get(2)?,
                author: r.get(3)?,
                parent_id,
                timestamp: r.get(5)?,
            })
        })?;
        rows.filter_map(Result::ok).collect()
    } else {
        vec![]
    };

    let integrations: Vec<IntegrationBindingDTO> = {
        match db.prepare(
            "SELECT id, scope_type, scope_id, integration_type, config_json, created_at
             FROM integration_bindings ORDER BY created_at"
        ) {
            Ok(mut stmt) => {
                stmt.query_map([], |r| {
                    let config_str: String = r.get(4)?;
                    let config = serde_json::from_str(&config_str).unwrap_or(serde_json::Value::Null);
                    Ok(IntegrationBindingDTO {
                        id: r.get(0)?,
                        scope_type: r.get(1)?,
                        scope_id: r.get(2)?,
                        integration_type: r.get(3)?,
                        config,
                        created_at: r.get(5)?,
                    })
                }).unwrap()
                    .filter_map(|r| r.ok())
                    .collect()
            }
            Err(_) => vec![], // Table doesn't exist yet
        }
    };
    let board_metadata: Vec<BoardMetadataDTO> = {
        match db.prepare(
            "SELECT m.board_id, m.labels, m.rating, m.view_count, m.contains_model, m.contains_skills, m.board_type, m.last_accessed FROM board_metadata m JOIN objects o ON m.board_id = o.id JOIN workspaces w ON o.workspace_id = w.id WHERE w.group_id = ?1"
        ) {
            Ok(mut stmt) => {
                stmt.query_map(params![group_id], |row| {
                    let labels_json: String = row.get(1)?;
                    let skills_json: String = row.get(5)?;
                    Ok(BoardMetadataDTO {
                        board_id: row.get(0)?,
                        labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                        rating: row.get(2)?,
                        view_count: row.get(3)?,
                        contains_model: row.get(4)?,
                        contains_skills: serde_json::from_str(&skills_json).unwrap_or_default(),
                        board_type: row.get(6)?,
                        last_accessed: row.get(7)?,
                    })
                }).unwrap().filter_map(|r| r.ok()).collect()
            }
            Err(_) => vec![],
        }
    };
    let group_clone = group.name.clone();
    let snapshot = TreeSnapshotDTO {
        groups: vec![group],
        workspaces,
        whiteboards,
        files,
        chats,
        integrations,
        board_metadata,
    };
    tracing::info!("snapshot {snapshot:?} built for group : {group_clone:?}");
    Ok(snapshot)
}

fn dump_tree_json(db: &Arc<Mutex<Connection>>) -> String {
    let db = db.lock().unwrap();

    let groups: Vec<Group> = {
        let mut stmt = db.prepare("SELECT id, name, icon, color, created_at FROM groups ORDER BY name").unwrap();
        let rows = stmt.query_map([], |r| {
            Ok(Group {
                id: r.get(0)?,
                name: r.get(1)?,
                icon: r.get(2)?,
                color: r.get(3)?,
                created_at: r.get(4)?,
            })
        }).unwrap();
        rows.filter_map(Result::ok).collect()
    };

    let workspaces: Vec<Workspace> = {
        let mut stmt = db.prepare("SELECT id, group_id, name, created_at FROM workspaces ORDER BY name").unwrap();
        let rows = stmt.query_map([], |r| {
            Ok(Workspace {
                id: r.get(0)?,
                group_id: r.get(1)?,
                name: r.get(2)?,
                created_at: r.get(3)?,
            })
        }).unwrap();
        rows.filter_map(Result::ok).collect()
    };

    let whiteboards: Vec<WhiteboardDTO> = {
        let mut stmt = db.prepare(
            "SELECT id, workspace_id, name, created_at FROM objects WHERE type='whiteboard' \
                 ORDER BY name",
        ).unwrap();
        let rows = stmt.query_map([], |r| {
            Ok(WhiteboardDTO {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                name: r.get(2)?,
                created_at: r.get(3)?,
            })
        }).unwrap();
        rows.filter_map(Result::ok).collect()
    };

    let files: Vec<FileDTO> = {
        let mut stmt = db.prepare(
            "SELECT id, group_id, workspace_id, board_id, name, hash, size, source_peer, local_path, created_at FROM objects \
                 WHERE type='file' ORDER BY name",
        ).unwrap();
        let rows = stmt.query_map([], |r| {
            Ok(FileDTO {
                id: r.get(0)?,
                group_id: r.get(1)?,
                workspace_id: r.get(2)?,
                board_id: r.get(3)?,
                name: r.get(4)?,
                hash: r.get(5)?,
                size: r.get::<_, i64>(6)? as u64,
                source_peer: r.get(7)?,
                local_path: r.get(8)?,
                created_at: r.get(9)?,
            })
        }).unwrap();
        rows.filter_map(Result::ok).collect()
    };

    let chats: Vec<ChatDTO> = {
        let mut stmt = db.prepare(
            "SELECT id, workspace_id, name, hash, data, created_at FROM objects \
                 WHERE type='chat' ORDER BY created_at",
        ).unwrap();
        let rows = stmt.query_map([], |r| {
            let parent_bytes: Option<Vec<u8>> = r.get(4)?;
            let parent_id = parent_bytes.and_then(|b| String::from_utf8(b).ok());
            Ok(ChatDTO {
                id: r.get(0)?,
                workspace_id: r.get(1)?,
                message: r.get(2)?,
                author: r.get(3)?,
                parent_id,
                timestamp: r.get(5)?,
            })
        }).unwrap();
        rows.filter_map(Result::ok).collect()
    };

    let integrations: Vec<IntegrationBindingDTO> = {
        match db.prepare(
            "SELECT id, scope_type, scope_id, integration_type, config_json, created_at
             FROM integration_bindings ORDER BY created_at"
        ) {
            Ok(mut stmt) => {
                stmt.query_map([], |r| {
                    let config_str: String = r.get(4)?;
                    let config = serde_json::from_str(&config_str).unwrap_or(serde_json::Value::Null);
                    Ok(IntegrationBindingDTO {
                        id: r.get(0)?,
                        scope_type: r.get(1)?,
                        scope_id: r.get(2)?,
                        integration_type: r.get(3)?,
                        config,
                        created_at: r.get(5)?,
                    })
                }).unwrap()
                    .filter_map(|r| r.ok())
                    .collect()
            }
            Err(_) => vec![],
        }
    };

    let board_metadata: Vec<BoardMetadataDTO> = {
        match db.prepare(
            "SELECT board_id, labels, rating, view_count, contains_model, contains_skills, board_type, last_accessed FROM board_metadata"
        ) {
            Ok(mut stmt) => {
                stmt.query_map([], |row| {
                    let labels_json: String = row.get(1)?;
                    let skills_json: String = row.get(5)?;
                    Ok(BoardMetadataDTO {
                        board_id: row.get(0)?,
                        labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                        rating: row.get(2)?,
                        view_count: row.get(3)?,
                        contains_model: row.get(4)?,
                        contains_skills: serde_json::from_str(&skills_json).unwrap_or_default(),
                        board_type: row.get(6)?,
                        last_accessed: row.get(7)?,
                    })
                }).unwrap().filter_map(|r| r.ok()).collect()
            }
            Err(_) => vec![],
        }
    };

    let snapshot = TreeSnapshotDTO {
        groups,
        workspaces,
        whiteboards,
        files,
        chats,
        integrations,
        board_metadata,
    };
    serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string())
}

fn seed_demo_if_empty(db: &Arc<Mutex<Connection>>) {
    let db_clone = db.clone();
    let count: i64 = db_clone.lock().unwrap().query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0)).unwrap_or(1);
    if count > 0 {
        return;
    }

    // Create demo group
    let group_id = blake3::hash(b"demo-group").to_hex().to_string();
    let now = chrono::Utc::now().timestamp();
    {
        let _ = db_clone.lock().unwrap().execute(
            "INSERT INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![group_id, "Demo Group", "folder.fill", "#00AEEF", now],
        );
    }

    // Create demo workspace
    let workspace_id = blake3::hash(b"demo-workspace").to_hex().to_string();
    {
        let _ = db_clone.lock().unwrap().execute(
            "INSERT INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![workspace_id, group_id, "Demo Workspace", now],
        );
    }

    // Create demo board
    let board_id = blake3::hash(b"demo-board").to_hex().to_string();
    {
        let _ = db_clone.lock().unwrap().execute(
            "INSERT INTO objects (id, workspace_id, type, name, created_at) VALUES (?1, ?2, \
             'whiteboard', ?3, ?4)",
            params![board_id, workspace_id, "Demo Board", now],
        );
    }
}

// ---------- FFI helpers ----------
fn compute_or_load_node_id() -> String {
    if let Some(dir) = DATA_DIR.get() {
        let node_id_file = dir.join("node_id.txt");
        if let Ok(id) = std::fs::read_to_string(&node_id_file) {
            return id.trim().to_string();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    save_node_id_to_disk(&id);
    id
}

fn save_node_id_to_disk(id: &str) {
    if let Some(dir) = DATA_DIR.get() {
        let node_id_file = dir.join("node_id.txt");
        let _ = std::fs::write(node_id_file, id);
    }
}

unsafe fn cstr_arg(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_string())
    }
}

fn to_c_string(s: String) -> *const c_char {
    CString::new(s).unwrap().into_raw()
}

// ---------- FFI: lifecycle ----------
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_data_dir(path: *const c_char) -> bool {
    eprintln!("🔥 cyan_set_data_dir ENTERED");
    let Some(s) = (unsafe { cstr_arg(path) }) else {
        eprintln!("❌ cyan_set_data_dir: path is null");
        return false;
    };
    eprintln!("📁 cyan_set_data_dir path: {}", s);
    let path_buf = PathBuf::from(s);

    // Set cyan_db_path on AIBridge if system exists
    if let Some(system) = SYSTEM.get() {
        let cyan_db_path = path_buf.join("cyan.db");
        let ai_bridge = system.ai_bridge.clone();
        if let Some(rt) = RUNTIME.get() {
            rt.spawn(async move {
                ai_bridge.set_cyan_db_path(cyan_db_path).await;
            });
        }
    }

    DATA_DIR.set(path_buf).is_ok()
}
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_discovery_key(key: *const c_char) -> bool {
    let Some(s) = (unsafe { cstr_arg(key) }) else {
        return false;
    };
    DISCOVERY_KEY.set(s).is_ok()
}

/// Initialize Cyan with ephemeral identity (for testing).
/// Different NodeID each launch - use for P2P mesh testing.
#[unsafe(no_mangle)]
pub extern "C" fn cyan_init(db_path: *const c_char) -> bool {
    if SYSTEM.get().is_some() {
        return true;
    }
    let path = unsafe {
        if db_path.is_null() {
            eprintln!("Database path is null");
            return false;
        }
        CStr::from_ptr(db_path).to_string_lossy().to_string()
    };
    let res = std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().expect("runtime");
        RUNTIME.set(runtime).ok();

        let rt = RUNTIME.get().unwrap();
        // Pass None for ephemeral identity (test mode)
        let sys = rt.block_on(async { CyanSystem::new(path, None).await });

        match sys {
            Ok(s) => {
                println!("⚠️ Cyan initialized (EPHEMERAL) with ID: {}", &s.node_id[..16]);
                SYSTEM.set(Arc::new(s)).is_ok()
            }
            Err(e) => {
                eprintln!("Failed init: {e}");
                false
            }
        }
    }).join();

    res.unwrap_or(false)
}

/// Initialize Cyan with persistent identity from Swift Keychain.
/// Same NodeID across app launches - use for production.
/// secret_key_hex: 64-character hex string (32 bytes)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_init_with_identity(
    db_path: *const c_char,
    secret_key_hex: *const c_char,
) -> bool {
    eprintln!("🔥 cyan_init_with_identity");
    if SYSTEM.get().is_some() {
        return true;
    }

    // Parse db_path
    let path = unsafe {
        if db_path.is_null() {
            eprintln!("Database path is null");
            return false;
        }
        CStr::from_ptr(db_path).to_string_lossy().to_string()
    };

    // Parse secret_key_hex
    let secret_key_bytes: [u8; 32] = unsafe {
        if secret_key_hex.is_null() {
            eprintln!("Secret key is null");
            return false;
        }
        let hex_str = match CStr::from_ptr(secret_key_hex).to_str() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("Invalid secret key UTF-8");
                return false;
            }
        };

        let bytes = match hex::decode(hex_str) {
            Ok(b) if b.len() == 32 => b,
            Ok(b) => {
                eprintln!("Secret key must be 32 bytes, got {}", b.len());
                return false;
            }
            Err(e) => {
                eprintln!("Invalid secret key hex: {e}");
                return false;
            }
        };

        bytes.try_into().unwrap()
    };

    let res = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("runtime");
        RUNTIME.set(runtime).ok();

        let rt = RUNTIME.get().unwrap();
        let sys = rt.block_on(async {
            CyanSystem::new(path, Some(secret_key_bytes)).await
        });

        match sys {
            Ok(s) => {
                println!("✅ Cyan initialized (PERSISTENT) with ID: {}", &s.node_id[..16]);
                SYSTEM.set(Arc::new(s)).is_ok()
            }
            Err(e) => {
                eprintln!("Failed init with identity: {e}");
                false
            }
        }
    }).join();

    res.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_xaero_id() -> *const c_char {
    let id = NODE_ID.get_or_init(|| compute_or_load_node_id());
    to_c_string(id.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_xaero_id(id: *const c_char) -> bool {
    if id.is_null() {
        return false;
    }
    let s = unsafe { CStr::from_ptr(id) }.to_str().ok().unwrap().to_string();

    let _ = NODE_ID.set(s.clone());
    save_node_id_to_disk(&s);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

/// Check if the Cyan system is initialized and ready
#[unsafe(no_mangle)]
pub extern "C" fn cyan_is_ready() -> bool {
    SYSTEM.get().is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_poll_events(_component: *const c_char) -> *mut c_char {
    let Some(cyan) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let event_ffi_buffer = cyan.event_ffi_buffer.clone();
    let buffer = event_ffi_buffer.lock();
    match buffer {
        Ok(mut buff) => match buff.pop_front() {
            None => std::ptr::null_mut(),
            Some(event_json) => CString::new(event_json).unwrap().into_raw(),
        },
        Err(e) => {
            tracing::error!("failed to lock due to {e:?}");
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_send_command(_component: *const c_char, json: *const c_char) -> bool {
    let json_str = unsafe { CStr::from_ptr(json).to_string_lossy().to_string() };

    let Some(system) = SYSTEM.get() else {
        return false;
    };

    match serde_json::from_str::<CommandMsg>(&json_str) {
        Ok(command) => match system.command_tx.send(command) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("failed to send command: {e:?}");
                false
            }
        },
        Err(e) => {
            eprintln!("failed to parse command: {e:?}");
            false
        }
    }
}

// ---------- FFI: groups ----------
#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_group(
    name: *const c_char,
    icon: *const c_char,
    color: *const c_char,
) {
    let Some(name) = (unsafe { cstr_arg(name) }) else {
        return;
    };
    let icon = (unsafe { cstr_arg(icon) }).unwrap_or_else(|| "folder.fill".into());
    let color = (unsafe { cstr_arg(color) }).unwrap_or_else(|| "#00AEEF".into());

    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::CreateGroup { name, icon, color });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_group(id: *const c_char, new_name: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let Some(name) = (unsafe { cstr_arg(new_name) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::RenameGroup { id, name });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_delete_group(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteGroup { id });
}

// ---------- FFI: workspaces ----------
#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_workspace(group_id: *const c_char, name: *const c_char) {
    let Some(gid) = (unsafe { cstr_arg(group_id) }) else {
        return;
    };
    let Some(name) = (unsafe { cstr_arg(name) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::CreateWorkspace {
        group_id: gid,
        name,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_workspace(id: *const c_char, new_name: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let Some(name) = (unsafe { cstr_arg(new_name) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::RenameWorkspace { id, name });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_delete_workspace(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteWorkspace { id });
}

// ---------- FFI: boards ----------
#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_board(workspace_id: *const c_char, name: *const c_char) {
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return;
    };
    let Some(name) = (unsafe { cstr_arg(name) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::CreateBoard {
        workspace_id: wid,
        name,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_board(id: *const c_char, new_name: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let Some(name) = (unsafe { cstr_arg(new_name) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::RenameBoard { id, name });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_delete_board(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteBoard { id });
}

// ---------- FFI: chats ----------
#[unsafe(no_mangle)]
pub extern "C" fn cyan_send_chat(
    workspace_id: *const c_char,
    message: *const c_char,
    parent_id: *const c_char,
) {
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return;
    };
    let Some(msg) = (unsafe { cstr_arg(message) }) else {
        return;
    };
    let parent = unsafe { cstr_arg(parent_id) }; // Can be null for root messages

    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::SendChat {
        workspace_id: wid,
        message: msg,
        parent_id: parent,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_delete_chat(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteChat { id });
}

// ---------- FFI: direct chats ----------
/// Start a direct QUIC chat stream with a peer
#[unsafe(no_mangle)]
pub extern "C" fn cyan_start_direct_chat(
    peer_id: *const c_char,
    workspace_id: *const c_char,
) {
    let Some(pid) = (unsafe { cstr_arg(peer_id) }) else {
        return;
    };
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.network_tx.send(NetworkCommand::StartChatStream {
        peer_id: pid,
        workspace_id: wid,
    });
}

/// Send a message on an existing direct chat stream
#[unsafe(no_mangle)]
pub extern "C" fn cyan_send_direct_chat(
    peer_id: *const c_char,
    workspace_id: *const c_char,
    message: *const c_char,
    parent_id: *const c_char,
) {
    let Some(pid) = (unsafe { cstr_arg(peer_id) }) else {
        return;
    };
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return;
    };
    let Some(msg) = (unsafe { cstr_arg(message) }) else {
        return;
    };
    let parent = unsafe { cstr_arg(parent_id) };

    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.network_tx.send(NetworkCommand::SendDirectChat {
        peer_id: pid,
        workspace_id: wid,
        message: msg,
        parent_id: parent,
    });
}

// ---------- FFI: uploads ----------
#[unsafe(no_mangle)]
pub extern "C" fn cyan_upload_file_to_group(group_id: *const c_char, path: *const c_char) {
    let Some(gid) = (unsafe { cstr_arg(group_id) }) else {
        return;
    };
    let Some(p) = (unsafe { cstr_arg(path) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.network_tx.send(NetworkCommand::UploadToGroup {
        group_id: gid,
        path: p,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_upload_file_to_workspace(workspace_id: *const c_char, path: *const c_char) {
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return;
    };
    let Some(p) = (unsafe { cstr_arg(path) }) else {
        return;
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return,
    };
    let _ = sys.network_tx.send(NetworkCommand::UploadToWorkspace {
        workspace_id: wid,
        path: p,
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_seed_demo_if_empty() {
    if let Some(sys) = SYSTEM.get() {
        let _ = sys.command_tx.send(CommandMsg::SeedDemoIfEmpty);
    }
}

// ---------- FFI: peer queries ----------
/// Get peers for a specific group as JSON array: ["peer_id_1", "peer_id_2", ...]
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_group_peers(group_id: *const c_char) -> *mut c_char {
    let Some(gid) = (unsafe { cstr_arg(group_id) }) else {
        return std::ptr::null_mut();
    };
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let peers: Vec<String> = {
        let peers_map = sys.peers_per_group.lock().unwrap();
        peers_map.get(&gid)
            .map(|set| set.iter().map(|pk| pk.to_string()).collect())
            .unwrap_or_default()
    };

    match serde_json::to_string(&peers) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get all peers grouped by group_id as JSON: { "group_id": ["peer1", "peer2"], ... }
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_all_peers() -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let all_peers: HashMap<String, Vec<String>> = {
        let peers_map = sys.peers_per_group.lock().unwrap();
        peers_map.iter()
            .map(|(gid, set)| (gid.clone(), set.iter().map(|pk| pk.to_string()).collect()))
            .collect()
    };

    match serde_json::to_string(&all_peers) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get count of peers for a specific group
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_group_peer_count(group_id: *const c_char) -> i32 {
    let Some(gid) = (unsafe { cstr_arg(group_id) }) else {
        return 0;
    };
    let Some(sys) = SYSTEM.get() else {
        return 0;
    };

    let peers_map = sys.peers_per_group.lock().unwrap();
    peers_map.get(&gid)
        .map(|set| set.len() as i32)
        .unwrap_or(0)
}

/// Get total peer count across all groups
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_total_peer_count() -> i32 {
    let Some(sys) = SYSTEM.get() else {
        return 0;
    };

    let peers_map = sys.peers_per_group.lock().unwrap();
    peers_map.values()
        .map(|set| set.len())
        .sum::<usize>() as i32
}

/// Get total object count (whiteboards + files)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_object_count() -> i32 {
    let Some(sys) = SYSTEM.get() else {
        return 0;
    };

    let db = sys.db.lock().unwrap();
    let count: i32 = db.query_row(
        "SELECT COUNT(*) FROM objects WHERE type IN ('whiteboard', 'file')",
        [],
        |row| row.get(0)
    ).unwrap_or(0);

    count
}

// ---------- Board Query FFI ----------

/// Get all boards for a group (across all workspaces in that group)
/// Returns JSON array: [{"id": "...", "workspace_id": "...", "group_id": "...", "name": "...", "created_at": 123}]
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_boards_for_group(group_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let gid = unsafe { CStr::from_ptr(group_id) }.to_string_lossy().to_string();

    let boards: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        // First get all workspace IDs for this group
        let mut ws_stmt = db.prepare(
            "SELECT id FROM workspaces WHERE group_id = ?1"
        ).unwrap();

        let workspace_ids: Vec<String> = ws_stmt
            .query_map(params![gid.clone()], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        if workspace_ids.is_empty() {
            return CString::new("[]").unwrap().into_raw();
        }

        // Query boards for all workspaces in this group
        let mut all_boards = Vec::new();
        for wid in &workspace_ids {
            let mut stmt = db.prepare(
                "SELECT id, workspace_id, name, created_at FROM objects
                 WHERE type = 'whiteboard' AND workspace_id = ?1
                 ORDER BY created_at DESC"
            ).unwrap();

            let boards_iter = stmt.query_map(params![wid], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "workspace_id": row.get::<_, String>(1)?,
                    "group_id": gid.clone(),
                    "name": row.get::<_, String>(2)?,
                    "created_at": row.get::<_, i64>(3)?,
                    "element_count": 0
                }))
            }).unwrap();

            for board in boards_iter.filter_map(|r| r.ok()) {
                all_boards.push(board);
            }
        }
        all_boards
    };

    match serde_json::to_string(&boards) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Get all boards for a specific workspace
/// Returns JSON array
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_boards_for_workspace(workspace_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let wid = unsafe { CStr::from_ptr(workspace_id) }.to_string_lossy().to_string();

    let boards: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        // Get group_id for this workspace
        let group_id: String = db.query_row(
            "SELECT group_id FROM workspaces WHERE id = ?1",
            params![wid.clone()],
            |row| row.get(0)
        ).unwrap_or_default();

        let mut stmt = db.prepare(
            "SELECT id, workspace_id, name, created_at FROM objects
             WHERE type = 'whiteboard' AND workspace_id = ?1
             ORDER BY created_at DESC"
        ).unwrap();

        stmt.query_map(params![wid], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "workspace_id": row.get::<_, String>(1)?,
                "group_id": group_id.clone(),
                "name": row.get::<_, String>(2)?,
                "created_at": row.get::<_, i64>(3)?,
                "element_count": 0
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&boards) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Get all boards across all groups and workspaces
/// Returns JSON array
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_all_boards() -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let boards: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = db.prepare(
            "SELECT o.id, o.workspace_id, w.group_id, o.name, o.created_at
             FROM objects o
             LEFT JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.type = 'whiteboard'
             ORDER BY o.created_at DESC"
        ).unwrap();

        stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "workspace_id": row.get::<_, String>(1)?,
                "group_id": row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                "name": row.get::<_, String>(3)?,
                "created_at": row.get::<_, i64>(4)?,
                "element_count": 0
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&boards) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

// ---------- Whiteboard Elements FFI ----------

/// Load all elements for a whiteboard/board
/// Returns JSON array of element objects
#[unsafe(no_mangle)]
pub extern "C" fn cyan_load_whiteboard_elements(board_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    let elements: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = db.prepare(
            "SELECT id, board_id, element_type, x, y, width, height, z_index,
                    style_json, content_json, created_at, updated_at
             FROM whiteboard_elements
             WHERE board_id = ?1
             ORDER BY z_index ASC, created_at ASC"
        ).unwrap();

        stmt.query_map(params![bid], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "board_id": row.get::<_, String>(1)?,
                "element_type": row.get::<_, String>(2)?,
                "x": row.get::<_, f64>(3)?,
                "y": row.get::<_, f64>(4)?,
                "width": row.get::<_, f64>(5)?,
                "height": row.get::<_, f64>(6)?,
                "z_index": row.get::<_, i32>(7)?,
                "style_json": row.get::<_, Option<String>>(8)?,
                "content_json": row.get::<_, Option<String>>(9)?,
                "created_at": row.get::<_, i64>(10)?,
                "updated_at": row.get::<_, i64>(11)?
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&elements) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Save (insert or update) a whiteboard element
/// Input: JSON object with element fields
/// Returns: true on success
#[unsafe(no_mangle)]
pub extern "C" fn cyan_save_whiteboard_element(element_json: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let json_str = unsafe { CStr::from_ptr(element_json) }.to_string_lossy().to_string();

    let Ok(elem) = serde_json::from_str::<serde_json::Value>(&json_str) else {
        return false;
    };

    let id = elem["id"].as_str().unwrap_or("").to_string();
    let board_id = elem["board_id"].as_str().unwrap_or("").to_string();
    let element_type = elem["element_type"].as_str().unwrap_or("rectangle").to_string();
    let x = elem["x"].as_f64().unwrap_or(0.0);
    let y = elem["y"].as_f64().unwrap_or(0.0);
    let width = elem["width"].as_f64().unwrap_or(100.0);
    let height = elem["height"].as_f64().unwrap_or(100.0);
    let z_index = elem["z_index"].as_i64().unwrap_or(0) as i32;
    let style_json = elem["style_json"].as_str().map(|s| s.to_string());
    let content_json = elem["content_json"].as_str().map(|s| s.to_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let created_at = elem["created_at"].as_i64().unwrap_or(now);
    let updated_at = now;

    if id.is_empty() || board_id.is_empty() {
        return false;
    }

    // Check if element exists (for add vs update event)
    let is_new: bool;
    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        // Check if exists
        is_new = db.query_row(
            "SELECT 1 FROM whiteboard_elements WHERE id = ?1",
            params![&id],
            |_| Ok(())
        ).is_err();

        // Get group_id via board -> workspace -> group
        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&board_id],
            |row| row.get(0)
        ).unwrap_or_default();

        // Insert or replace
        let result = db.execute(
            "INSERT OR REPLACE INTO whiteboard_elements
             (id, board_id, element_type, x, y, width, height, z_index, style_json, content_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![&id, &board_id, &element_type, x, y, width, height, z_index, &style_json, &content_json, created_at, updated_at]
        );

        if result.is_err() {
            return false;
        }
    }

    // Broadcast via gossip
    if !group_id.is_empty() {
        let event = if is_new {
            NetworkEvent::WhiteboardElementAdded {
                id: id.clone(),
                board_id: board_id.clone(),
                element_type,
                x, y, width, height, z_index,
                style_json,
                content_json,
                created_at,
                updated_at,
            }
        } else {
            NetworkEvent::WhiteboardElementUpdated {
                id: id.clone(),
                board_id: board_id.clone(),
                element_type,
                x, y, width, height, z_index,
                style_json,
                content_json,
                updated_at,
            }
        };

        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event,
        });
    }

    true
}

/// Delete a whiteboard element by ID
#[unsafe(no_mangle)]
pub extern "C" fn cyan_delete_whiteboard_element(element_id: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let id = unsafe { CStr::from_ptr(element_id) }.to_string_lossy().to_string();

    if id.is_empty() {
        return false;
    }

    let board_id: String;
    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        // Get board_id before deleting
        board_id = db.query_row(
            "SELECT board_id FROM whiteboard_elements WHERE id = ?1",
            params![&id],
            |row| row.get(0)
        ).unwrap_or_default();

        // Get group_id via board -> workspace -> group
        group_id = if !board_id.is_empty() {
            db.query_row(
                "SELECT w.group_id FROM objects o
                 JOIN workspaces w ON o.workspace_id = w.id
                 WHERE o.id = ?1",
                params![&board_id],
                |row| row.get(0)
            ).unwrap_or_default()
        } else {
            String::new()
        };

        let result = db.execute(
            "DELETE FROM whiteboard_elements WHERE id = ?1",
            params![&id]
        );

        if result.is_err() {
            return false;
        }
    }

    // Broadcast via gossip
    if !group_id.is_empty() && !board_id.is_empty() {
        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event: NetworkEvent::WhiteboardElementDeleted {
                id,
                board_id,
            },
        });
    }

    true
}

/// Clear all elements for a whiteboard/board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_clear_whiteboard(board_id: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    if bid.is_empty() {
        return false;
    }

    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        // Get group_id via board -> workspace -> group
        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        let result = db.execute(
            "DELETE FROM whiteboard_elements WHERE board_id = ?1",
            params![&bid]
        );

        if result.is_err() {
            return false;
        }
    }

    // Broadcast via gossip
    if !group_id.is_empty() {
        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event: NetworkEvent::WhiteboardCleared {
                board_id: bid,
            },
        });
    }

    true
}

/// Get element count for a board (useful for BoardGridView badges)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_whiteboard_element_count(board_id: *const c_char) -> i32 {
    let Some(sys) = SYSTEM.get() else {
        return 0;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    let db = sys.db.lock().unwrap();
    db.query_row(
        "SELECT COUNT(*) FROM whiteboard_elements WHERE board_id = ?1",
        params![bid],
        |row| row.get(0)
    ).unwrap_or(0)
}

/// Get all workspace IDs for a group
/// Returns JSON array of workspace ID strings: ["ws1", "ws2", ...]
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_workspaces_for_group(group_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let gid = unsafe { CStr::from_ptr(group_id) }.to_string_lossy().to_string();

    let workspace_ids: Vec<String> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = db.prepare(
            "SELECT id FROM workspaces WHERE group_id = ?1"
        ).unwrap();

        stmt.query_map(params![gid], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    match serde_json::to_string(&workspace_ids) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

// ---------- FFI: File Transfer ----------

/// Upload a file with scope (group/workspace/board)
/// scope_json: {"type": "Group", "group_id": "..."} or {"type": "Workspace", "workspace_id": "..."} etc.
/// Returns JSON: {"success": true, "file_id": "...", "hash": "...", "size": 123} or {"success": false, "error": "..."}
#[unsafe(no_mangle)]
pub extern "C" fn cyan_upload_file(path: *const c_char, scope_json: *const c_char) -> *mut c_char {
    eprintln!("🦀 cyan_upload_file called!");
    let Some(file_path) = (unsafe { cstr_arg(path) }) else {
        eprintln!("🦀 cyan_upload_file: invalid path");
        return CString::new(r#"{"success":false,"error":"Invalid path"}"#).unwrap().into_raw();
    };
    eprintln!("🦀 cyan_upload_file: path = {}", file_path);
    let Some(scope_str) = (unsafe { cstr_arg(scope_json) }) else {
        return CString::new(r#"{"success":false,"error":"Invalid scope"}"#).unwrap().into_raw();
    };
    let Some(sys) = SYSTEM.get() else {
        eprintln!("🦀 cyan_upload_file: invalid scope");
        return CString::new(r#"{"success":false,"error":"System not initialized"}"#).unwrap().into_raw();
    };

    eprintln!("🦀 cyan_upload_file: scope = {}", scope_str);
    // Parse scope
    let scope: serde_json::Value = match serde_json::from_str(&scope_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("🦀failed to parse scope due to : {e:?}");
            return CString::new(format!(r#"{{"success":false,"error":"Invalid scope JSON: {}"}}"#, e))
                .unwrap().into_raw();
        }
    };

    // Read file
    let bytes = match std::fs::read(&file_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("🦀failed to read file path due to : {e:?}");
            return CString::new(format!(r#"{{"success":false,"error":"Failed to read file: {}"}}"#, e))
                .unwrap().into_raw();
        }
    };

    let file_name = Path::new(&file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let size = bytes.len() as u64;
    let now = chrono::Utc::now().timestamp();
    eprintln!("🦀 attempting to store file locally!");
    // Store file locally
    let files_dir = DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("files");
    if let Err(e) = std::fs::create_dir_all(&files_dir) {
        eprintln!("🦀 failed to create dir due to : {e:?}");
        return CString::new(format!(r#"{{"success":false,"error":"Failed to create files dir at {:?}: {}"}}"#, files_dir, e))
            .unwrap().into_raw();
    }
    let local_path = files_dir.join(&hash);
    if let Err(e) = std::fs::write(&local_path, &bytes) {
        eprintln!("🦀 failed to write file due  to : {e:?}");
        return CString::new(format!(r#"{{"success":false,"error":"Failed to store file: {}"}}"#, e))
            .unwrap().into_raw();
    }

    // Determine scope and IDs
    let scope_type = scope["type"].as_str().unwrap_or("");
    let (group_id, workspace_id, board_id): (Option<String>, Option<String>, Option<String>);

    match scope_type {
        "Group" => {
            group_id = scope["group_id"].as_str().map(|s| s.to_string());
            workspace_id = None;
            board_id = None;
        }
        "Workspace" => {
            workspace_id = scope["workspace_id"].as_str().map(|s| s.to_string());
            let db = sys.db.lock().unwrap();
            group_id = workspace_id.as_ref().and_then(|wid| {
                db.query_row(
                    "SELECT group_id FROM workspaces WHERE id = ?1",
                    params![wid],
                    |row| row.get(0),
                ).ok()
            });
            board_id = None;
        }
        "Board" => {
            board_id = scope["board_id"].as_str().map(|s| s.to_string());
            let db = sys.db.lock().unwrap();
            let ids: Option<(String, String)> = board_id.as_ref().and_then(|bid| {
                db.query_row(
                    "SELECT o.workspace_id, w.group_id FROM objects o
                     JOIN workspaces w ON o.workspace_id = w.id
                     WHERE o.id = ?1",
                    params![bid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).ok()
            });
            workspace_id = ids.as_ref().map(|(wid, _)| wid.clone());
            group_id = ids.map(|(_, gid)| gid);
        }
        scope_type => {
            eprintln!("🦀 invalid scope type error  {scope_type:?}");
            return CString::new(r#"{"success":false,"error":"Unknown scope type"}"#)
                .unwrap().into_raw();
        }
    }

    let gid = match &group_id {
        Some(g) => g.clone(),
        None => {
            return CString::new(r#"{"success":false,"error":"Could not determine group"}"#)
                .unwrap().into_raw();
        }
    };

    // Generate file ID
    let file_id = blake3::hash(format!("file:{}:{}:{}", &gid, &file_name, now).as_bytes())
        .to_hex()
        .to_string();

    // Insert into database
    {
        let db = sys.db.lock().unwrap();
        let result = db.execute(
            "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, board_id, type, name, hash, size, source_peer, local_path, created_at)
             VALUES (?1, ?2, ?3, ?4, 'file', ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                file_id,
                group_id,
                workspace_id,
                board_id,
                file_name,
                hash,
                size as i64,
                sys.node_id,
                local_path.to_string_lossy().to_string(),
                now
            ],
        );

        if let Err(e) = result {
            return CString::new(format!(r#"{{"success":false,"error":"DB error: {}"}}"#, e))
                .unwrap().into_raw();
        }
    }

    // Broadcast FileAvailable
    let evt = NetworkEvent::FileAvailable {
        id: file_id.clone(),
        group_id: group_id.clone(),
        workspace_id: workspace_id.clone(),
        board_id: board_id.clone(),
        name: file_name.clone(),
        hash: hash.clone(),
        size,
        source_peer: sys.node_id.clone(),
        created_at: now,
    };

    let _ = sys.network_tx.send(NetworkCommand::Broadcast {
        group_id: gid,
        event: evt,
    });

    // Return success
    let result = serde_json::json!({
        "success": true,
        "file_id": file_id,
        "hash": hash,
        "size": size
    });

    CString::new(result.to_string()).unwrap().into_raw()
}

/// Request download of a file from its source peer
#[unsafe(no_mangle)]
pub extern "C" fn cyan_request_file_download(file_id: *const c_char) -> bool {
    let Some(fid) = (unsafe { cstr_arg(file_id) }) else {
        return false;
    };
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    // Look up file info
    let file_info: Option<(String, String)> = {
        let db = sys.db.lock().unwrap();
        db.query_row(
            "SELECT hash, source_peer FROM objects WHERE id = ?1 AND type = 'file'",
            params![fid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).ok()
    };

    let (hash, source_peer) = match file_info {
        Some((h, sp)) => (h, sp),
        None => return false,
    };

    // Check if already downloaded
    {
        let db = sys.db.lock().unwrap();
        let local_path: Option<String> = db
            .query_row(
                "SELECT local_path FROM objects WHERE id = ?1",
                params![fid],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        if let Some(path) = local_path {
            if Path::new(&path).exists() {
                return true; // Already have it locally
            }
        }
    }

    // Send download request
    let _ = sys.network_tx.send(NetworkCommand::RequestFileDownload {
        file_id: fid,
        hash,
        source_peer,
    });

    true
}

/// Get file status (local/remote)
/// Returns JSON: {"status": "local", "local_path": "..."} or {"status": "remote"}
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_file_status(file_id: *const c_char) -> *mut c_char {
    let Some(fid) = (unsafe { cstr_arg(file_id) }) else {
        return CString::new(r#"{"status":"unknown"}"#).unwrap().into_raw();
    };
    let Some(sys) = SYSTEM.get() else {
        return CString::new(r#"{"status":"unknown"}"#).unwrap().into_raw();
    };

    let db = sys.db.lock().unwrap();
    let local_path: Option<String> = db
        .query_row(
            "SELECT local_path FROM objects WHERE id = ?1 AND type = 'file'",
            params![fid],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    let status = match local_path {
        Some(path) if Path::new(&path).exists() => {
            serde_json::json!({
                "status": "local",
                "local_path": path
            })
        }
        _ => {
            serde_json::json!({
                "status": "remote"
            })
        }
    };

    CString::new(status.to_string()).unwrap().into_raw()
}

/// Get files for a scope
/// scope_json: {"type": "Group", "id": "..."} or {"type": "Workspace", "id": "..."} or {"type": "Board", "id": "..."}
/// Returns JSON array of file objects
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_files(scope_json: *const c_char) -> *mut c_char {
    let Some(scope_str) = (unsafe { cstr_arg(scope_json) }) else {
        return CString::new("[]").unwrap().into_raw();
    };
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let scope: serde_json::Value = match serde_json::from_str(&scope_str) {
        Ok(v) => v,
        Err(_) => return CString::new("[]").unwrap().into_raw(),
    };

    let scope_type = scope["type"].as_str().unwrap_or("");
    let id = scope["id"].as_str()
        .or_else(|| scope["group_id"].as_str())
        .or_else(|| scope["workspace_id"].as_str())
        .or_else(|| scope["board_id"].as_str())
        .unwrap_or("");

    let files: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let query = match scope_type {
            "Group" => {
                "SELECT id, group_id, workspace_id, board_id, name, hash, size, source_peer, local_path, created_at
                 FROM objects WHERE type = 'file' AND group_id = ?1"
            }
            "Workspace" => {
                "SELECT id, group_id, workspace_id, board_id, name, hash, size, source_peer, local_path, created_at
                 FROM objects WHERE type = 'file' AND workspace_id = ?1"
            }
            "Board" => {
                "SELECT id, group_id, workspace_id, board_id, name, hash, size, source_peer, local_path, created_at
                 FROM objects WHERE type = 'file' AND board_id = ?1"
            }
            _ => return CString::new("[]").unwrap().into_raw(),
        };

        let mut stmt = db.prepare(query).unwrap();
        stmt.query_map(params![id], |row| {
            let local_path: Option<String> = row.get(8)?;
            let is_local = local_path
                .as_ref()
                .map(|p| Path::new(p).exists())
                .unwrap_or(false);

            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "group_id": row.get::<_, Option<String>>(1)?,
                "workspace_id": row.get::<_, Option<String>>(2)?,
                "board_id": row.get::<_, Option<String>>(3)?,
                "name": row.get::<_, String>(4)?,
                "hash": row.get::<_, String>(5)?,
                "size": row.get::<_, i64>(6)?,
                "source_peer": row.get::<_, Option<String>>(7)?,
                "local_path": local_path,
                "created_at": row.get::<_, i64>(9)?,
                "is_local": is_local
            }))
        })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    match serde_json::to_string(&files) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Get local file path if file is downloaded
/// Returns null if file is not local
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_file_local_path(file_id: *const c_char) -> *mut c_char {
    let Some(fid) = (unsafe { cstr_arg(file_id) }) else {
        return std::ptr::null_mut();
    };
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let db = sys.db.lock().unwrap();
    let local_path: Option<String> = db
        .query_row(
            "SELECT local_path FROM objects WHERE id = ?1 AND type = 'file'",
            params![fid],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    match local_path {
        Some(path) if Path::new(&path).exists() => {
            CString::new(path).unwrap().into_raw()
        }
        _ => std::ptr::null_mut(),
    }
}

// ---------- FFI: Integration Bridge ----------

/// Handle integration commands via JSON dispatch
/// Swift sends: {"cmd": "start", "scope_type": "workspace", ...}
/// Returns JSON response: {"success": true, ...}
#[unsafe(no_mangle)]
pub extern "C" fn cyan_integration_command(json: *const c_char) -> *mut c_char {
    let Some(cmd_json) = (unsafe { cstr_arg(json) }) else {
        return CString::new(r#"{"success":false,"error":"Invalid JSON"}"#).unwrap().into_raw();
    };
    let Some(sys) = SYSTEM.get() else {
        return CString::new(r#"{"success":false,"error":"System not initialized"}"#).unwrap().into_raw();
    };
    let Some(runtime) = RUNTIME.get() else {
        return CString::new(r#"{"success":false,"error":"Runtime not initialized"}"#).unwrap().into_raw();
    };

    let result = runtime.block_on(async {
        sys.integration_bridge.handle_command(&cmd_json).await
    });

    CString::new(result).unwrap_or_else(|_| {
        CString::new(r#"{"success":false,"error":"CString conversion failed"}"#).unwrap()
    }).into_raw()
}

/// Poll for integration events (uses same buffer as cyan_poll_events)
/// Returns integration events only, filtering out other event types
/// NOW: Uses dedicated integration_event_buffer to avoid race condition with FileTree polling
#[unsafe(no_mangle)]
pub extern "C" fn cyan_poll_integration_events() -> *mut c_char {
    let Some(cyan) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let integration_buffer = cyan.integration_event_buffer.clone();
    let buffer = integration_buffer.lock();
    match buffer {
        Ok(mut buff) => match buff.pop_front() {
            None => std::ptr::null_mut(),
            Some(event_json) => CString::new(event_json).unwrap().into_raw(),
        },
        Err(e) => {
            tracing::error!("failed to lock integration buffer due to {e:?}");
            std::ptr::null_mut()
        }
    }
}

// ==================== AI FFI ====================



// ==================== NOTEBOOK CELLS FFI ====================

/// Load all notebook cells for a board, ordered by cell_order
#[unsafe(no_mangle)]
pub extern "C" fn cyan_load_notebook_cells(board_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    let cells: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = match db.prepare(
            "SELECT id, board_id, cell_type, cell_order, content, output,
                    collapsed, height, metadata_json, created_at, updated_at
             FROM notebook_cells
             WHERE board_id = ?1
             ORDER BY cell_order ASC"
        ) {
            Ok(s) => s,
            Err(_) => return CString::new("[]").unwrap().into_raw(),
        };

        stmt.query_map(params![bid], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "board_id": row.get::<_, String>(1)?,
                "cell_type": row.get::<_, String>(2)?,
                "cell_order": row.get::<_, i32>(3)?,
                "content": row.get::<_, Option<String>>(4)?,
                "output": row.get::<_, Option<String>>(5)?,
                "collapsed": row.get::<_, i32>(6)? != 0,
                "height": row.get::<_, Option<f64>>(7)?,
                "metadata_json": row.get::<_, Option<String>>(8)?,
                "created_at": row.get::<_, i64>(9)?,
                "updated_at": row.get::<_, i64>(10)?
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&cells) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Save (insert or update) a notebook cell
#[unsafe(no_mangle)]
pub extern "C" fn cyan_save_notebook_cell(cell_json: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let json_str = unsafe { CStr::from_ptr(cell_json) }.to_string_lossy().to_string();

    let Ok(cell) = serde_json::from_str::<serde_json::Value>(&json_str) else {
        return false;
    };

    let id = cell["id"].as_str().unwrap_or("").to_string();
    let board_id = cell["board_id"].as_str().unwrap_or("").to_string();
    let cell_type = cell["cell_type"].as_str().unwrap_or("markdown").to_string();
    let cell_order = cell["cell_order"].as_i64().unwrap_or(0) as i32;
    let content = cell["content"].as_str().map(|s| s.to_string());
    let output = cell["output"].as_str().map(|s| s.to_string());
    let collapsed = cell["collapsed"].as_bool().unwrap_or(false);
    let height = cell["height"].as_f64();
    let metadata_json = cell["metadata_json"].as_str().map(|s| s.to_string());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let created_at = cell["created_at"].as_i64().unwrap_or(now);
    let updated_at = now;

    if id.is_empty() || board_id.is_empty() {
        return false;
    }

    let is_new: bool;
    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        // Check if exists
        is_new = db.query_row(
            "SELECT 1 FROM notebook_cells WHERE id = ?1",
            params![&id],
            |_| Ok(())
        ).is_err();

        // Get group_id via board -> workspace -> group
        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&board_id],
            |row| row.get(0)
        ).unwrap_or_default();

        // Insert or replace
        let result = db.execute(
            "INSERT OR REPLACE INTO notebook_cells
             (id, board_id, cell_type, cell_order, content, output, collapsed, height, metadata_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![&id, &board_id, &cell_type, cell_order, &content, &output, collapsed as i32, height, &metadata_json, created_at, updated_at]
        );

        if result.is_err() {
            return false;
        }
    }

    // Broadcast via gossip
    if !group_id.is_empty() {
        let event = if is_new {
            NetworkEvent::NotebookCellAdded {
                id: id.clone(),
                board_id: board_id.clone(),
                cell_type,
                cell_order,
                content,
            }
        } else {
            NetworkEvent::NotebookCellUpdated {
                id: id.clone(),
                board_id: board_id.clone(),
                cell_type,
                cell_order,
                content,
                output,
                collapsed,
                height,
                metadata_json,
            }
        };

        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event,
        });
    }

    true
}

/// Delete a notebook cell
#[unsafe(no_mangle)]
pub extern "C" fn cyan_delete_notebook_cell(cell_id: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let cid = unsafe { CStr::from_ptr(cell_id) }.to_string_lossy().to_string();

    let board_id: String;
    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        // Get board_id and group_id before delete
        let ids: Option<(String, String)> = db.query_row(
            "SELECT c.board_id, w.group_id
             FROM notebook_cells c
             JOIN objects o ON c.board_id = o.id
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE c.id = ?1",
            params![&cid],
            |row| Ok((row.get(0)?, row.get(1)?))
        ).ok();

        let Some((bid, gid)) = ids else {
            return false;
        };
        board_id = bid;
        group_id = gid;

        // Also clear cell_id from any whiteboard_elements belonging to this cell
        let _ = db.execute(
            "UPDATE whiteboard_elements SET cell_id = NULL WHERE cell_id = ?1",
            params![&cid]
        );

        // Delete the cell
        if db.execute("DELETE FROM notebook_cells WHERE id = ?1", params![&cid]).is_err() {
            return false;
        }
    }

    // Broadcast deletion
    if !group_id.is_empty() {
        let event = NetworkEvent::NotebookCellDeleted {
            id: cid.clone(),
            board_id: board_id.clone(),
        };

        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event,
        });
    }

    true
}

/// Reorder cells within a board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_reorder_notebook_cells(board_id: *const c_char, cell_ids_json: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let json_str = unsafe { CStr::from_ptr(cell_ids_json) }.to_string_lossy().to_string();

    let Ok(cell_ids) = serde_json::from_str::<Vec<String>>(&json_str) else {
        return false;
    };

    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        for (idx, cell_id) in cell_ids.iter().enumerate() {
            let _ = db.execute(
                "UPDATE notebook_cells SET cell_order = ?1, updated_at = ?2 WHERE id = ?3 AND board_id = ?4",
                params![idx as i32, now, cell_id, &bid]
            );
        }
    }

    if !group_id.is_empty() {
        let event = NetworkEvent::NotebookCellsReordered {
            board_id: bid.clone(),
            cell_ids: cell_ids.clone(),
        };

        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event,
        });
    }

    true
}

/// Get board mode (canvas, notebook, or notes)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_board_mode(board_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("canvas").unwrap().into_raw();
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    let mode: String = {
        let db = sys.db.lock().unwrap();
        let raw_mode: String = db.query_row(
            "SELECT COALESCE(board_mode, 'canvas') FROM objects WHERE id = ?1",
            params![bid],
            |row| row.get(0)
        ).unwrap_or_else(|_| "canvas".to_string());

        // Normalize legacy 'freeform' to 'canvas'
        if raw_mode == "freeform" {
            "canvas".to_string()
        } else {
            raw_mode
        }
    };

    CString::new(mode).unwrap().into_raw()
}

/// Set board mode (canvas, notebook, or notes)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_board_mode(board_id: *const c_char, mode: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let mode_str = unsafe { CStr::from_ptr(mode) }.to_string_lossy().to_string();

    // Normalize legacy 'freeform' to 'canvas'
    let normalized_mode = if mode_str == "freeform" {
        "canvas".to_string()
    } else {
        mode_str.clone()
    };

    // Validate mode
    if normalized_mode != "canvas" && normalized_mode != "notebook" && normalized_mode != "notes" {
        tracing::warn!("Invalid board mode: {}", normalized_mode);
        return false;
    }

    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        if db.execute(
            "UPDATE objects SET board_mode = ?1 WHERE id = ?2",
            params![&normalized_mode, &bid]
        ).is_err() {
            return false;
        }
    }

    if !group_id.is_empty() {
        let event = NetworkEvent::BoardModeChanged {
            board_id: bid.clone(),
            mode: normalized_mode.clone(),
        };

        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event,
        });
    }

    true
}

/// Load whiteboard elements for a specific cell (canvas cells)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_load_cell_elements(cell_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let cid = unsafe { CStr::from_ptr(cell_id) }.to_string_lossy().to_string();

    let elements: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = match db.prepare(
            "SELECT id, board_id, element_type, x, y, width, height, z_index,
                    style_json, content_json, created_at, updated_at, cell_id
             FROM whiteboard_elements
             WHERE cell_id = ?1
             ORDER BY z_index ASC, created_at ASC"
        ) {
            Ok(s) => s,
            Err(_) => return CString::new("[]").unwrap().into_raw(),
        };

        stmt.query_map(params![cid], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "board_id": row.get::<_, String>(1)?,
                "element_type": row.get::<_, String>(2)?,
                "x": row.get::<_, f64>(3)?,
                "y": row.get::<_, f64>(4)?,
                "width": row.get::<_, f64>(5)?,
                "height": row.get::<_, f64>(6)?,
                "z_index": row.get::<_, i32>(7)?,
                "style_json": row.get::<_, Option<String>>(8)?,
                "content_json": row.get::<_, Option<String>>(9)?,
                "created_at": row.get::<_, i64>(10)?,
                "updated_at": row.get::<_, i64>(11)?,
                "cell_id": row.get::<_, Option<String>>(12)?
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&elements) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

// AI Bridge FFI Exports

fn ai_response_queue() -> &'static Mutex<VecDeque<String>> {
    AI_RESPONSE_QUEUE.get_or_init(|| Mutex::new(VecDeque::with_capacity(16)))
}

/// Handle AI commands via JSON
/// Commands: initialize, image_to_mermaid, ask_analyst, feed_event,
///           set_proactive, register_model, unload_model, infer_model, list_models
/// Returns immediately - poll cyan_poll_ai_response for result
#[unsafe(no_mangle)]
pub extern "C" fn cyan_ai_command(json: *const c_char) -> bool {
    let cmd_json = match unsafe { CStr::from_ptr(json) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => {
            if let Ok(mut q) = ai_response_queue().lock() {
                q.push_back(r#"{"success":false,"error":"Invalid UTF-8"}"#.to_string());
            }
            return false;
        }
    };

    let Some(sys) = SYSTEM.get() else {
        if let Ok(mut q) = ai_response_queue().lock() {
            q.push_back(r#"{"success":false,"error":"System not initialized"}"#.to_string());
        }
        return false;
    };

    let Some(runtime) = RUNTIME.get() else {
        if let Ok(mut q) = ai_response_queue().lock() {
            q.push_back(r#"{"success":false,"error":"Runtime not initialized"}"#.to_string());
        }
        return false;
    };

    // Spawn async task - returns immediately
    let bridge = Arc::clone(&sys.ai_bridge);
    runtime.spawn(async move {
        let result = bridge.handle_command(&cmd_json).await;
        eprintln!("🎯 [cyan_ai_command] Queuing response: {} chars", result.len());
        if let Ok(mut q) = ai_response_queue().lock() {
            q.push_back(result);
        }
    });

    true
}

/// Poll for AI command response
/// Returns JSON string or null if no response pending
#[unsafe(no_mangle)]
pub extern "C" fn cyan_poll_ai_response() -> *mut c_char {
    let Ok(mut queue) = ai_response_queue().lock() else {
        return std::ptr::null_mut();
    };

    match queue.pop_front() {
        Some(response) => {
            eprintln!("📤 [cyan_poll_ai_response] Returning: {} chars", response.len());
            CString::new(response)
                .map(|s| s.into_raw())
                .unwrap_or(std::ptr::null_mut())
        }
        None => std::ptr::null_mut(),
    }
}

/// Poll for AI proactive insights (for ConsoleView)
/// Returns JSON string or null if no insights pending
#[unsafe(no_mangle)]
pub extern "C" fn cyan_poll_ai_insights() -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let Some(runtime) = RUNTIME.get() else {
        return std::ptr::null_mut();
    };

    match runtime.block_on(sys.ai_bridge.poll_insights()) {
        Some(insight) => match serde_json::to_string(&insight) {
            Ok(json) => CString::new(json).unwrap().into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        None => std::ptr::null_mut(),
    }
}
// ============== Board Metadata FFI ==============
// Add this before the final closing brace in lib.rs (after cyan_poll_ai_insights)

/// Get metadata for a single board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_board_metadata(board_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    let metadata: Option<BoardMetadata> = {
        let db = sys.db.lock().unwrap();

        db.query_row(
            "SELECT board_id, labels, rating, view_count, contains_model,
                    contains_skills, board_type, last_accessed
             FROM board_metadata WHERE board_id = ?1",
            params![&bid],
            |row| {
                let labels_json: String = row.get(1)?;
                let skills_json: String = row.get(5)?;

                Ok(BoardMetadata {
                    board_id: row.get(0)?,
                    labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                    rating: row.get(2)?,
                    view_count: row.get(3)?,
                    contains_model: row.get(4)?,
                    contains_skills: serde_json::from_str(&skills_json).unwrap_or_default(),
                    board_type: row.get(6)?,
                    last_accessed: row.get(7)?,
                })
            }
        ).ok()
    };

    let result = metadata.unwrap_or_else(|| {
        let db = sys.db.lock().unwrap();
        let board_type: String = db.query_row(
            "SELECT COALESCE(board_mode, 'canvas') FROM objects WHERE id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_else(|_| "canvas".to_string());

        BoardMetadata {
            board_id: bid,
            board_type,
            ..Default::default()
        }
    });

    match serde_json::to_string(&result) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get metadata for all boards in a scope
/// scope_type: "workspace" | "group" | "all"
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_boards_metadata(scope_type: *const c_char, scope_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let stype = unsafe { CStr::from_ptr(scope_type) }.to_string_lossy().to_string();
    let sid = unsafe { CStr::from_ptr(scope_id) }.to_string_lossy().to_string();

    let results: Vec<BoardMetadata> = {
        let db = sys.db.lock().unwrap();

        let query = match stype.as_str() {
            "workspace" => {
                "SELECT o.id, COALESCE(m.labels, '[]'), COALESCE(m.rating, 0),
                        COALESCE(m.view_count, 0), m.contains_model,
                        COALESCE(m.contains_skills, '[]'), COALESCE(o.board_mode, 'canvas'),
                        COALESCE(m.last_accessed, 0)
                 FROM objects o
                 LEFT JOIN board_metadata m ON o.id = m.board_id
                 WHERE o.workspace_id = ?1 AND o.type = 'whiteboard'
                 ORDER BY COALESCE(m.rating, 0) DESC, o.created_at DESC"
            }
            "group" => {
                "SELECT o.id, COALESCE(m.labels, '[]'), COALESCE(m.rating, 0),
                        COALESCE(m.view_count, 0), m.contains_model,
                        COALESCE(m.contains_skills, '[]'), COALESCE(o.board_mode, 'canvas'),
                        COALESCE(m.last_accessed, 0)
                 FROM objects o
                 JOIN workspaces w ON o.workspace_id = w.id
                 LEFT JOIN board_metadata m ON o.id = m.board_id
                 WHERE w.group_id = ?1 AND o.type = 'whiteboard'
                 ORDER BY COALESCE(m.rating, 0) DESC, o.created_at DESC"
            }
            _ => {
                "SELECT o.id, COALESCE(m.labels, '[]'), COALESCE(m.rating, 0),
                        COALESCE(m.view_count, 0), m.contains_model,
                        COALESCE(m.contains_skills, '[]'), COALESCE(o.board_mode, 'canvas'),
                        COALESCE(m.last_accessed, 0)
                 FROM objects o
                 LEFT JOIN board_metadata m ON o.id = m.board_id
                 WHERE o.type = 'whiteboard'
                 ORDER BY COALESCE(m.rating, 0) DESC, o.created_at DESC
                 LIMIT 100"
            }
        };

        let mut stmt = match db.prepare(query) {
            Ok(s) => s,
            Err(_) => return CString::new("[]").unwrap().into_raw(),
        };

        let param = if stype == "all" { "" } else { &sid };

        stmt.query_map(params![param], |row| {
            let labels_json: String = row.get(1)?;
            let skills_json: String = row.get(5)?;

            Ok(BoardMetadata {
                board_id: row.get(0)?,
                labels: serde_json::from_str(&labels_json).unwrap_or_default(),
                rating: row.get(2)?,
                view_count: row.get(3)?,
                contains_model: row.get(4)?,
                contains_skills: serde_json::from_str(&skills_json).unwrap_or_default(),
                board_type: row.get(6)?,
                last_accessed: row.get(7)?,
            })
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&results) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Get top N boards by rating for a group
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_top_boards(group_id: *const c_char, limit: i32) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let gid = unsafe { CStr::from_ptr(group_id) }.to_string_lossy().to_string();
    let lim = if limit <= 0 { 10 } else { limit.min(50) };

    let results: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = match db.prepare(
            "SELECT o.id, o.name, o.workspace_id, w.name as workspace_name,
                    COALESCE(m.labels, '[]'), COALESCE(m.rating, 0),
                    COALESCE(o.board_mode, 'canvas'), m.contains_model
             FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             LEFT JOIN board_metadata m ON o.id = m.board_id
             WHERE w.group_id = ?1 AND o.type = 'whiteboard'
             ORDER BY COALESCE(m.rating, 0) DESC, COALESCE(m.view_count, 0) DESC
             LIMIT ?2"
        ) {
            Ok(s) => s,
            Err(_) => return CString::new("[]").unwrap().into_raw(),
        };

        stmt.query_map(params![&gid, lim], |row| {
            let labels_json: String = row.get(4)?;
            let labels: Vec<String> = serde_json::from_str(&labels_json).unwrap_or_default();

            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "workspace_id": row.get::<_, String>(2)?,
                "workspace_name": row.get::<_, String>(3)?,
                "labels": labels,
                "rating": row.get::<_, i32>(5)?,
                "board_type": row.get::<_, String>(6)?,
                "contains_model": row.get::<_, Option<String>>(7)?
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&results) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}

/// Set labels for a board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_board_labels(board_id: *const c_char, labels_json: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let json_str = unsafe { CStr::from_ptr(labels_json) }.to_string_lossy().to_string();

    let labels: Vec<String> = match serde_json::from_str(&json_str) {
        Ok(l) => l,
        Err(_) => return false,
    };

    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        // Upsert metadata
        if db.execute(
            "INSERT INTO board_metadata (board_id, labels) VALUES (?1, ?2)
             ON CONFLICT(board_id) DO UPDATE SET labels = ?2",
            params![&bid, &json_str]
        ).is_err() {
            return false;
        }
    }

    // Broadcast
    if !group_id.is_empty() {
        let event = NetworkEvent::BoardLabelsUpdated {
            board_id: bid,
            labels,
        };

        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event,
        });
    }

    true
}

/// Add a single label to a board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_board_label(board_id: *const c_char, label: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let new_label = unsafe { CStr::from_ptr(label) }.to_string_lossy().to_string();

    let group_id: String;
    let updated_labels: Vec<String>;

    {
        let db = sys.db.lock().unwrap();

        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        // Get existing labels
        let existing: String = db.query_row(
            "SELECT COALESCE(labels, '[]') FROM board_metadata WHERE board_id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_else(|_| "[]".to_string());

        let mut labels: Vec<String> = serde_json::from_str(&existing).unwrap_or_default();

        // Add if not exists
        if !labels.contains(&new_label) {
            labels.push(new_label);
        }

        updated_labels = labels.clone();
        let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".to_string());

        // Upsert
        if db.execute(
            "INSERT INTO board_metadata (board_id, labels) VALUES (?1, ?2)
             ON CONFLICT(board_id) DO UPDATE SET labels = ?2",
            params![&bid, &labels_json]
        ).is_err() {
            return false;
        }
    }

    if !group_id.is_empty() {
        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event: NetworkEvent::BoardLabelsUpdated {
                board_id: bid,
                labels: updated_labels,
            },
        });
    }

    true
}

/// Remove a label from a board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_remove_board_label(board_id: *const c_char, label: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let remove_label = unsafe { CStr::from_ptr(label) }.to_string_lossy().to_string();

    let group_id: String;
    let updated_labels: Vec<String>;

    {
        let db = sys.db.lock().unwrap();

        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        let existing: String = db.query_row(
            "SELECT COALESCE(labels, '[]') FROM board_metadata WHERE board_id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_else(|_| "[]".to_string());

        let mut labels: Vec<String> = serde_json::from_str(&existing).unwrap_or_default();
        labels.retain(|l| l != &remove_label);

        updated_labels = labels.clone();
        let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".to_string());

        let _ = db.execute(
            "UPDATE board_metadata SET labels = ?1 WHERE board_id = ?2",
            params![&labels_json, &bid]
        );
    }

    if !group_id.is_empty() {
        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event: NetworkEvent::BoardLabelsUpdated {
                board_id: bid,
                labels: updated_labels,
            },
        });
    }

    true
}

/// Rate a board (0-5)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_rate_board(board_id: *const c_char, rating: i32) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let clamped_rating = rating.clamp(0, 5);

    let group_id: String;

    {
        let db = sys.db.lock().unwrap();

        group_id = db.query_row(
            "SELECT w.group_id FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| row.get(0)
        ).unwrap_or_default();

        if db.execute(
            "INSERT INTO board_metadata (board_id, rating) VALUES (?1, ?2)
             ON CONFLICT(board_id) DO UPDATE SET rating = ?2",
            params![&bid, clamped_rating]
        ).is_err() {
            return false;
        }
    }

    if !group_id.is_empty() {
        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id,
            event: NetworkEvent::BoardRated {
                board_id: bid,
                rating: clamped_rating,
            },
        });
    }

    true
}

/// Increment view count and update last_accessed
#[unsafe(no_mangle)]
pub extern "C" fn cyan_record_board_view(board_id: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let db = sys.db.lock().unwrap();

    db.execute(
        "INSERT INTO board_metadata (board_id, view_count, last_accessed) VALUES (?1, 1, ?2)
         ON CONFLICT(board_id) DO UPDATE SET view_count = view_count + 1, last_accessed = ?2",
        params![&bid, now]
    ).is_ok()
}

/// Set model info for a board (called when notebook has model cell)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_board_model(board_id: *const c_char, model_name: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let model = if model_name.is_null() {
        None
    } else {
        let m = unsafe { CStr::from_ptr(model_name) }.to_string_lossy().to_string();
        if m.is_empty() { None } else { Some(m) }
    };

    let db = sys.db.lock().unwrap();

    db.execute(
        "INSERT INTO board_metadata (board_id, contains_model) VALUES (?1, ?2)
         ON CONFLICT(board_id) DO UPDATE SET contains_model = ?2",
        params![&bid, &model]
    ).is_ok()
}

/// Set skills for a board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_board_skills(board_id: *const c_char, skills_json: *const c_char) -> bool {
    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();
    let json_str = unsafe { CStr::from_ptr(skills_json) }.to_string_lossy().to_string();

    // Validate JSON
    if serde_json::from_str::<Vec<String>>(&json_str).is_err() {
        return false;
    }

    let db = sys.db.lock().unwrap();

    db.execute(
        "INSERT INTO board_metadata (board_id, contains_skills) VALUES (?1, ?2)
         ON CONFLICT(board_id) DO UPDATE SET contains_skills = ?2",
        params![&bid, &json_str]
    ).is_ok()
}

/// Generate deep link URL for a board
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_board_link(board_id: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let bid = unsafe { CStr::from_ptr(board_id) }.to_string_lossy().to_string();

    let link: Option<String> = {
        let db = sys.db.lock().unwrap();

        db.query_row(
            "SELECT w.group_id, o.workspace_id
             FROM objects o
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE o.id = ?1",
            params![&bid],
            |row| {
                let group_id: String = row.get(0)?;
                let workspace_id: String = row.get(1)?;
                Ok(format!("cyan://group/{}/workspace/{}/board/{}", group_id, workspace_id, bid))
            }
        ).ok()
    };

    match link {
        Some(url) => CString::new(url).unwrap().into_raw(),
        None => CString::new(format!("cyan://board/{}", bid)).unwrap().into_raw(),
    }
}

/// Search boards by label
#[unsafe(no_mangle)]
pub extern "C" fn cyan_search_boards_by_label(label: *const c_char) -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return CString::new("[]").unwrap().into_raw();
    };

    let search_label = unsafe { CStr::from_ptr(label) }.to_string_lossy().to_string();
    let pattern = format!("%\"{}%", search_label); // JSON contains pattern

    let results: Vec<serde_json::Value> = {
        let db = sys.db.lock().unwrap();

        let mut stmt = match db.prepare(
            "SELECT o.id, o.name, o.workspace_id, w.name, w.group_id,
                    COALESCE(m.labels, '[]'), COALESCE(m.rating, 0)
             FROM board_metadata m
             JOIN objects o ON m.board_id = o.id
             JOIN workspaces w ON o.workspace_id = w.id
             WHERE m.labels LIKE ?1
             ORDER BY m.rating DESC
             LIMIT 50"
        ) {
            Ok(s) => s,
            Err(_) => return CString::new("[]").unwrap().into_raw(),
        };

        stmt.query_map(params![&pattern], |row| {
            let labels_json: String = row.get(5)?;

            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "workspace_id": row.get::<_, String>(2)?,
                "workspace_name": row.get::<_, String>(3)?,
                "group_id": row.get::<_, String>(4)?,
                "labels": serde_json::from_str::<Vec<String>>(&labels_json).unwrap_or_default(),
                "rating": row.get::<_, i32>(6)?,
                "link": format!("cyan://group/{}/workspace/{}/board/{}",
                    row.get::<_, String>(4)?, row.get::<_, String>(2)?, row.get::<_, String>(0)?)
            }))
        }).unwrap().filter_map(|r| r.ok()).collect()
    };

    match serde_json::to_string(&results) {
        Ok(json) => CString::new(json).unwrap().into_raw(),
        Err(_) => CString::new("[]").unwrap().into_raw(),
    }
}
// ============================================================
// USER PROFILE FFI FUNCTIONS
// ============================================================

/// Get user profile by node_id
/// Returns JSON: {"node_id": "...", "display_name": "...", "avatar_hash": "...", "status": "...", "last_seen": 123}
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_user_profile(node_id: *const c_char) -> *mut c_char {
    let Some(nid) = (unsafe { cstr_arg(node_id) }) else {
        return std::ptr::null_mut();
    };

    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let profile: Option<serde_json::Value> = {
        let db = sys.db.lock().unwrap();
        db.query_row(
            "SELECT node_id, display_name, avatar_hash, status, last_seen, updated_at
             FROM user_profiles WHERE node_id = ?1",
            params![nid],
            |row| {
                Ok(serde_json::json!({
                    "node_id": row.get::<_, String>(0)?,
                    "display_name": row.get::<_, Option<String>>(1)?,
                    "avatar_hash": row.get::<_, Option<String>>(2)?,
                    "status": row.get::<_, String>(3)?,
                    "last_seen": row.get::<_, Option<i64>>(4)?,
                    "updated_at": row.get::<_, Option<i64>>(5)?
                }))
            }
        ).optional().unwrap_or(None)
    };

    match profile {
        Some(p) => CString::new(p.to_string()).unwrap().into_raw(),
        None => {
            let fallback = serde_json::json!({
                "node_id": nid,
                "display_name": null,
                "avatar_hash": null,
                "status": "unknown",
                "last_seen": null
            });
            CString::new(fallback.to_string()).unwrap().into_raw()
        }
    }
}

/// Get multiple user profiles at once (batch lookup)
/// Input: JSON array of node_ids ["id1", "id2", ...]
/// Returns: JSON object {"id1": {...}, "id2": {...}, ...}
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_profiles_batch(node_ids_json: *const c_char) -> *mut c_char {
    let Some(json_str) = (unsafe { cstr_arg(node_ids_json) }) else {
        return CString::new("{}").unwrap().into_raw();
    };

    let node_ids: Vec<String> = match serde_json::from_str(&json_str) {
        Ok(ids) => ids,
        Err(_) => return CString::new("{}").unwrap().into_raw(),
    };

    let Some(sys) = SYSTEM.get() else {
        return CString::new("{}").unwrap().into_raw();
    };

    let mut result = serde_json::Map::new();

    {
        let db = sys.db.lock().unwrap();

        for nid in &node_ids {
            let profile: Option<serde_json::Value> = db.query_row(
                "SELECT node_id, display_name, avatar_hash, status, last_seen
                 FROM user_profiles WHERE node_id = ?1",
                params![nid],
                |row| {
                    Ok(serde_json::json!({
                        "node_id": row.get::<_, String>(0)?,
                        "display_name": row.get::<_, Option<String>>(1)?,
                        "avatar_hash": row.get::<_, Option<String>>(2)?,
                        "status": row.get::<_, String>(3)?,
                        "last_seen": row.get::<_, Option<i64>>(4)?
                    }))
                }
            ).optional().unwrap_or(None);

            if let Some(p) = profile {
                result.insert(nid.clone(), p);
            } else {
                result.insert(nid.clone(), serde_json::json!({
                    "node_id": nid,
                    "display_name": null,
                    "status": "unknown"
                }));
            }
        }
    }

    CString::new(serde_json::Value::Object(result).to_string()).unwrap().into_raw()
}

/// Set my profile (display name and optional avatar)
/// avatar_path can be null - if provided, file is hashed and stored in blobs
/// Broadcasts ProfileUpdated to all groups I'm a member of
#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_my_profile(
    display_name: *const c_char,
    avatar_path: *const c_char
) -> bool {
    let Some(name) = (unsafe { cstr_arg(display_name) }) else {
        return false;
    };

    let avatar_path_opt = unsafe { cstr_arg(avatar_path) };

    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let node_id = sys.node_id.clone();
    let now = chrono::Utc::now().timestamp();

    // Handle avatar if provided
    let avatar_hash: Option<String> = if let Some(path) = avatar_path_opt {
        match std::fs::read(&path) {
            Ok(data) => {
                let hash = blake3::hash(&data).to_hex().to_string();
                if let Some(data_dir) = DATA_DIR.get() {
                    let blobs_dir = data_dir.join("blobs");
                    let _ = std::fs::create_dir_all(&blobs_dir);
                    let blob_path = blobs_dir.join(&hash);
                    let _ = std::fs::write(&blob_path, &data);
                }
                Some(hash)
            }
            Err(_) => None,
        }
    } else {
        let db = sys.db.lock().unwrap();
        db.query_row(
            "SELECT avatar_hash FROM user_profiles WHERE node_id = ?1",
            params![&node_id],
            |row| row.get(0)
        ).ok()
    };

    // Upsert profile
    {
        let db = sys.db.lock().unwrap();
        let _ = db.execute(
            "INSERT INTO user_profiles (node_id, display_name, avatar_hash, status, updated_at)
             VALUES (?1, ?2, ?3, 'online', ?4)
             ON CONFLICT(node_id) DO UPDATE SET
                display_name = excluded.display_name,
                avatar_hash = COALESCE(excluded.avatar_hash, user_profiles.avatar_hash),
                status = 'online',
                updated_at = excluded.updated_at",
            params![&node_id, &name, &avatar_hash, now],
        );
    }

    // Broadcast to all groups
    let group_ids: Vec<String> = {
        let db = sys.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT id FROM groups").unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    let evt = NetworkEvent::ProfileUpdated {
        node_id: node_id.clone(),
        display_name: name.clone(),
        avatar_hash: avatar_hash.clone(),
    };

    for gid in group_ids {
        let _ = sys.network_tx.send(NetworkCommand::Broadcast {
            group_id: gid,
            event: evt.clone(),
        });
    }

    let _ = sys.event_tx.send(SwiftEvent::Network(evt));

    true
}

/// Get my own node ID (the Iroh public key)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_my_node_id() -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    CString::new(sys.node_id.clone()).unwrap().into_raw()
}

/// Get my own profile
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_my_profile() -> *mut c_char {
    let Some(sys) = SYSTEM.get() else {
        return std::ptr::null_mut();
    };

    let node_id = sys.node_id.clone();

    let profile: Option<serde_json::Value> = {
        let db = sys.db.lock().unwrap();
        db.query_row(
            "SELECT node_id, display_name, avatar_hash, status, last_seen, updated_at
             FROM user_profiles WHERE node_id = ?1",
            params![&node_id],
            |row| {
                Ok(serde_json::json!({
                    "node_id": row.get::<_, String>(0)?,
                    "display_name": row.get::<_, Option<String>>(1)?,
                    "avatar_hash": row.get::<_, Option<String>>(2)?,
                    "status": row.get::<_, String>(3)?,
                    "last_seen": row.get::<_, Option<i64>>(4)?,
                    "updated_at": row.get::<_, Option<i64>>(5)?
                }))
            }
        ).optional().unwrap_or(None)
    };

    match profile {
        Some(p) => CString::new(p.to_string()).unwrap().into_raw(),
        None => {
            let fallback = serde_json::json!({
                "node_id": node_id,
                "display_name": null,
                "avatar_hash": null,
                "status": "online",
                "last_seen": null
            });
            CString::new(fallback.to_string()).unwrap().into_raw()
        }
    }
}

/// Update a peer's status (called when gossip events occur)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_peer_status(node_id: *const c_char, status: *const c_char) -> bool {
    let Some(nid) = (unsafe { cstr_arg(node_id) }) else {
        return false;
    };
    let Some(stat) = (unsafe { cstr_arg(status) }) else {
        return false;
    };

    let Some(sys) = SYSTEM.get() else {
        return false;
    };

    let now = chrono::Utc::now().timestamp();

    let db = sys.db.lock().unwrap();
    let result = db.execute(
        "INSERT INTO user_profiles (node_id, status, last_seen, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(node_id) DO UPDATE SET
            status = excluded.status,
            last_seen = excluded.last_seen,
            updated_at = excluded.updated_at",
        params![nid, stat, now],
    );

    result.is_ok()
}