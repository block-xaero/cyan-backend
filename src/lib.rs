// src/lib.rs
#![allow(clippy::too_many_arguments)]

use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_char, CStr, CString},
    path::{Path, PathBuf},
    sync::{Arc, LockResult, Mutex},
};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures::StreamExt;
use iroh::{Endpoint, RelayMode, SecretKey};
use iroh_blobs::store::fs::FsStore as BlobStore;
use iroh_gossip::{
    api::{Event as GossipEvent, GossipTopic},
    proto::state::TopicId,
    Gossip,
};
use once_cell::sync::OnceCell;
use rand_chacha::rand_core::SeedableRng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::{runtime::Runtime, sync::mpsc};

// ---------- Globals ----------
static RUNTIME: OnceCell<Runtime> = OnceCell::new();
static SYSTEM: OnceCell<Arc<CyanSystem>> = OnceCell::new();
static DISCOVERY_KEY: OnceCell<String> = OnceCell::new();
static DATA_DIR: OnceCell<PathBuf> = OnceCell::new();

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
        timestamp: i64,
    },
}

// ---------- Gossip events ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NetworkEvent {
    GroupCreated(Group),
    GroupRenamed {
        id: String,
        name: String,
    },
    WorkspaceCreated(Workspace),
    WorkspaceRenamed {
        id: String,
        name: String,
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
    FileAvailable {
        id: String,
        group_id: Option<String>,
        workspace_id: Option<String>,
        name: String,
        hash: String,
        size: u64,
        created_at: i64,
    },
}

