#![allow(clippy::single_match)]

use anyhow::Result;
use bytes::Bytes;
use futures::StreamExt;
use once_cell::sync::OnceCell;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use iroh::discovery::pkarr::PkarrPublisher;
use iroh::{Endpoint, RelayMode, SecretKey};
use iroh_blobs::store::fs::FsStore as BlobStore;
use iroh_gossip::{
    net::Gossip,
    proto::TopicId,
};

use rand_chacha::rand_core::SeedableRng;

// =================== Globals ===================

static RUNTIME: OnceCell<Runtime> = OnceCell::new();
static SYSTEM: OnceCell<Arc<CyanSystem>> = OnceCell::new();
static DISCOVERY_KEY: OnceCell<String> = OnceCell::new();
static DATA_DIR: OnceCell<PathBuf> = OnceCell::new();

// =================== Data Types ===================

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
#[serde(tag = "type")]
pub enum Object {
    Whiteboard {
        id: String,
        workspace_id: String,
        name: String,
        data: Vec<u8>,
        created_at: i64,
    },
    File {
        id: String,
        workspace_id: String,
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

// =================== Network Events ===================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkEvent {
    GroupCreated(Group),
    WorkspaceCreated(Workspace),
    ObjectCreated(Object),
    ObjectUpdated(Object),
    FileAvailable { hash: String, name: String, size: u64 },
}

// =================== Commands ===================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    CreateGroup { name: String, icon: String, color: String },
    CreateWorkspace { group_id: String, name: String },
    CreateObject { workspace_id: String, object: Object },
    UpdateObject { object: Object },
    UploadFileForWorkspace { workspace_id: String, path: String },
    UploadFileForBoard { board_id: String, path: String },
    UploadFileForGroup { group_id: String, path: String },
}

// =================== FFI envelope ===================

#[repr(C)]
pub struct FFICommand {
    pub cmd_type: u32,
    pub data: Vec<u8>,
    pub callback_id: u64,
}

// =================== Internal NetworkCommand ===================

#[derive(Debug)]
enum NetworkCommand {
    BroadcastEvent { group_id: String, event: NetworkEvent },
    JoinGroup { group_id: String },
    UploadFileForWorkspace { workspace_id: String, path: String },
    UploadFileForBoard { board_id: String, path: String },
    UploadFileForGroup { group_id: String, path: String },
}

// =================== System ===================

pub struct CyanSystem {
    pub xaero_id: String,
    secret_key: SecretKey,
    command_tx: mpsc::UnboundedSender<FFICommand>,
    network_tx: mpsc::UnboundedSender<NetworkCommand>,
    db: Arc<Mutex<Connection>>,
    blobs_dir: PathBuf,
}

impl CyanSystem {
    async fn new() -> Result<Self> {
        // Identity
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        let secret_key = SecretKey::generate(&mut rng);
        let xaero_id = secret_key.public().to_string();

        // Data dir
        let data_dir = DATA_DIR
            .get()
            .cloned()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        std::fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("cyan.db");
        let blobs_dir = data_dir.join("blobs");
        std::fs::create_dir_all(&blobs_dir)?;

        // DB
        let db = Connection::open(&db_path)?;
        Self::init_db(&db)?;

        // Channels
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (network_tx, network_rx) = mpsc::unbounded_channel();

        let system = Self {
            xaero_id: xaero_id.clone(),
            secret_key: secret_key.clone(),
            command_tx,
            network_tx: network_tx.clone(),
            db: Arc::new(Mutex::new(db)),
            blobs_dir,
        };

        // Command actor
        {
            let db = system.db.clone();
            let net_tx = network_tx.clone();
            RUNTIME.get().unwrap().spawn(async move {
                let actor = CommandActor {
                    rx: command_rx,
                    db,
                    network_tx: net_tx,
                };
                actor.run().await;
            });
        }

        // Network actor
        {
            let db = system.db.clone();
            let xaero = xaero_id.clone();
            let data_dir_clone = DATA_DIR
                .get()
                .cloned()
                .unwrap_or_else(|| PathBuf::from("."));
            RUNTIME.get().unwrap().spawn(async move {
                let network = NetworkActor::new(
                    xaero,
                    secret_key,
                    db,
                    network_rx,
                    data_dir_clone,
                )
                    .await
                    .expect("Failed to create network actor");
                network.run().await;
            });
        }

        Ok(system)
    }

