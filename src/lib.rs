// src/lib.rs
#![allow(clippy::too_many_arguments)]
extern crate core;

use crate::NetworkCommand::RequestSnapshot;
use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures::StreamExt;
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey};
use iroh_blobs::store::fs::FsStore as BlobStore;
use iroh_blobs::util::SendStream;
use iroh_gossip::{
    api::{Event as GossipEvent, GossipTopic},
    proto::state::TopicId,
    Gossip,
};
use once_cell::sync::OnceCell;
use rand_chacha::rand_core::SeedableRng;
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
        name: String,
        hash: String,
        size: u64,
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
}

/// Message sent through channel to chat stream writer task
#[derive(Debug, Clone)]
struct DirectChatOutbound {
    workspace_id: String,
    message: String,
    parent_id: Option<String>,
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
}

// ---------- System ----------
pub struct CyanSystem {
    pub node_id: String,
    pub secret_key: SecretKey,
    pub command_tx: mpsc::UnboundedSender<CommandMsg>,
    pub event_tx: mpsc::UnboundedSender<SwiftEvent>,
    pub network_tx: mpsc::UnboundedSender<NetworkCommand>,
    pub event_ffi_buffer: Arc<Mutex<VecDeque<String>>>,
    pub db: Arc<Mutex<Connection>>,
    /// Peers per group, shared with NetworkActor for FFI queries
    pub peers_per_group: Arc<Mutex<HashMap<String, HashSet<PublicKey>>>>,
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
            type TEXT NOT NULL,
            name TEXT,
            hash TEXT,
            size INTEGER,
            data BLOB,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(group_id) REFERENCES groups(id),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(id)
        );
        "#,
    )?;
    Ok(())
}

impl CyanSystem {
    async fn new(db_path: String) -> Result<Self> {
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        );
        let secret_key = SecretKey::generate(&mut rng);
        let node_id = secret_key.public().to_string();

        let db = Connection::open(db_path)?;
        ensure_schema(&db)?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<CommandMsg>();
        let (net_tx, net_rx) = mpsc::unbounded_channel::<NetworkCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwiftEvent>();
        let event_ffi_buffer: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(Default::default()));
        let peers_per_group: Arc<Mutex<HashMap<String, HashSet<PublicKey>>>> = Arc::new(Mutex::new(HashMap::new()));

        let event_ffi_buffer_clone = event_ffi_buffer.clone();
        let node_id_clone = node_id.clone();
        let secret_key_clone = secret_key.clone();
        let peers_per_group_clone = peers_per_group.clone();

        let system = Self {
            node_id: node_id.clone(),
            secret_key: secret_key.clone(),
            event_tx: event_tx.clone(),
            command_tx: cmd_tx,
            network_tx: net_tx.clone(),
            event_ffi_buffer,
            db: Arc::new(Mutex::new(db)),
            peers_per_group,
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

        RUNTIME.get().unwrap().spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match serde_json::to_string(&event) {
                    Ok(event_json) => {
                        event_ffi_buffer_clone.lock().unwrap().push_back(event_json);
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

                                // Notify Swift
                                let _ = event_tx.send(SwiftEvent::Network(NetworkEvent::ChatSent {
                                    id,
                                    workspace_id: workspace_id.clone(),
                                    message: chat_msg.message,
                                    author: peer_id.clone(),
                                    parent_id: chat_msg.parent_id,
                                    timestamp: now,
                                }));
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
                                                name,
                                                hash,
                                                size,
                                                created_at,
                                            } => {
                                                let db = db.lock().unwrap();
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO objects (id, group_id, \
                                             workspace_id, type, name, hash, size, created_at)
                                             VALUES (?1, ?2, ?3, 'file', ?4, ?5, ?6, ?7)",
                                                    params![
                                                id,
                                                group_id,
                                                workspace_id,
                                                name,
                                                hash,
                                                *size as i64,
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
                                        }

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
                                            "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, \
                                             type, name, hash, size, created_at)
                                             VALUES (?1, ?2, NULL, 'file', ?3, ?4, ?5, ?6)",
                                            params![id, group_id, name, hash, size as i64, created_at],
                                        );
                                    }

                                    let topic = self.topics.lock().unwrap().get(&group_id).cloned();
                                    if let Some(topic) = topic {
                                        let evt = NetworkEvent::FileAvailable {
                                            id: id.clone(),
                                            group_id: Some(group_id.clone()),
                                            workspace_id: None,
                                            name: Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string(),
                                            hash,
                                            size,
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
                                            "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, \
                                             type, name, hash, size, created_at)
                                             VALUES (?1, ?2, ?3, 'file', ?4, ?5, ?6, ?7)",
                                            params![
                                                id,
                                                gid,
                                                workspace_id,
                                                name,
                                                hash,
                                                size as i64,
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
                                                name: Path::new(&path).file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string(),
                                                hash,
                                                size,
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
    name: String,
    hash: String,
    size: u64,
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
        let mut stmt = db.prepare("SELECT id, group_id, workspace_id, name, hash, size, created_at FROM objects WHERE type='file' AND group_id=?1")?;
        let group_files = stmt.query_map(params![group_id], |r| {
            Ok(FileDTO {
                id: r.get(0)?,
                group_id: r.get(1)?,
                workspace_id: r.get(2)?,
                name: r.get(3)?,
                hash: r.get(4)?,
                size: r.get::<_, i64>(5)? as u64,
                created_at: r.get(6)?,
            })
        })?;
        all_files.extend(group_files.filter_map(Result::ok));

        // Workspace-level files
        if !workspace_ids.is_empty() {
            let placeholders = workspace_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let query = format!(
                "SELECT id, group_id, workspace_id, name, hash, size, created_at FROM objects WHERE type='file' AND workspace_id IN ({})",
                placeholders
            );
            let mut stmt = db.prepare(&query)?;
            let params: Vec<&dyn rusqlite::ToSql> = workspace_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let ws_files = stmt.query_map(params.as_slice(), |r| {
                Ok(FileDTO {
                    id: r.get(0)?,
                    group_id: r.get(1)?,
                    workspace_id: r.get(2)?,
                    name: r.get(3)?,
                    hash: r.get(4)?,
                    size: r.get::<_, i64>(5)? as u64,
                    created_at: r.get(6)?,
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

    Ok(TreeSnapshotDTO {
        groups: vec![group],
        workspaces,
        whiteboards,
        files,
        chats,
    })
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
            "SELECT id, group_id, workspace_id, name, hash, size, created_at FROM objects \
                 WHERE type='file' ORDER BY name",
        ).unwrap();
        let rows = stmt.query_map([], |r| {
            Ok(FileDTO {
                id: r.get(0)?,
                group_id: r.get(1)?,
                workspace_id: r.get(2)?,
                name: r.get(3)?,
                hash: r.get(4)?,
                size: r.get::<_, i64>(5)? as u64,
                created_at: r.get(6)?,
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

    let snapshot = TreeSnapshotDTO {
        groups,
        workspaces,
        whiteboards,
        files,
        chats,
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
    let Some(s) = (unsafe { cstr_arg(path) }) else {
        return false;
    };
    DATA_DIR.set(PathBuf::from(s)).is_ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_set_discovery_key(key: *const c_char) -> bool {
    let Some(s) = (unsafe { cstr_arg(key) }) else {
        return false;
    };
    DISCOVERY_KEY.set(s).is_ok()
}

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
        let sys = rt.block_on(async { CyanSystem::new(path).await });

        match sys {
            Ok(s) => {
                println!("Cyan initialized with ID: {}", s.node_id);
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