// ---------- Internal commands ----------
#[derive(Debug)]
enum NetworkCommand {
    RequestSnapshot { from_peer: String },
    SendSnapshot { to_peer: String, snapshot: TreeSnapshotDTO },
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
}
// ---------- System ----------
pub struct CyanSystem {
    pub node_id: String,
    pub secret_key: SecretKey,
    // cyan swift sends commands from here
    pub command_tx: mpsc::UnboundedSender<CommandMsg>,
    // tokio channel for network and command actors to write to
    pub event_tx: mpsc::UnboundedSender<SwiftEvent>,
    // tokio channel to receive network events
    pub network_tx: mpsc::UnboundedSender<NetworkCommand>,
    // buffer in command responses, events flowing p2p
    pub event_ffi_buffer: Arc<Mutex<VecDeque<String>>>,
    pub db: Arc<Mutex<Connection>>,
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
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        );
        let secret_key = SecretKey::generate(&mut rng);
        let node_id = secret_key.public().to_string();

        let db = Connection::open(db_path)?;
        ensure_schema(&db)?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<CommandMsg>();
        let (net_tx, net_rx) = mpsc::unbounded_channel::<NetworkCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwiftEvent>();
        let event_ffi_buffer: Arc<Mutex<VecDeque<String>>> =
            Arc::new(Mutex::new(Default::default()));

        // Clone what you need for spawns BEFORE moving into system
        let event_ffi_buffer_clone = event_ffi_buffer.clone();
        let node_id_clone = node_id.clone();
        let secret_key_clone = secret_key.clone();

        let system = Self {
            node_id: node_id.clone(),
            secret_key: secret_key.clone(),
            event_tx,
            command_tx: cmd_tx,
            network_tx: net_tx.clone(),
            event_ffi_buffer,
            db: Arc::new(Mutex::new(db)),
        };

        let db_clone = system.db.clone();
        RUNTIME.get().unwrap().spawn(async move {
            CommandActor {
                db: db_clone,
                rx: cmd_rx,
                network_tx: net_tx, // Move the original
            }
            .run()
            .await;
        });

        let db_clone = system.db.clone();
        RUNTIME.get().unwrap().spawn(async move {
            match NetworkActor::new(node_id_clone, secret_key_clone, db_clone, net_rx).await {
                Ok(mut n) => n.run().await,
                Err(e) => eprintln!("Network actor failed: {e}"),
            }
        });

        RUNTIME.get().unwrap().spawn(async move {
            // event reader and buffer writer.
            while let Some(event) = event_rx.recv().await {
                match serde_json::to_string(&event) {
                    Ok(event_json) => {
                        event_ffi_buffer_clone.lock().unwrap().push_back(event_json);
                    }
                    Err(e) => {
                        eprintln!("Failed to serialize event: {e:?}"); // Don't panic in production
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
}

impl CommandActor {
    async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                CommandMsg::CreateGroup { name, icon, color } => {
                    let id = blake3::hash(format!("{}-{}", name, chrono::Utc::now()).as_bytes())
                        .to_hex()
                        .to_string();
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

                    // Send event to Swift
                    if let Some(system) = SYSTEM.get() {
                        let _ = system
                            .event_tx
                            .send(SwiftEvent::Network(NetworkEvent::GroupCreated(g)));
                    }
                }

                CommandMsg::RenameGroup { id, name } => {
                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute("UPDATE groups SET name=?1 WHERE id=?2", params![
                            name.clone(),
                            id.clone()
                        ])
                        .unwrap_or(0)
                            > 0
                    };

                    if ok {
                        // Send rename event to Swift
                        if let Some(system) = SYSTEM.get() {
                            let _ = system.event_tx.send(SwiftEvent::Network(
                                NetworkEvent::GroupRenamed {
                                    id: id.clone(),
                                    name: name.clone(),
                                },
                            ));
                        }

                        // Broadcast to network
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
                        db.execute("DELETE FROM groups WHERE id=?1", params![id])
                            .unwrap_or(0)
                            > 0
                    };

                    if ok {
                        // Send delete event to Swift
                        if let Some(system) = SYSTEM.get() {
                            let _ = system
                                .event_tx
                                .send(SwiftEvent::GroupDeleted { id: id.clone() });
                        }
                    }
                }

                CommandMsg::CreateWorkspace { group_id, name } => {
                    let id = blake3::hash(format!("ws:{}-{}", &group_id, name).as_bytes())
                        .to_hex()
                        .to_string();
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

                    // Send event to Swift
                    if let Some(system) = SYSTEM.get() {
                        let _ = system
                            .event_tx
                            .send(SwiftEvent::Network(NetworkEvent::WorkspaceCreated(ws)));
                    }
                }

                CommandMsg::RenameWorkspace { id, name } => {
                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute("UPDATE workspaces SET name=?1 WHERE id=?2", params![
                            name.clone(),
                            id.clone()
                        ])
                        .unwrap_or(0)
                            > 0
                    };

                    if ok {
                        // Send rename event to Swift
                        if let Some(system) = SYSTEM.get() {
                            let _ = system.event_tx.send(SwiftEvent::Network(
                                NetworkEvent::WorkspaceRenamed {
                                    id: id.clone(),
                                    name: name.clone(),
                                },
                            ));
                        }
                    }
                }

                CommandMsg::DeleteWorkspace { id } => {
                    let ok = {
                        let group_id: Option<String> = {
                            let db = self.db.lock().unwrap();
                            let mut stmt = db
                                .prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1")
                                .unwrap();
                            stmt.query_row(params![id.clone()], |r| r.get::<_, String>(0))
                                .optional()
                                .unwrap()
                        };
                        let db = self.db.lock().unwrap();
                        let _ =
                            db.execute("DELETE FROM objects WHERE workspace_id=?1", params![id]);
                        let ok = db
                            .execute("DELETE FROM workspaces WHERE id=?1", params![id])
                            .unwrap_or(0)
                            > 0;
                        if ok {
                            if let Some(gid) = group_id {
                                let _ = self
                                    .network_tx
                                    .send(NetworkCommand::JoinGroup { group_id: gid });
                            }
                        }
                        ok
                    };

                    if ok {
                        // Send delete event to Swift
                        if let Some(system) = SYSTEM.get() {
                            let _ = system
                                .event_tx
                                .send(SwiftEvent::WorkspaceDeleted { id: id.clone() });
                        }
                    }
                }

                CommandMsg::CreateBoard { workspace_id, name } => {
                    let id = blake3::hash(format!("wb:{}-{}", &workspace_id, name).as_bytes())
                        .to_hex()
                        .to_string();
                    let now = chrono::Utc::now().timestamp();

                    {
                        let db = self.db.lock().unwrap();
                        let _ = db.execute(
                            "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, \
                             created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
                            params![id.clone(), workspace_id.clone(), name.clone(), now],
                        );
                    }

                    // Send board created event to Swift
                    if let Some(system) = SYSTEM.get() {
                        let _ =
                            system
                                .event_tx
                                .send(SwiftEvent::Network(NetworkEvent::BoardCreated {
                                    id: id.clone(),
                                    workspace_id: workspace_id.clone(),
                                    name: name.clone(),
                                    created_at: now,
                                }));
                    }

                    // Broadcast to network
                    let _ = self.network_tx.send(NetworkCommand::Broadcast {
                        group_id: workspace_id.clone(), // Note: might need to get actual group_id
                        event: NetworkEvent::BoardCreated {
                            id: id.clone(),
                            workspace_id,
                            name,
                            created_at: now,
                        },
                    });
                }

                CommandMsg::RenameBoard { id, name } => {
                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute(
                            "UPDATE objects SET name=?1 WHERE id=?2 AND type='whiteboard'",
                            params![name.clone(), id.clone()],
                        )
                        .unwrap_or(0)
                            > 0
                    };

                    if ok {
                        // Send rename event to Swift
                        if let Some(system) = SYSTEM.get() {
                            let _ = system.event_tx.send(SwiftEvent::Network(
                                NetworkEvent::BoardRenamed {
                                    id: id.clone(),
                                    name: name.clone(),
                                },
                            ));
                        }
                    }
                }

                CommandMsg::DeleteBoard { id } => {
                    let ok = {
                        let db = self.db.lock().unwrap();
                        db.execute(
                            "DELETE FROM objects WHERE id=?1 AND type='whiteboard'",
                            params![id],
                        )
                        .unwrap_or(0)
                            > 0
                    };

                    if ok {
                        // Send delete event to Swift
                        if let Some(system) = SYSTEM.get() {
                            let _ = system
                                .event_tx
                                .send(SwiftEvent::BoardDeleted { id: id.clone() });
                        }
                    }
                }

                CommandMsg::Snapshot {} => {
                    let json = dump_tree_json(&self.db);

                    // Send tree loaded event to Swift
                    if let Some(system) = SYSTEM.get() {
                        let _ = system.event_tx.send(SwiftEvent::TreeLoaded(json.clone()));
                    }
                }

                CommandMsg::SeedDemoIfEmpty => {
                    seed_demo_if_empty(&self.db);

                    // After seeding, send a tree loaded event
                    if let Some(system) = SYSTEM.get() {
                        let json = dump_tree_json(&self.db);
                        let _ = system.event_tx.send(SwiftEvent::TreeLoaded(json));
                    }
                }
            }
        }
    }
}