    fn init_db(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;

            CREATE TABLE IF NOT EXISTS groups (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                icon TEXT,
                color TEXT,
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
                workspace_id TEXT NOT NULL,
                type TEXT NOT NULL,
                data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(id)
            );
            "#,
        )?;
        Ok(())
    }
}

// =================== Command Actor ===================

struct CommandActor {
    rx: mpsc::UnboundedReceiver<FFICommand>,
    db: Arc<Mutex<Connection>>,
    network_tx: mpsc::UnboundedSender<NetworkCommand>,
}

impl CommandActor {
    async fn run(mut self) {
        while let Some(ffi_cmd) = self.rx.recv().await {
            if let Ok(cmd) = serde_json::from_slice::<Command>(&ffi_cmd.data) {
                if let Err(e) = self.process_command(cmd).await {
                    eprintln!("Command error: {e}");
                }
            }
        }
    }

    async fn process_command(&mut self, cmd: Command) -> Result<()> {
        let db = self.db.lock().unwrap();

        match cmd {
            Command::CreateGroup { name, icon, color } => {
                let id = blake3::hash(format!("{}-{}", name, chrono::Utc::now()).as_bytes())
                    .to_hex()
                    .to_string();
                let now = chrono::Utc::now().timestamp();

                let group = Group {
                    id: id.clone(),
                    name: name.clone(),
                    icon: icon.clone(),
                    color: color.clone(),
                    created_at: now,
                };

                db.execute(
                    "INSERT INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![&group.id, &group.name, &group.icon, &group.color, group.created_at],
                )?;

                let _ = self.network_tx.send(NetworkCommand::JoinGroup {
                    group_id: id.clone(),
                });
                let _ = self.network_tx.send(NetworkCommand::BroadcastEvent {
                    group_id: id,
                    event: NetworkEvent::GroupCreated(group),
                });
            }

            Command::CreateWorkspace { group_id, name } => {
                let id = blake3::hash(format!("{}-{}", name, chrono::Utc::now()).as_bytes())
                    .to_hex()
                    .to_string();
                let now = chrono::Utc::now().timestamp();

                let ws = Workspace {
                    id: id.clone(),
                    group_id: group_id.clone(),
                    name: name.clone(),
                    created_at: now,
                };

                db.execute(
                    "INSERT INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![&ws.id, &ws.group_id, &ws.name, ws.created_at],
                )?;

                let _ = self.network_tx.send(NetworkCommand::JoinGroup {
                    group_id: group_id.clone(),
                });
                let _ = self.network_tx.send(NetworkCommand::BroadcastEvent {
                    group_id,
                    event: NetworkEvent::WorkspaceCreated(ws),
                });
            }

            Command::CreateObject { workspace_id, object } => {
                let now = chrono::Utc::now().timestamp();
                let data = serde_json::to_vec(&object)?;
                let (id, ws_id, obj_type, created_at) = match &object {
                    Object::Whiteboard { id, workspace_id, created_at, .. } => {
                        (id.clone(), workspace_id.clone(), "whiteboard".to_string(), *created_at)
                    }
                    Object::File { id, workspace_id, created_at, .. } => {
                        (id.clone(), workspace_id.clone(), "file".to_string(), *created_at)
                    }
                    Object::Chat { id, workspace_id, timestamp, .. } => {
                        (id.clone(), workspace_id.clone(), "chat".to_string(), *timestamp)
                    }
                };

                db.execute(
                    "INSERT INTO objects (id, workspace_id, type, data, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![&id, &ws_id, &obj_type, &data, created_at.max(now)],
                )?;

                // Broadcast on the group's topic
                if let Some(gid) = query_group_id_for_workspace(&db, &workspace_id) {
                    let _ = self.network_tx.send(NetworkCommand::BroadcastEvent {
                        group_id: gid,
                        event: NetworkEvent::ObjectCreated(object),
                    });
                }
            }

            Command::UploadFileForWorkspace { workspace_id, path } => {
                let _ = self.network_tx.send(NetworkCommand::UploadFileForWorkspace {
                    workspace_id,
                    path,
                });
            }

            Command::UploadFileForBoard { board_id, path } => {
                let _ = self
                    .network_tx
                    .send(NetworkCommand::UploadFileForBoard { board_id, path });
            }

            Command::UploadFileForGroup { group_id, path } => {
                let _ = self
                    .network_tx
                    .send(NetworkCommand::UploadFileForGroup { group_id, path });
            }

            Command::UpdateObject { .. } => {
                // no-op for now
            }
        }

        Ok(())
    }
}