struct NetworkActor {
    node_id: String,
    db: Arc<Mutex<Connection>>,
    rx: mpsc::UnboundedReceiver<NetworkCommand>,
    endpoint: Endpoint,
    gossip: Gossip,
    blob_store: BlobStore,
    topics: HashMap<String, Arc<tokio::sync::Mutex<GossipTopic>>>,
}

impl NetworkActor {
    async fn new(
        node_id: String,
        secret_key: SecretKey,
        db: Arc<Mutex<Connection>>,
        rx: mpsc::UnboundedReceiver<NetworkCommand>,
    ) -> Result<Self> {
        let builder = Endpoint::builder()
            .secret_key(secret_key.clone())
            .alpns(vec![b"xsp-1.0".to_vec()])
            .relay_mode(RelayMode::Default);

        let endpoint = builder.bind().await?;
        println!("Node ID: {}", endpoint.id());

        let gossip = Gossip::builder().spawn(endpoint.clone());

        // Blob store
        let root = DATA_DIR
            .get()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("blobs");
        let blob_store = BlobStore::load(root).await?;

        // Discovery
        let dkey = DISCOVERY_KEY
            .get()
            .cloned()
            .unwrap_or_else(|| "cyan-dev".to_string());
        let discovery_topic_id = TopicId::from_bytes(
            blake3::hash(format!("cyan/discovery/{}", dkey).as_bytes()).as_bytes()[..32]
                .try_into()
                ?,
        );

        if let Ok(mut disc_reader) = gossip.subscribe(discovery_topic_id, vec![]).await {
            println!("Joined discovery network");

            // Reader task (consumes disc_reader)
            tokio::spawn(async move {
                while let Some(Ok(ev)) = disc_reader.next().await {
                    if let GossipEvent::Received(msg) = ev {
                        if let Ok(txt) = String::from_utf8(msg.content.to_vec()) {
                            if txt.contains("peer_announce") {
                                // seen announce
                            }
                        }
                    }
                }
            });

            // Separate writer handle
            if let Ok(mut disc_writer) = gossip.subscribe(discovery_topic_id, vec![]).await {
                let announce = serde_json::json!({
                    "type":"peer_announce",
                    "id": node_id,
                    "ts": chrono::Utc::now().timestamp(),
                });
                let _ = disc_writer
                    .broadcast(Bytes::from(announce.to_string()))
                    .await;
            }
        }

        // Subscribe existing groups
        {
            let ids: Vec<String> = {
                let db = db.lock().unwrap();
                let mut s = db.prepare("SELECT id FROM groups").unwrap();
                let mut rows = s.query([]).unwrap();
                let mut out = vec![];
                while let Some(r) = rows.next().unwrap() {
                    out.push(r.get::<_, String>(0).unwrap());
                }
                out
            };
            for gid in ids {
                let _ = Self::join_group_internal(&gossip, gid, db.clone()).await;
            }
        }

        Ok(Self {
            node_id,
            db,
            rx,
            endpoint,
            gossip,
            blob_store,
            topics: HashMap::new(),
        })
    }