// =================== Network Actor ===================

struct NetworkActor {
    xaero_id: String,
    db: Arc<Mutex<Connection>>,
    rx: mpsc::UnboundedReceiver<NetworkCommand>,
    endpoint: Endpoint,
    gossip: Gossip,
    blob_store: BlobStore,
    groups: HashMap<String, TopicId>,
    blobs_dir: PathBuf,
}

impl NetworkActor {
    async fn new(
        xaero_id: String,
        secret_key: SecretKey,
        db: Arc<Mutex<Connection>>,
        rx: mpsc::UnboundedReceiver<NetworkCommand>,
        data_dir: PathBuf,
    ) -> Result<Self> {
        // discovery key
        let discovery_key = DISCOVERY_KEY
            .get()
            .cloned()
            .unwrap_or_else(|| blake3::hash(xaero_id.as_bytes()).to_hex().to_string());

        println!("Discovery Key: {discovery_key}");

        // endpoint
        let builder = Endpoint::builder()
            .secret_key(secret_key.clone())
            .alpns(vec![b"xsp-1.0".to_vec()])
            .relay_mode(RelayMode::Default);

        let builder = {
            let pkarr = PkarrPublisher::n0_dns().build(secret_key.clone());
            builder.discovery(pkarr)
        };

        let endpoint = builder.bind().await?;
        println!("Node ID: {}", endpoint.id());

        let gossip = Gossip::builder().spawn(endpoint.clone());

        // blob store in data_dir/blobs
        let blobs_dir = data_dir.join("blobs");
        let blob_store = BlobStore::load(&blobs_dir).await?;

        // join discovery topic & announce
        let disc_topic_id = topic_for(&format!("cyan/discovery/{discovery_key}"));
        let mut disc = gossip.subscribe(disc_topic_id, vec![]).await?;
        let announce = serde_json::json!({
            "type": "peer_announce",
            "id": xaero_id,
            "timestamp": chrono::Utc::now().timestamp()
        });
        disc.broadcast(Bytes::from(announce.to_string())).await?;
        println!("Joined discovery network");

        tokio::spawn(async move {
            let mut reader = disc;
            while let Some(Ok(ev)) = reader.next().await {
                if let iroh_gossip::api::Event::Received(msg) = ev {
                    if let Ok(json) =
                        serde_json::from_slice::<serde_json::Value>(&msg.content)
                    {
                        if json.get("type") == Some(&serde_json::Value::String("peer_announce".into())) {
                            if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                                println!("Discovered peer: {id}");
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            xaero_id,
            db,
            rx,
            endpoint,
            gossip,
            blob_store,
            groups: HashMap::new(),
            blobs_dir: data_dir.join("blobs"),
        })
    }

    async fn run(mut self) {
        // Subscribe to existing groups
        if let Err(e) = self.subscribe_to_existing_groups().await {
            eprintln!("Failed to subscribe to existing groups: {e}");
        }

        loop {
            tokio::select! {
                Some(cmd) = self.rx.recv() => {
                    match cmd {
                        NetworkCommand::BroadcastEvent { group_id, event } => {
                            let topic_id = self.get_or_join_group(&group_id).await;
                            if let Ok(mut topic) = self.gossip.subscribe(topic_id, vec![]).await {
                                let data = serde_json::to_vec(&event).unwrap();
                                if let Err(e) = topic.broadcast(Bytes::from(data)).await {
                                    eprintln!("Failed to broadcast: {e}");
                                }
                            }
                        }
                        NetworkCommand::JoinGroup { group_id } => {
                            self.get_or_join_group(&group_id).await;
                        }
                        NetworkCommand::UploadFileForWorkspace { workspace_id, path } => {
                            self.handle_upload_for_workspace(&workspace_id, &path).await;
                        }
                        NetworkCommand::UploadFileForBoard { board_id, path } => {
                            self.handle_upload_for_board(&board_id, &path).await;
                        }
                        NetworkCommand::UploadFileForGroup { group_id, path } => {
                            self.handle_upload_for_group(&group_id, &path).await;
                        }
                    }
                }
            }
        }
    }

    async fn handle_upload_for_group(&mut self, group_id: &str, path: &str) {
        match tokio::fs::read(path).await {
            Ok(data) => {
                let tag = self.blob_store.add_bytes(data.clone()).await.unwrap();
                let hash_str = format!("{}", tag.hash);
                println!(
                    "File uploaded for group {group_id}: {hash_str} ({} bytes)",
                    data.len()
                );
                // Broadcast availability
                let evt = NetworkEvent::FileAvailable {
                    hash: hash_str,
                    name: Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("file")
                        .to_string(),
                    size: data.len() as u64,
                };
                let topic_id = self.get_or_join_group(group_id).await;
                if let Ok(mut topic) = self.gossip.subscribe(topic_id, vec![]).await {
                    if let Err(e) = topic
                        .broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap()))
                        .await
                    {
                        eprintln!("Broadcast err: {e}");
                    }
                }
            }
            Err(e) => eprintln!("Read error {path}: {e}"),
        }
    }

    async fn handle_upload_for_workspace(&mut self, workspace_id: &str, path: &str) {
        let group_id = {
            let db = self.db.lock().unwrap();
            query_group_id_for_workspace(&db, workspace_id)
        };

        match tokio::fs::read(path).await {
            Ok(data) => {
                let tag = self.blob_store.add_bytes(data.clone()).await.unwrap();
                let hash_str = format!("{}", tag.hash);
                println!(
                    "File uploaded for workspace {workspace_id}: {hash_str} ({} bytes)",
                    data.len()
                );

                let evt = NetworkEvent::FileAvailable {
                    hash: hash_str,
                    name: Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("file")
                        .to_string(),
                    size: data.len() as u64,
                };
                if let Some(gid) = group_id {
                    let topic_id = self.get_or_join_group(&gid).await;
                    if let Ok(mut topic) = self.gossip.subscribe(topic_id, vec![]).await {
                        if let Err(e) = topic
                            .broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap()))
                            .await
                        {
                            eprintln!("Broadcast err: {e}");
                        }
                    }
                }
            }
            Err(e) => eprintln!("Read error {path}: {e}"),
        }
    }