    async fn join_group_internal(
        gossip: &Gossip,
        group_id: String,
        db: Arc<Mutex<Connection>>,
    ) -> Result<Arc<tokio::sync::Mutex<GossipTopic>>> {
        let topic_id = TopicId::from_bytes(
            blake3::hash(format!("cyan/group/{}", group_id).as_bytes()).as_bytes()[..32]
                .try_into()?,
        );

        let tx_topic = gossip.subscribe(topic_id, vec![]).await?;
        let mut rx_topic = gossip.subscribe(topic_id, vec![]).await?;
        println!("Joined group: {}", group_id);

        let db_for_task = db.clone();
        tokio::spawn(async move {
            while let Some(Ok(ev)) = rx_topic.next().await {
                if let GossipEvent::Received(msg) = ev {
                    if let Ok(evt) = serde_json::from_slice::<NetworkEvent>(&msg.content) {
                        let db = db_for_task.clone();
                        match evt {
                            NetworkEvent::GroupCreated(g) => {
                                let db = db.lock().unwrap();
                                let _ = db.execute(
                                    "INSERT OR IGNORE INTO groups (id, name, icon, color, \
                                     created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                                    params![g.id, g.name, g.icon, g.color, g.created_at],
                                );
                            }
                            NetworkEvent::GroupRenamed { id, name } => {
                                let db = db.lock().unwrap();
                                let _ = db
                                    .execute("UPDATE groups SET name=?1 WHERE id=?2", params![
                                        name, id
                                    ]);
                            }
                            NetworkEvent::WorkspaceCreated(ws) => {
                                let db = db.lock().unwrap();
                                let _ = db.execute(
                                    "INSERT OR IGNORE INTO workspaces (id, group_id, name, \
                                     created_at) VALUES (?1, ?2, ?3, ?4)",
                                    params![ws.id, ws.group_id, ws.name, ws.created_at],
                                );
                            }
                            NetworkEvent::WorkspaceRenamed { id, name } => {
                                let db = db.lock().unwrap();
                                let _ = db
                                    .execute("UPDATE workspaces SET name=?1 WHERE id=?2", params![
                                        name, id
                                    ]);
                            }
                            NetworkEvent::BoardCreated {
                                id,
                                workspace_id,
                                name,
                                created_at,
                            } => {
                                let db = db.lock().unwrap();
                                let _ = db.execute(
                                    "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, \
                                     created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
                                    params![id, workspace_id, name, created_at],
                                );
                            }
                            NetworkEvent::BoardRenamed { id, name } => {
                                let db = db.lock().unwrap();
                                let _ = db.execute(
                                    "UPDATE objects SET name=?1 WHERE id=?2 AND type='whiteboard'",
                                    params![name, id],
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
                                    "INSERT OR IGNORE INTO objects (id, group_id, workspace_id, \
                                     type, name, hash, size, created_at)
                                     VALUES (?1, ?2, ?3, 'file', ?4, ?5, ?6, ?7)",
                                    params![
                                        id,
                                        group_id,
                                        workspace_id,
                                        name,
                                        hash,
                                        size as i64,
                                        created_at
                                    ],
                                );
                            }
                        }
                    }
                }
            }
        });

        Ok(Arc::new(tokio::sync::Mutex::new(tx_topic)))
    }

    async fn run(&mut self) {
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                NetworkCommand::JoinGroup { group_id } =>
                    if !self.topics.contains_key(&group_id) {
                        if let Ok(topic) = Self::join_group_internal(
                            &self.gossip,
                            group_id.clone(),
                            self.db.clone(),
                        )
                        .await
                        {
                            self.topics.insert(group_id, topic);
                        }
                    },

                NetworkCommand::Broadcast { group_id, event } => {
                    if let Some(topic) = self.topics.get(&group_id) {
                        let data = serde_json::to_vec(&event).unwrap();
                        let mut guard = topic.lock().await;
                        let _ = guard.broadcast(Bytes::from(data)).await;
                    }
                }

                NetworkCommand::UploadToGroup { group_id, path } => {
                    if let Ok(bytes) = tokio::fs::read(&path).await {
                        let _ = self.blob_store.add_bytes(bytes.clone()).await;
                        let id = blake3::hash(format!("file:{}:{}", &group_id, &path).as_bytes())
                            .to_hex()
                            .to_string();
                        let hash = blake3::hash(&bytes).to_hex().to_string();
                        let size = bytes.len() as u64;
                        let created_at = chrono::Utc::now().timestamp();

                        {
                            let db = self.db.lock().unwrap();
                            let name = Path::new(&path)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("file");
                            let _ = db.execute(
                                "INSERT OR REPLACE INTO objects
                                 (id, group_id, workspace_id, type, name, hash, size, created_at)
                                 VALUES (?1, ?2, NULL, 'file', ?3, ?4, ?5, ?6)",
                                params![id, group_id, name, hash, size as i64, created_at],
                            );
                        }

                        if let Some(topic) = self.topics.get(&group_id) {
                            let evt = NetworkEvent::FileAvailable {
                                id: id.clone(),
                                group_id: Some(group_id.clone()),
                                workspace_id: None,
                                name: Path::new(&path)
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("file")
                                    .to_string(),
                                hash,
                                size,
                                created_at,
                            };
                            let mut guard = topic.lock().await;
                            let _ = guard
                                .broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap()))
                                .await;
                        }
                    }
                }

                NetworkCommand::UploadToWorkspace { workspace_id, path } => {
                    if let Ok(bytes) = tokio::fs::read(&path).await {
                        let _ = self.blob_store.add_bytes(bytes.clone()).await;

                        let gid: Option<String> = {
                            let db = self.db.lock().unwrap();
                            let mut stmt = db
                                .prepare("SELECT group_id FROM workspaces WHERE id=?1 LIMIT 1")
                                .unwrap();
                            stmt.query_row(params![workspace_id.clone()], |r| r.get::<_, String>(0))
                                .optional()
                                .unwrap()
                        };

                        let id =
                            blake3::hash(format!("file:{}:{}", &workspace_id, &path).as_bytes())
                                .to_hex()
                                .to_string();
                        let hash = blake3::hash(&bytes).to_hex().to_string();
                        let size = bytes.len() as u64;
                        let created_at = chrono::Utc::now().timestamp();

                        {
                            let db = self.db.lock().unwrap();
                            let name = Path::new(&path)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("file");
                            let _ = db.execute(
                                "INSERT OR REPLACE INTO objects
                                 (id, group_id, workspace_id, type, name, hash, size, created_at)
                                 VALUES (?1, ?2, ?3, 'file', ?4, ?5, ?6, ?7)",
                                params![id, gid, workspace_id, name, hash, size as i64, created_at],
                            );
                        }

                        if let Some(group_id) = gid {
                            if let Some(topic) = self.topics.get(&group_id) {
                                let evt = NetworkEvent::FileAvailable {
                                    id: id.clone(),
                                    group_id: Some(group_id.clone()),
                                    workspace_id: Some(workspace_id.clone()),
                                    name: Path::new(&path)
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("file")
                                        .to_string(),
                                    hash,
                                    size,
                                    created_at,
                                };
                                let mut guard = topic.lock().await;
                                let _ = guard
                                    .broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap()))
                                    .await;
                            }
                        }
                    }
                }
                NetworkCommand::RequestSnapshot { .. } => {}
                NetworkCommand::SendSnapshot { .. } => {}
            }
        }
    }
}