    async fn handle_upload_for_board(&mut self, board_id: &str, path: &str) {
        let (workspace_id, group_id) = {
            let db = self.db.lock().unwrap();
            let ws = query_workspace_id_for_board(&db, board_id).unwrap_or_default();
            let gid = if ws.is_empty() {
                String::new()
            } else {
                query_group_id_for_workspace(&db, &ws).unwrap_or_default()
            };
            (ws, gid)
        };

        match tokio::fs::read(path).await {
            Ok(data) => {
                let tag = self.blob_store.add_bytes(data.clone()).await.unwrap();
                let hash_str = format!("{}", tag.hash);
                println!(
                    "File uploaded for board {board_id} (ws={workspace_id}): {hash_str} ({} bytes)",
                    data.len()
                );
                let evt = NetworkEvent::FileAvailable {
                    hash: hash_str,
                    name: Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("file")
                        .to_string(),
                    size: data.len() as u64,
                };
                if !group_id.is_empty() {
                    let topic_id = self.get_or_join_group(&group_id).await;
                    if let Ok(mut topic) = self.gossip.subscribe(topic_id, vec![]).await {
                        if let Err(e) = topic
                            .broadcast(Bytes::from(serde_json::to_vec(&evt).unwrap()))
                            .await
                        {
                            eprintln!("Broadcast err: {e}");
                        }
                    }
                }
            }
            Err(e) => eprintln!("Read error {path}: {e}"),
        }
    }

    async fn subscribe_to_existing_groups(&mut self) -> Result<()> {
        let group_ids = {
            let db = self.db.lock().unwrap();
            let mut stmt = db.prepare("SELECT id FROM groups")?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        for gid in group_ids {
            println!("Subscribing to existing group: {gid}");
            self.get_or_join_group(&gid).await;
        }
        Ok(())
    }

    async fn get_or_join_group(&mut self, group_id: &str) -> TopicId {
        if let Some(topic_id) = self.groups.get(group_id) {
            return *topic_id;
        }

        let topic_id = topic_for(&format!("cyan/group/{group_id}"));

        // Subscribe for receiving and spawn handler
        if let Ok(mut sub) = self.gossip.subscribe(topic_id, vec![]).await {
            println!("Joined group: {group_id}");
            self.groups.insert(group_id.to_string(), topic_id);

            let db = self.db.clone();
            let gid = group_id.to_string();
            tokio::spawn(async move {
                let mut reader = sub;
                while let Some(Ok(event)) = reader.next().await {
                    if let iroh_gossip::api::Event::Received(msg) = event {
                        if let Ok(net_event) =
                            serde_json::from_slice::<NetworkEvent>(&msg.content)
                        {
                            Self::handle_network_event(db.clone(), gid.clone(), net_event).await;
                        }
                    }
                }
            });
        }

        topic_id
    }

    async fn handle_network_event(
        db: Arc<Mutex<Connection>>,
        _group_id: String,
        event: NetworkEvent,
    ) {
        let db = db.lock().unwrap();
        match event {
            NetworkEvent::GroupCreated(group) => {
                let _ = db.execute(
                    "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![group.id, group.name, group.icon, group.color, group.created_at],
                );
            }
            NetworkEvent::WorkspaceCreated(ws) => {
                let _ = db.execute(
                    "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![ws.id, ws.group_id, ws.name, ws.created_at],
                );
            }
            NetworkEvent::ObjectCreated(obj) => {
                let data = serde_json::to_vec(&obj).unwrap_or_default();
                let (id, workspace_id, typ, created_at) = match &obj {
                    Object::Whiteboard { id, workspace_id, created_at, .. } => (id.clone(), workspace_id.clone(), "whiteboard".to_string(), *created_at),
                    Object::File { id, workspace_id, created_at, .. } => (id.clone(), workspace_id.clone(), "file".to_string(), *created_at),
                    Object::Chat { id, workspace_id, timestamp, .. } => (id.clone(), workspace_id.clone(), "chat".to_string(), *timestamp),
                };
                let _ = db.execute(
                    "INSERT OR IGNORE INTO objects (id, workspace_id, type, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, workspace_id, typ, data, created_at],
                );
            }
            _ => {}
        }
    }
}

// =================== Query helpers ===================

fn query_group_id_for_workspace(db: &Connection, ws_id: &str) -> Option<String> {
    let mut stmt = db.prepare("SELECT group_id FROM workspaces WHERE id = ?1 LIMIT 1").ok()?;
    let mut rows = stmt.query(params![ws_id]).ok()?;
    match rows.next() {
        Ok(Some(row)) => row.get::<_, String>(0).ok(),
        _ => None,
    }
}

fn query_workspace_id_for_board(db: &Connection, board_id: &str) -> Option<String> {
    let mut stmt = db
        .prepare("SELECT workspace_id FROM objects WHERE id = ?1 AND type='whiteboard' LIMIT 1")
        .ok()?;
    let mut rows = stmt.query(params![board_id]).ok()?;
    match rows.next() {
        Ok(Some(row)) => row.get::<_, String>(0).ok(),
        _ => None,
    }
}

// =================== Helpers ===================

fn topic_for(s: &str) -> TopicId {
    let mut arr = [0u8; 32];
    let h = blake3::hash(s.as_bytes());
    arr.copy_from_slice(&h.as_bytes()[..32]);
    TopicId::from_bytes(arr)
}

// =================== FFI ===================

#[no_mangle]
pub extern "C" fn cyan_set_data_dir(path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    let c = unsafe { CStr::from_ptr(path) };
    let Ok(p) = c.to_str() else { return false };
    let dir = PathBuf::from(p);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("create_dir_all error: {e}");
        return false;
    }
    if let Err(e) = std::env::set_current_dir(&dir) {
        eprintln!("set_current_dir error: {e}");
        return false;
    }
    let _ = DATA_DIR.set(dir);
    true
}