// ---------- Snapshot ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GroupRowDTO {
    id: String,
    name: String,
    icon: String,
    color: String,
    created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceRowDTO {
    id: String,
    group_id: String,
    name: String,
    created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WhiteboardRowDTO {
    id: String,
    workspace_id: String,
    name: String,
    created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRowDTO {
    id: String,
    group_id: Option<String>,
    workspace_id: Option<String>,
    name: String,
    hash: String,
    size: u64,
    created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TreeSnapshotDTO {
    groups: Vec<GroupRowDTO>,
    workspaces: Vec<WorkspaceRowDTO>,
    whiteboards: Vec<WhiteboardRowDTO>,
    files: Vec<FileRowDTO>,
}

fn dump_tree_json(db_arc: &Arc<Mutex<Connection>>) -> String {
    let db = db_arc.lock().unwrap();

    let mut groups = vec![];
    {
        let mut s = db
            .prepare("SELECT id, name, icon, color, created_at FROM groups ORDER BY created_at ASC")
            .unwrap();
        let mut rows = s.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            groups.push(GroupRowDTO {
                id: r.get(0).unwrap(),
                name: r.get(1).unwrap(),
                icon: r.get(2).unwrap(),
                color: r.get(3).unwrap(),
                created_at: r.get::<_, i64>(4).unwrap(),
            });
        }
    }

    let mut workspaces = vec![];
    {
        let mut s = db
            .prepare(
                "SELECT id, group_id, name, created_at FROM workspaces ORDER BY created_at ASC",
            )
            .unwrap();
        let mut rows = s.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            workspaces.push(WorkspaceRowDTO {
                id: r.get(0).unwrap(),
                group_id: r.get(1).unwrap(),
                name: r.get(2).unwrap(),
                created_at: r.get::<_, i64>(3).unwrap(),
            });
        }
    }

    let mut whiteboards = vec![];
    {
        let mut s = db
            .prepare(
                "SELECT id, workspace_id, name, created_at FROM objects WHERE type='whiteboard' \
                 ORDER BY created_at ASC",
            )
            .unwrap();
        let mut rows = s.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            whiteboards.push(WhiteboardRowDTO {
                id: r.get(0).unwrap(),
                workspace_id: r.get(1).unwrap(),
                name: r.get(2).unwrap(),
                created_at: r.get::<_, i64>(3).unwrap(),
            });
        }
    }

    let mut files = vec![];
    {
        let mut s = db
            .prepare(
                "SELECT id, group_id, workspace_id, name, hash, size, created_at
                 FROM objects WHERE type='file' ORDER BY created_at ASC",
            )
            .unwrap();
        let mut rows = s.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            files.push(FileRowDTO {
                id: r.get(0).unwrap(),
                group_id: r.get(1).unwrap(),
                workspace_id: r.get(2).unwrap(),
                name: r.get(3).unwrap(),
                hash: r.get(4).unwrap(),
                size: r.get::<_, i64>(5).unwrap() as u64,
                created_at: r.get::<_, i64>(6).unwrap(),
            });
        }
    }

    serde_json::to_string(&TreeSnapshotDTO {
        groups,
        workspaces,
        whiteboards,
        files,
    })
    .unwrap()
}

fn seed_demo_if_empty(db_arc: &Arc<Mutex<Connection>>) {
    let db = db_arc.lock().unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))
        .unwrap_or(0);
    if count > 0 {
        return;
    }
    let now = chrono::Utc::now().timestamp();

    let g1 = blake3::hash(b"DemoGroup").to_hex().to_string();
    let _ = db.execute(
        "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, \
         ?5)",
        params![g1, "Design", "folder.fill", "#00AEEF", now],
    );

    let ws1 = blake3::hash(b"WS:BrandKit").to_hex().to_string();
    let ws2 = blake3::hash(b"WS:MarketingSite").to_hex().to_string();
    let _ = db.execute(
        "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![ws1, g1, "Brand Kit", now],
    );
    let _ = db.execute(
        "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![ws2, g1, "Marketing Site", now + 1],
    );

    let wb1 = blake3::hash(b"WB:LogoExplorations").to_hex().to_string();
    let wb2 = blake3::hash(b"WB:HomepageWireframe").to_hex().to_string();
    let _ = db.execute(
        "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, created_at) VALUES (?1, ?2, \
         'whiteboard', ?3, ?4)",
        params![wb1, ws1, "Logo Explorations", now],
    );
    let _ = db.execute(
        "INSERT OR IGNORE INTO objects (id, workspace_id, type, name, created_at) VALUES (?1, ?2, \
         'whiteboard', ?3, ?4)",
        params![wb2, ws2, "Homepage Wireframe", now + 1],
    );
}

// ---------- FFI helpers ----------
fn cstr(s: String) -> *const c_char {
    CString::new(s).unwrap().into_raw()
}

unsafe fn cstr_arg(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_string())
    }
}

// ---------- FFI: lifecycle ----------
#[no_mangle]
pub extern "C" fn cyan_set_data_dir(path: *const c_char) -> bool {
    let Some(p) = (unsafe { cstr_arg(path) }) else {
        return false;
    };
    let pb = PathBuf::from(p);
    if std::fs::create_dir_all(&pb).is_err() {
        return false;
    }
    DATA_DIR.set(pb.clone()).ok();
    std::env::set_current_dir(&pb).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_set_discovery_key(key: *const c_char) -> bool {
    let Some(s) = (unsafe { cstr_arg(key) }) else {
        return false;
    };
    DISCOVERY_KEY.set(s).is_ok()
}

#[no_mangle]
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("runtime");
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
    })
    .join();
    true
}

// ---------- FFI: snapshot ----------
#[no_mangle]
pub extern "C" fn cyan_dump_tree_json() -> *const c_char {
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return cstr("{}".to_string()),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let _ = sys.command_tx.send(CommandMsg::Snapshot {});
    let json = rx.blocking_recv().unwrap_or_else(|| "{}".to_string());
    cstr(json)
}

#[no_mangle]
pub extern "C" fn cyan_free_c_string(p: *mut c_char) {
    if !p.is_null() {
        unsafe {
            let _ = CString::from_raw(p);
        }
    }
}

// ---------- FFI: groups ----------
#[no_mangle]
pub extern "C" fn cyan_create_group(
    name: *const c_char,
    icon: *const c_char,
    color: *const c_char,
) {
    let Some(name) = (unsafe { cstr_arg(name) }) else {
        return ();
    };
    let icon = (unsafe { cstr_arg(icon) }).unwrap_or_else(|| "folder.fill".into());
    let color = (unsafe { cstr_arg(color) }).unwrap_or_else(|| "#00AEEF".into());

    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys
        .command_tx
        .send(CommandMsg::CreateGroup { name, icon, color });
}

#[no_mangle]
pub extern "C" fn cyan_rename_group(id: *const c_char, new_name: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return ();
    };
    let Some(name) = (unsafe { cstr_arg(new_name) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::RenameGroup {
        id: id.clone(),
        name: name.clone(),
    });
}

#[no_mangle]
pub extern "C" fn cyan_delete_group(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteGroup { id });
}