#[no_mangle]
pub extern "C" fn cyan_set_discovery_key(key: *const c_char) -> bool {
    if key.is_null() {
        return false;
    }
    let c = unsafe { CStr::from_ptr(key) };
    let Ok(s) = c.to_str() else { return false };
    DISCOVERY_KEY.set(s.to_string()).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_init() -> bool {
    if SYSTEM.get().is_some() {
        return true;
    }

    let result = std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("Failed to create runtime");

        let _ = RUNTIME.set(runtime);

        let rt = RUNTIME.get().unwrap();
        let system = rt.block_on(async { CyanSystem::new().await });

        match system {
            Ok(s) => {
                println!("Cyan initialized with ID: {}", s.xaero_id);
                SYSTEM.set(Arc::new(s)).is_ok()
            }
            Err(e) => {
                eprintln!("Failed to init: {e}");
                false
            }
        }
    })
        .join();

    result.unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn cyan_get_xaero_id() -> *const c_char {
    match SYSTEM.get() {
        Some(s) => CString::new(s.xaero_id.clone()).unwrap().into_raw(),
        None => std::ptr::null(),
    }
}

#[no_mangle]
pub extern "C" fn cyan_create_group(
    name: *const c_char,
    icon: *const c_char,
    color: *const c_char,
    callback_id: u64,
) -> bool {
    if name.is_null() || icon.is_null() || color.is_null() {
        return false;
    }
    let name_str = unsafe { CStr::from_ptr(name) }.to_string_lossy().to_string();
    let icon_str = unsafe { CStr::from_ptr(icon) }.to_string_lossy().to_string();
    let color_str = unsafe { CStr::from_ptr(color) }.to_string_lossy().to_string();

    let cmd = Command::CreateGroup {
        name: name_str,
        icon: icon_str,
        color: color_str,
    };
    let json = serde_json::to_vec(&cmd).unwrap();

    let Some(system) = SYSTEM.get() else { return false };
    let ffi_cmd = FFICommand {
        cmd_type: 1,
        data: json,
        callback_id,
    };

    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_create_workspace(group_hex: *const c_char, name: *const c_char) -> bool {
    if group_hex.is_null() || name.is_null() {
        return false;
    }
    let group_id = unsafe { CStr::from_ptr(group_hex) }.to_string_lossy().to_string();
    let name_str = unsafe { CStr::from_ptr(name) }.to_string_lossy().to_string();

    let cmd = Command::CreateWorkspace { group_id, name: name_str };
    let json = serde_json::to_vec(&cmd).unwrap();

    let Some(system) = SYSTEM.get() else { return false };
    let ffi_cmd = FFICommand {
        cmd_type: 2,
        data: json,
        callback_id: 0,
    };
    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_create_board(
    group_hex: *const c_char,
    workspace_hex: *const c_char,
    name: *const c_char,
) -> bool {
    if group_hex.is_null() || workspace_hex.is_null() || name.is_null() {
        return false;
    }
    let _group_id = unsafe { CStr::from_ptr(group_hex) }.to_string_lossy().to_string();
    let workspace_id = unsafe { CStr::from_ptr(workspace_hex) }.to_string_lossy().to_string();
    let name_str = unsafe { CStr::from_ptr(name) }.to_string_lossy().to_string();

    let id = blake3::hash(format!("{}-{}", name_str, chrono::Utc::now()).as_bytes())
        .to_hex()
        .to_string();
    let obj = Object::Whiteboard {
        id: id.clone(),
        workspace_id: workspace_id.clone(),
        name: name_str,
        data: Vec::new(),
        created_at: chrono::Utc::now().timestamp(),
    };
    let cmd = Command::CreateObject {
        workspace_id,
        object: obj,
    };
    let json = serde_json::to_vec(&cmd).unwrap();

    let Some(system) = SYSTEM.get() else { return false };
    let ffi_cmd = FFICommand {
        cmd_type: 3,
        data: json,
        callback_id: 0,
    };
    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_upload_file_to_group(group_hex: *const c_char, path: *const c_char) -> bool {
    if group_hex.is_null() || path.is_null() {
        return false;
    }
    let group_id = unsafe { CStr::from_ptr(group_hex) }.to_string_lossy().to_string();
    let path_str = unsafe { CStr::from_ptr(path) }.to_string_lossy().to_string();

    let cmd = Command::UploadFileForGroup { group_id, path: path_str };
    let json = serde_json::to_vec(&cmd).unwrap();
    let Some(system) = SYSTEM.get() else { return false };
    let ffi_cmd = FFICommand { cmd_type: 10, data: json, callback_id: 0 };
    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_upload_file_to_workspace(workspace_hex: *const c_char, path: *const c_char) -> bool {
    if workspace_hex.is_null() || path.is_null() {
        return false;
    }
    let workspace_id = unsafe { CStr::from_ptr(workspace_hex) }.to_string_lossy().to_string();
    let path_str = unsafe { CStr::from_ptr(path) }.to_string_lossy().to_string();

    let cmd = Command::UploadFileForWorkspace { workspace_id, path: path_str };
    let json = serde_json::to_vec(&cmd).unwrap();
    let Some(system) = SYSTEM.get() else { return false };
    let ffi_cmd = FFICommand { cmd_type: 11, data: json, callback_id: 0 };
    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_upload_file_to_board(board_hex: *const c_char, path: *const c_char) -> bool {
    if board_hex.is_null() || path.is_null() {
        return false;
    }
    let board_id = unsafe { CStr::from_ptr(board_hex) }.to_string_lossy().to_string();
    let path_str = unsafe { CStr::from_ptr(path) }.to_string_lossy().to_string();

    let cmd = Command::UploadFileForBoard { board_id, path: path_str };
    let json = serde_json::to_vec(&cmd).unwrap();
    let Some(system) = SYSTEM.get() else { return false };
    let ffi_cmd = FFICommand { cmd_type: 12, data: json, callback_id: 0 };
    system.command_tx.send(ffi_cmd).is_ok()
}

// =================== Snapshot for Swift ===================

#[derive(Debug, Clone, Serialize,serde::Deserialize)]
struct GroupRowDTO {
    id: String,
    name: String,
    icon: String,
    color: String,
    created_at: i64,
}
#[derive(Debug, Clone, Serialize,serde::Deserialize)]
struct WorkspaceRowDTO {
    id: String,
    group_id: String,
    name: String,
    created_at: i64,
}
#[derive(Debug, Clone, Serialize,serde::Deserialize)]
struct WhiteboardRowDTO {
    id: String,
    workspace_id: String,
    name: String,
    created_at: i64,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TreeSnapshotDTO {
    pub groups: Vec<GroupRowDTO>,
    pub workspaces: Vec<WorkspaceRowDTO>,
    pub whiteboards: Vec<WhiteboardRowDTO>,
}

fn dump_tree_to_json_string(conn: &Connection) -> Result<String> {
    // groups
    let mut groups_stmt =
        conn.prepare("SELECT id, name, IFNULL(icon,''), IFNULL(color,''), created_at FROM groups ORDER BY created_at ASC")?;
    let mut groups_vec: Vec<GroupRowDTO> = Vec::new();
    {
        let rows = groups_stmt.query_map([], |row| {
            Ok(GroupRowDTO {
                id: row.get(0)?,
                name: row.get(1)?,
                icon: row.get(2)?,
                color: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        for r in rows {
            groups_vec.push(r?);
        }
    }

    // workspaces
    let mut ws_stmt = conn.prepare(
        "SELECT id, group_id, name, created_at FROM workspaces ORDER BY created_at ASC",
    )?;
    let mut ws_vec: Vec<WorkspaceRowDTO> = Vec::new();
    {
        let rows = ws_stmt.query_map([], |row| {
            Ok(WorkspaceRowDTO {
                id: row.get(0)?,
                group_id: row.get(1)?,
                name: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        for r in rows {
            ws_vec.push(r?);
        }
    }

    // whiteboards (objects table)
    let mut wb_stmt = conn.prepare(
        "SELECT id, workspace_id, data, created_at FROM objects WHERE type='whiteboard' ORDER BY created_at ASC",
    )?;
    let mut wb_vec: Vec<WhiteboardRowDTO> = Vec::new();
    {
        let rows = wb_stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let workspace_id: String = row.get(1)?;
            let data: Vec<u8> = row.get(2)?;
            let created_at: i64 = row.get(3)?;
            let name = match serde_json::from_slice::<Object>(&data) {
                Ok(Object::Whiteboard { name, .. }) => name,
                _ => "Whiteboard".to_string(),
            };
            Ok(WhiteboardRowDTO {
                id,
                workspace_id,
                name,
                created_at,
            })
        })?;
        for r in rows {
            wb_vec.push(r?);
        }
    }

    let json = serde_json::json!({
        "groups": groups_vec,
        "workspaces": ws_vec,
        "whiteboards": wb_vec
    });
    Ok(serde_json::to_string(&json)?)
}

#[no_mangle]
pub extern "C" fn cyan_dump_tree_json() -> *const c_char {
    if let Some(sys) = SYSTEM.get() {
        let db = sys.db.lock().unwrap();
        match dump_tree_to_json_string(&db) {
            Ok(s) => CString::new(s).unwrap().into_raw(),
            Err(e) => {
                eprintln!("dump_tree_json error: {e}");
                CString::new("{}".to_string()).unwrap().into_raw()
            }
        }
    } else {
        CString::new("{}".to_string()).unwrap().into_raw()
    }
}

#[no_mangle]
pub extern "C" fn cyan_free_c_string(p: *mut c_char) {
    if p.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(p);
    }
}

#[no_mangle]
pub extern "C" fn cyan_seed_demo_if_empty() -> bool {
    let Some(sys) = SYSTEM.get() else { return false; };
    let db_guard = match sys.db.lock() { Ok(g) => g, Err(_) => return false };
    let db: &rusqlite::Connection = &db_guard;

    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))
        .unwrap_or(0);

    if count > 0 {
        return true; // already populated
    }

    let now = chrono::Utc::now().timestamp();

    // Demo group
    let g1 = blake3::hash(b"DemoGroup").to_hex().to_string();
    let _ = db.execute(
        "INSERT INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![g1, "Design", "folder.fill", "#00AEEF", now],
    );

    // Demo workspaces
    let ws1 = blake3::hash(b"WS:BrandKit").to_hex().to_string();
    let ws2 = blake3::hash(b"WS:MarketingSite").to_hex().to_string();

    let _ = db.execute(
        "INSERT INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![ws1, g1, "Brand Kit", now + 1],
    );
    let _ = db.execute(
        "INSERT INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![ws2, g1, "Marketing Site", now + 2],
    );

    // Demo whiteboards in `objects` (type = 'whiteboard')
    let wb1 = blake3::hash(b"WB:LogoExplorations").to_hex().to_string();
    let wb2 = blake3::hash(b"WB:HomepageWireframe").to_hex().to_string();

    let wb_json1 = serde_json::to_vec(&serde_json::json!({
        "Whiteboard": {
            "id": wb1, "workspace_id": ws1, "name": "Logo Explorations", "data": []
        }
    })).unwrap();

    let wb_json2 = serde_json::to_vec(&serde_json::json!({
        "Whiteboard": {
            "id": wb2, "workspace_id": ws2, "name": "Homepage Wireframe", "data": []
        }
    })).unwrap();

    let _ = db.execute(
        "INSERT INTO objects (id, workspace_id, type, data, created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
        rusqlite::params![wb1, ws1, wb_json1, now + 3],
    );
    let _ = db.execute(
        "INSERT INTO objects (id, workspace_id, type, data, created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
        rusqlite::params![wb2, ws2, wb_json2, now + 4],
    );

    true
}

#[no_mangle]
pub extern "C" fn cyan_seed_from_json(json: *const std::os::raw::c_char) -> bool {
    if json.is_null() { return false; }
    let cstr = unsafe { std::ffi::CStr::from_ptr(json) };
    let Ok(s) = cstr.to_str() else { return false; };
    let Ok(snap) = serde_json::from_str::<TreeSnapshotDTO>(s) else { return false; };

    let Some(sys) = SYSTEM.get() else { return false; };
    let mut db_guard = match sys.db.lock() { Ok(g) => g, Err(_) => return false };
    // Best-effort transaction
    let tx = match db_guard.transaction() { Ok(t) => t, Err(_) => return false };

    for g in snap.groups {
        let _ = tx.execute(
            "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![g.id, g.name, g.icon, g.color, g.created_at],
        );
    }
    for ws in snap.workspaces {
        let _ = tx.execute(
            "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![ws.id, ws.group_id, ws.name, ws.created_at],
        );
    }
    for wb in snap.whiteboards {
        // store as object type=whiteboard; embed a simple JSON with "Whiteboard" wrapper
        let data = serde_json::to_vec(&serde_json::json!({
            "Whiteboard": {
                "id": wb.id,
                "workspace_id": wb.workspace_id,
                "name": wb.name,
                "data": []
            }
        })).unwrap_or_default();

        let _ = tx.execute(
            "INSERT OR IGNORE INTO objects (id, workspace_id, type, data, created_at) VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
            rusqlite::params![wb.id, wb.workspace_id, data, wb.created_at],
        );
    }

    let _ = tx.commit();
    true
}