// ---------- FFI: workspaces ----------
#[no_mangle]
pub extern "C" fn cyan_create_workspace(group_id: *const c_char, name: *const c_char) {
    let Some(gid) = (unsafe { cstr_arg(group_id) }) else {
        return ();
    };
    let Some(name) = (unsafe { cstr_arg(name) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::CreateWorkspace {
        group_id: gid.clone(),
        name: name.clone(),
    });
}

#[no_mangle]
pub extern "C" fn cyan_rename_workspace(id: *const c_char, new_name: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return ();
    };
    let Some(name) = (unsafe { cstr_arg(new_name) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys
        .command_tx
        .send(CommandMsg::RenameWorkspace { id, name });
}

#[no_mangle]
pub extern "C" fn cyan_delete_workspace(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteWorkspace { id });
}

// ---------- FFI: boards ----------
#[no_mangle]
pub extern "C" fn cyan_create_board(workspace_id: *const c_char, name: *const c_char) {
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return ();
    };
    let Some(name) = (unsafe { cstr_arg(name) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::CreateBoard {
        workspace_id: wid,
        name,
    });
}

#[no_mangle]
pub extern "C" fn cyan_rename_board(id: *const c_char, new_name: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return ();
    };
    let Some(name) = (unsafe { cstr_arg(new_name) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::RenameBoard { id, name });
}

#[no_mangle]
pub extern "C" fn cyan_delete_board(id: *const c_char) {
    let Some(id) = (unsafe { cstr_arg(id) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.command_tx.send(CommandMsg::DeleteBoard { id });
}

// ---------- FFI: uploads ----------
#[no_mangle]
pub extern "C" fn cyan_upload_file_to_group(group_id: *const c_char, path: *const c_char) {
    let Some(gid) = (unsafe { cstr_arg(group_id) }) else {
        return ();
    };
    let Some(p) = (unsafe { cstr_arg(path) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.network_tx.send(NetworkCommand::UploadToGroup {
        group_id: gid,
        path: p,
    });
}

#[no_mangle]
pub extern "C" fn cyan_upload_file_to_workspace(workspace_id: *const c_char, path: *const c_char) {
    let Some(wid) = (unsafe { cstr_arg(workspace_id) }) else {
        return ();
    };
    let Some(p) = (unsafe { cstr_arg(path) }) else {
        return ();
    };
    let sys = match SYSTEM.get() {
        Some(s) => s.clone(),
        None => return (),
    };
    let _ = sys.network_tx.send(NetworkCommand::UploadToWorkspace {
        workspace_id: wid,
        path: p,
    });
}

// ---------- FFI: seed ----------
#[no_mangle]
pub extern "C" fn cyan_seed_demo_if_empty() {
    if let Some(sys) = SYSTEM.get() {
        let _ = sys.command_tx.send(CommandMsg::SeedDemoIfEmpty);
    }
}

use blake3;
use futures::future::Lazy;
use tokio::sync::mpsc::error::SendError;
// You already depend on these crates:
use uuid::Uuid;

// ---------- Node ID storage ----------
static NODE_ID: OnceCell<String> = OnceCell::new();

// Convert Rust String -> *const c_char (owned; caller must free via cyan_free_c_string)
fn to_c_string(s: String) -> *const c_char {
    CString::new(s).unwrap().into_raw()
}

fn data_dir() -> PathBuf {
    // We rely on your existing cyan_set_data_dir() which calls set_current_dir().
    // If not set, this will be the process CWD.
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn node_id_file() -> PathBuf {
    data_dir().join("cyan.node_id")
}

fn load_node_id_from_disk() -> Option<String> {
    let path = node_id_file();
    if let Ok(s) = std::fs::read_to_string(&path) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn save_node_id_to_disk(id: &str) {
    let path = node_id_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, id.as_bytes());
}

fn generate_node_id() -> String {
    // Stable enough for now: random v4 UUID -> blake3 hex
    blake3::hash(Uuid::new_v4().as_bytes()).to_hex().to_string()
}

fn compute_or_load_node_id() -> String {
    // 1) env override (handy for testing)
    if let Ok(v) = std::env::var("CYAN_NODE_ID") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    // 2) persisted file
    if let Some(s) = load_node_id_from_disk() {
        return s;
    }
    // 3) generate & persist
    let s = generate_node_id();
    save_node_id_to_disk(&s);
    s
}

/// Returns the node/Xaero id as a newly allocated C string.
/// Call `cyan_free_c_string` to free it on the Swift side.
#[no_mangle]
pub extern "C" fn cyan_get_xaero_id() -> *const c_char {
    let id = NODE_ID.get_or_init(|| compute_or_load_node_id());
    to_c_string(id.clone())
}

/// Optionally set the node/Xaero id from Swift (persists to disk too).
#[no_mangle]
pub extern "C" fn cyan_set_xaero_id(id: *const c_char) -> bool {
    if id.is_null() {
        return false;
    }
    let s = unsafe { CStr::from_ptr(id) }
        .to_str()
        .ok()
        .unwrap()
        .to_string();

    // If NODE_ID was not set, set it; else overwrite the file anyway.
    let _ = NODE_ID.set(s.clone());
    save_node_id_to_disk(&s);
    true
}

#[no_mangle]
pub extern "C" fn cyan_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

#[no_mangle]
pub extern "C" fn cyan_poll_events(component: *const c_char) -> *mut c_char {
    let component = unsafe { CStr::from_ptr(component).to_string_lossy() };
    let cyan  = SYSTEM.get().unwrap();
    let event_ffi_buffer = cyan.event_ffi_buffer.clone();
    let buffer = event_ffi_buffer.lock();
    match buffer {
        Ok(mut buff) => {
            match buff.pop_front() {
                None => {
                    tracing::info!("no event to poll!");
                    std::ptr::null_mut()
                }
                Some(event_json) => {
                    CString::new(event_json).unwrap().into_raw()
                }
            }
        }
        Err(e) => {
            tracing::error!("failed to lock due to {e:?}");
            return std::ptr::null_mut();
        }
    }
}

#[no_mangle]
pub extern "C" fn cyan_send_command(component: *const c_char, json: *const c_char) -> bool {
    let component = unsafe { CStr::from_ptr(component).to_string_lossy().to_string() };
    let json_str = unsafe { CStr::from_ptr(json).to_string_lossy().to_string() };
    match serde_json::from_str::<CommandMsg>(&json_str) {
        Ok(command) => match SYSTEM.get().unwrap().command_tx.send(command) {
            Ok(_) => true,
            Err(e) => {
                panic!("failed to send command: {e:?}");
            }
        },
        Err(e) => {
            // this is for debug /mvp otherwise we would have to
            // something else to track of these overflows or fill an error buffer.
            panic!("failed to send command: {e:?}");
        }
    }
}
