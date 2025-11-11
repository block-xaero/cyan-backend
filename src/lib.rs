// src/lib.rs
use anyhow::Result;
use bytes::Bytes;
use futures::StreamExt;
use once_cell::sync::OnceCell;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use iroh::{Endpoint, RelayMode, SecretKey};
use iroh::discovery::pkarr::PkarrPublisher;
use iroh_blobs::store::fs::FsStore as BlobStore;
use iroh_gossip::{net::Gossip, proto::TopicId};
use iroh_gossip::api::{GossipTopic, Event as GossipEvent};

use rand_chacha::rand_core::SeedableRng;

// --- Global State ---
static RUNTIME: OnceCell<Runtime> = OnceCell::new();
static SYSTEM: OnceCell<Arc<CyanSystem>> = OnceCell::new();
static DISCOVERY_KEY: OnceCell<String> = OnceCell::new();

// --- Core Types ---
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
    },
    File {
        id: String,
        workspace_id: String,
        name: String,
        hash: String,
        size: u64,
    },
    Chat {
        id: String,
        workspace_id: String,
        message: String,
        author: String,
        timestamp: i64,
    },
}

// --- Network Events (group topics) ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkEvent {
    GroupCreated(Group),
    WorkspaceCreated(Workspace),
    ObjectCreated(Object),
    ObjectUpdated(Object),
    FileAvailable { hash: String, name: String, size: u64 },
}

// --- Discovery Messages (discovery topic) ---
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum DiscoveryMsg {
    PeerAnnounce { id: String, ts: i64 },
    SyncGroups { groups: Vec<Group> },
    SyncWorkspaces { group_id: String, workspaces: Vec<Workspace> },
    SyncObjects { workspace_id: String, objects: Vec<Object> },
}

// --- Commands ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    // legacy CLI path
    CreateGroup { name: String, icon: String, color: String },
    // binary path (UI provides ids/timestamps)
    CreateGroupEx { id: String, name: String, icon: String, color: String, created_at: i64 },

    CreateWorkspace { id: String, group_id: String, name: String, created_at: i64 },

    CreateObject { workspace_id: String, object: Object },
    UpdateObject { object: Object },

    UploadFile { workspace_id: String, path: String },
}

// --- FFI Command wrapper ---
#[repr(C)]
pub struct FFICommand {
    pub cmd_type: u32,
    pub data: Vec<u8>,
    pub callback_id: u64,
}

// --- Network Command ---
#[derive(Debug)]
enum UploadTarget {
    Group(String),
    Workspace(String),
    Board(String),
}

#[derive(Debug)]
enum NetworkCommand {
    BroadcastEvent { group_id: String, event: NetworkEvent },
    JoinGroup { group_id: String },
    UploadFile { target: UploadTarget, path: String },
}

// --- Main System ---
pub struct CyanSystem {
    pub xaero_id: String,
    pub secret_key: SecretKey,
    pub(crate) command_tx: mpsc::UnboundedSender<FFICommand>,
    pub(crate) network_tx: mpsc::UnboundedSender<NetworkCommand>,
    pub db: Arc<Mutex<Connection>>,
}

impl CyanSystem {
    async fn new() -> Result<Self> {
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        let secret_key = SecretKey::generate(&mut rng);
        let xaero_id = secret_key.public().to_string();

        // DB in current dir
        let db_path = Path::new("cyan.db");
        let db = Connection::open(db_path)?;
        Self::init_db(&db)?;

        // channels
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (network_tx, network_rx) = mpsc::unbounded_channel();

        let system = Self {
            xaero_id: xaero_id.clone(),
            secret_key: secret_key.clone(),
            command_tx,
            network_tx: network_tx.clone(),
            db: Arc::new(Mutex::new(db)),
        };

        // command actor
        let db_clone = system.db.clone();
        let network_tx_clone = network_tx.clone();
        RUNTIME.get().unwrap().spawn(async move {
            let actor = CommandActor {
                rx: command_rx,
                db: db_clone,
                network_tx: network_tx_clone,
            };
            actor.run().await;
        });

        // network actor
        let db_clone = system.db.clone();
        RUNTIME.get().unwrap().spawn(async move {
            let network = NetworkActor::new(xaero_id.clone(), secret_key, db_clone, network_rx)
                .await
                .expect("Failed to create network actor");
            network.run().await;
        });

        Ok(system)
    }

    fn init_db(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
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
            ",
        )?;
        Ok(())
    }
}

// --- Command Actor ---
struct CommandActor {
    rx: mpsc::UnboundedReceiver<FFICommand>,
    db: Arc<Mutex<Connection>>,
    network_tx: mpsc::UnboundedSender<NetworkCommand>,
}

impl CommandActor {
    async fn run(mut self) {
        while let Some(ffi_cmd) = self.rx.recv().await {
            if let Ok(cmd) = serde_json::from_slice::<Command>(&ffi_cmd.data) {
                let _ = self.process_command(cmd, ffi_cmd.callback_id).await;
            }
        }
    }

    async fn process_command(&mut self, cmd: Command, _callback_id: u64) -> Result<()> {
        let db = self.db.lock().unwrap();

        match cmd {
            Command::CreateGroup { name, icon, color } => {
                // legacy CLI path: synth id & created_at
                let id = blake3::hash(format!("{}-{}", name, chrono::Utc::now()).as_bytes())
                    .to_hex()
                    .to_string();
                let now = chrono::Utc::now().timestamp();

                let group = Group {
                    id: id.clone(),
                    name,
                    icon,
                    color,
                    created_at: now,
                };

                db.execute(
                    "INSERT INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![&group.id, &group.name, &group.icon, &group.color, group.created_at],
                )?;

                self.network_tx
                    .send(NetworkCommand::JoinGroup {
                        group_id: id.clone(),
                    })
                    .unwrap();

                self.network_tx
                    .send(NetworkCommand::BroadcastEvent {
                        group_id: id,
                        event: NetworkEvent::GroupCreated(group),
                    })
                    .unwrap();
            }

            Command::CreateGroupEx {
                id,
                name,
                icon,
                color,
                created_at,
            } => {
                let group = Group {
                    id: id.clone(),
                    name,
                    icon,
                    color,
                    created_at,
                };

                db.execute(
                    "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![&group.id, &group.name, &group.icon, &group.color, group.created_at],
                )?;

                self.network_tx
                    .send(NetworkCommand::JoinGroup {
                        group_id: id.clone(),
                    })
                    .unwrap();

                self.network_tx
                    .send(NetworkCommand::BroadcastEvent {
                        group_id: id,
                        event: NetworkEvent::GroupCreated(group),
                    })
                    .unwrap();
            }

            Command::CreateWorkspace {
                id,
                group_id,
                name,
                created_at,
            } => {
                db.execute(
                    "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![&id, &group_id, &name, created_at],
                )?;

                let ws = Workspace {
                    id: id.clone(),
                    group_id: group_id.clone(),
                    name,
                    created_at,
                };

                self.network_tx
                    .send(NetworkCommand::BroadcastEvent {
                        group_id,
                        event: NetworkEvent::WorkspaceCreated(ws),
                    })
                    .ok();
            }

            Command::CreateObject { workspace_id, object } => {
                let now = chrono::Utc::now().timestamp();

                let (id, type_str, data) = match &object {
                    Object::Whiteboard { id, .. } => (id.clone(), "whiteboard", serde_json::to_vec(&object)?),
                    Object::File { id, .. } => (id.clone(), "file", serde_json::to_vec(&object)?),
                    Object::Chat { .. } => {
                        let id = blake3::hash(uuid::Uuid::new_v4().as_bytes())
                            .to_hex()
                            .to_string();
                        (id, "chat", serde_json::to_vec(&object)?)
                    }
                };

                db.execute(
                    "INSERT OR IGNORE INTO objects (id, workspace_id, type, data, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, workspace_id, type_str, data, now],
                )?;

                // find group to broadcast on
                let group_id: Option<String> = db
                    .query_row::<String, _, _>(
                        "SELECT group_id FROM workspaces WHERE id = ?1",
                        [workspace_id.clone()],
                        |row| row.get(0),
                    )
                    .ok();

                if let Some(gid) = group_id {
                    self.network_tx
                        .send(NetworkCommand::BroadcastEvent {
                            group_id: gid,
                            event: NetworkEvent::ObjectCreated(object),
                        })
                        .ok();
                }
            }

            Command::UpdateObject { .. } => {
                // left for future
            }

            Command::UploadFile { workspace_id: _, path: _ } => {
                // not used; uploads go direct through NetworkCommand
            }
        }

        Ok(())
    }
}

// --- Network Actor with Iroh ---
struct NetworkActor {
    xaero_id: String,
    db: Arc<Mutex<Connection>>,
    rx: mpsc::UnboundedReceiver<NetworkCommand>,
    _endpoint: Endpoint,  // underscore to silence "never read" warning
    gossip: Gossip,
    blob_store: BlobStore,
    groups: HashMap<String, TopicId>,
    discovery: GossipTopic, // discovery topic handle (listen + broadcast)
}

impl NetworkActor {
    async fn new(
        xaero_id: String,
        secret_key: SecretKey,
        db: Arc<Mutex<Connection>>,
        rx: mpsc::UnboundedReceiver<NetworkCommand>,
    ) -> Result<Self> {
        let discovery_key = DISCOVERY_KEY.get().map(|k| k.clone()).unwrap_or_else(|| {
            blake3::hash(xaero_id.as_bytes()).to_hex().to_string()
        });

        println!("Discovery Key: {}", discovery_key);

        let builder = Endpoint::builder()
            .secret_key(secret_key.clone())
            .alpns(vec![b"xsp-1.0".to_vec()])
            .relay_mode(RelayMode::Default);

        let builder = if !discovery_key.is_empty() {
            let pkarr_publisher = PkarrPublisher::n0_dns().build(secret_key.clone());
            builder.discovery(pkarr_publisher)
        } else {
            builder
        };

        let endpoint = builder.bind().await?;
        println!("Node ID: {}", endpoint.id());

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let blob_store = BlobStore::load("./blobs").await?;

        let discovery_topic_id = TopicId::from_bytes(
            blake3::hash(format!("cyan/discovery/{}", discovery_key).as_bytes())
                .as_bytes()[..32]
                .try_into()
                .unwrap(),
        );

        let mut discovery = gossip.subscribe(discovery_topic_id, vec![]).await?;

        // Announce ourselves (peers use this to trigger a snapshot push)
        let announce = DiscoveryMsg::PeerAnnounce {
            id: xaero_id.clone(),
            ts: chrono::Utc::now().timestamp(),
        };
        discovery
            .broadcast(Bytes::from(serde_json::to_vec(&announce)?))
            .await?;
        println!("Joined discovery network");

        Ok(Self {
            xaero_id,
            db,
            rx,
            _endpoint: endpoint,
            gossip,
            blob_store,
            groups: HashMap::new(),
            discovery,
        })
    }

    async fn run(mut self) {
        // Subscribe to existing groups on startup
        if let Err(e) = self.subscribe_to_existing_groups().await {
            eprintln!("Failed to subscribe to existing groups: {}", e);
        }

        // Push full snapshot on startup (so existing peers see our state too)
        if let Err(e) = self.send_full_snapshot().await {
            eprintln!("send_full_snapshot (startup) error: {e}");
        }

        loop {
            tokio::select! {
                Some(cmd) = self.rx.recv() => {
                    match cmd {
                        NetworkCommand::BroadcastEvent { group_id, event } => {
                            let topic_id = self.get_or_join_group(&group_id).await;
                            match self.gossip.subscribe(topic_id, vec![]).await {
                                Ok(mut topic) => {
                                    let data = serde_json::to_vec(&event).unwrap();
                                    if let Err(e) = topic.broadcast(Bytes::from(data)).await {
                                        eprintln!("Failed to broadcast: {}", e);
                                    }
                                }
                                Err(e) => eprintln!("Failed to open topic for broadcast: {}", e),
                            }

                            // Mirror GroupCreated to discovery so late joiners learn group topics fast
                            if let NetworkEvent::GroupCreated(g) = &event {
                                let msg = DiscoveryMsg::SyncGroups { groups: vec![g.clone()] };
                                let _ = self.discovery.broadcast(Bytes::from(serde_json::to_vec(&msg).unwrap())).await;
                            }
                        }

                        NetworkCommand::JoinGroup { group_id } => {
                            self.get_or_join_group(&group_id).await;
                        }

                        NetworkCommand::UploadFile { target, path } => {
                            match tokio::fs::read(&path).await {
                                Ok(data) => {
                                    let tag = self.blob_store.add_bytes(data.clone()).await.unwrap();
                                    let hash = tag.hash;
                                    println!("File uploaded: {:?} ({} bytes)", hash, data.len());

                                    // Optionally broadcast file availability to a group if we can resolve it
                                    if let Some(group_id) = match target {
                                        UploadTarget::Workspace(ref wid) => self.resolve_group_for_workspace(wid),
                                        UploadTarget::Board(ref _bid) => None,
                                        UploadTarget::Group(ref gid) => Some(gid.clone()),
                                    } {
                                        let name = std::path::Path::new(&path)
                                            .file_name()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("file")
                                            .to_string();

                                        let evt = NetworkEvent::FileAvailable {
                                            hash: format!("{:?}", hash),
                                            name,
                                            size: data.len() as u64,
                                        };

                                        self.gossip_event(group_id, evt).await.ok();
                                    }
                                }
                                Err(e) => eprintln!("Failed to read file {}: {}", path, e),
                            }
                        }
                    }
                }

                Some(Ok(evt)) = self.discovery.next() => {
                    if let GossipEvent::Received(msg) = evt {
                        // First try DiscoveryMsg
                        if let Ok(dmsg) = serde_json::from_slice::<DiscoveryMsg>(&msg.content) {
                            match dmsg {
                                DiscoveryMsg::PeerAnnounce { id, .. } => {
                                    if id != self.xaero_id {
                                        // Push our snapshot to the newly-announced peer
                                        let _ = self.send_full_snapshot().await;
                                    }
                                }
                                DiscoveryMsg::SyncGroups { groups } => {
                                    // Upsert groups & join topics
                                    {
                                        let db = self.db.lock().unwrap();
                                        for g in &groups {
                                            let _ = db.execute(
                                                "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                                                params![g.id, g.name, g.icon, g.color, g.created_at],
                                            );
                                        }
                                    }
                                    for g in groups {
                                        self.get_or_join_group(&g.id).await;
                                    }
                                }
                                DiscoveryMsg::SyncWorkspaces { group_id: _ , workspaces } => {
                                    let db = self.db.lock().unwrap();
                                    for ws in workspaces {
                                        let _ = db.execute(
                                            "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                                            params![ws.id, ws.group_id, ws.name, ws.created_at],
                                        );
                                    }
                                }
                                DiscoveryMsg::SyncObjects { workspace_id: _ , objects } => {
                                    let db = self.db.lock().unwrap();
                                    for obj in objects {
                                        match &obj {
                                            Object::Whiteboard { id, workspace_id, .. } => {
                                                let _ = db.execute(
                                                    "INSERT OR IGNORE INTO objects (id, workspace_id, type, data, created_at)
                                                     VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
                                                    params![
                                                        id,
                                                        workspace_id,
                                                        serde_json::to_vec(&obj).unwrap(),
                                                        chrono::Utc::now().timestamp()
                                                    ],
                                                );
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            continue;
                        }

                        // Back-compat: also accept NetworkEvent::GroupCreated mirrored on discovery
                        if let Ok(net_evt) = serde_json::from_slice::<NetworkEvent>(&msg.content) {
                            if let NetworkEvent::GroupCreated(g) = net_evt {
                                let _ = self.db.lock().unwrap().execute(
                                    "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                                    params![g.id, g.name, g.icon, g.color, g.created_at],
                                );
                                self.get_or_join_group(&g.id).await;
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Snapshot helpers ---

    async fn send_full_snapshot(&mut self) -> Result<()> {
        // Gather snapshot under DB lock, then release before awaits
        let (groups, workspaces_by_group, whiteboards_by_ws) = {
            let db = self.db.lock().unwrap();

            // groups
            let mut gs = Vec::new();
            {
                let mut stmt = db.prepare("SELECT id, name, icon, color, created_at FROM groups")?;
                for r in stmt.query_map([], |row| {
                    Ok(Group {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        icon: row.get(2)?,
                        color: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })? {
                    gs.push(r?);
                }
            }

            // workspaces per group
            let mut ws_map: HashMap<String, Vec<Workspace>> = HashMap::new();
            {
                let mut stmt = db.prepare("SELECT id, group_id, name, created_at FROM workspaces")?;
                for r in stmt.query_map([], |row| {
                    Ok(Workspace {
                        id: row.get(0)?,
                        group_id: row.get(1)?,
                        name: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                })? {
                    let ws = r?;
                    ws_map.entry(ws.group_id.clone()).or_default().push(ws);
                }
            }

            // whiteboards per workspace
            let mut wb_map: HashMap<String, Vec<Object>> = HashMap::new();
            {
                let mut stmt = db.prepare("SELECT data FROM objects WHERE type = 'whiteboard'")?;
                for r in stmt.query_map([], |row| {
                    let data: Vec<u8> = row.get(0)?;
                    let obj: Object = serde_json::from_slice(&data).unwrap_or_else(|_| {
                        Object::Whiteboard { id: String::new(), workspace_id: String::new(), name: "<invalid>".into(), data: Vec::new() }
                    });
                    Ok(obj)
                })? {
                    let obj = r?;
                    if let Object::Whiteboard { workspace_id, .. } = &obj {
                        wb_map.entry(workspace_id.clone()).or_default().push(obj);
                    }
                }
            }

            (gs, ws_map, wb_map)
        };

        // Send groups
        if !groups.is_empty() {
            let msg = DiscoveryMsg::SyncGroups { groups: groups.clone() };
            let _ = self.discovery.broadcast(Bytes::from(serde_json::to_vec(&msg)?)).await;
        }

        // Send workspaces grouped by group
        for (gid, workspaces) in workspaces_by_group {
            if workspaces.is_empty() { continue; }
            let msg = DiscoveryMsg::SyncWorkspaces { group_id: gid, workspaces };
            let _ = self.discovery.broadcast(Bytes::from(serde_json::to_vec(&msg)?)).await;
        }

        // Send whiteboards grouped by workspace
        for (wid, objs) in whiteboards_by_ws {
            if objs.is_empty() { continue; }
            let msg = DiscoveryMsg::SyncObjects { workspace_id: wid, objects: objs };
            let _ = self.discovery.broadcast(Bytes::from(serde_json::to_vec(&msg)?)).await;
        }

        Ok(())
    }

    fn resolve_group_for_workspace(&self, _wid: &str) -> Option<String> {
        let db = self.db.lock().unwrap();
        db.query_row::<String, _, _>(
            "SELECT group_id FROM workspaces WHERE id = ?1",
            [_wid],
            |row| row.get(0),
        )
            .ok()
    }

    async fn gossip_event(&self, group_id: String, event: NetworkEvent) -> Result<()> {
        let topic_id = *self.groups.get(&group_id).unwrap_or(&self.compute_topic(&group_id));
        if let Ok(mut topic) = self.gossip.subscribe(topic_id, vec![]).await {
            topic.broadcast(Bytes::from(serde_json::to_vec(&event)?)).await?;
        }
        Ok(())
    }

    async fn get_or_join_group(&mut self, group_id: &str) -> TopicId {
        if let Some(topic_id) = self.groups.get(group_id) {
            return *topic_id;
        }

        let topic_id = self.compute_topic(group_id);

        if let Ok(mut sub) = self.gossip.subscribe(topic_id, vec![]).await {
            println!("Joined group: {}", group_id);
            self.groups.insert(group_id.to_string(), topic_id);

            let db = self.db.clone();
            let gid = group_id.to_string();
            tokio::spawn(async move {
                while let Some(Ok(event)) = sub.next().await {
                    if let iroh_gossip::api::Event::Received(msg) = event {
                        if let Ok(net_event) = serde_json::from_slice::<NetworkEvent>(&msg.content) {
                            Self::handle_network_event(db.clone(), gid.clone(), net_event).await;
                        }
                    }
                }
            });
        }

        topic_id
    }

    fn compute_topic(&self, group_id: &str) -> TopicId {
        let topic_bytes: [u8; 32] = blake3::hash(format!("cyan/group/{}", group_id).as_bytes())
            .as_bytes()[..32]
            .try_into()
            .unwrap();
        TopicId::from_bytes(topic_bytes)
    }

    async fn subscribe_to_existing_groups(&mut self) -> Result<()> {
        let group_ids = {
            let db = self.db.lock().unwrap();
            let mut stmt = db.prepare("SELECT id FROM groups")?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
            ids
        };

        for group_id in group_ids {
            println!("Subscribing to existing group: {}", group_id);
            self.get_or_join_group(&group_id).await;
        }

        Ok(())
    }

    async fn handle_network_event(db: Arc<Mutex<Connection>>, _group_id: String, event: NetworkEvent) {
        let db = db.lock().unwrap();
        match event {
            NetworkEvent::GroupCreated(group) => {
                println!("Received group from network: {}", group.name);
                let _ = db.execute(
                    "INSERT OR IGNORE INTO groups (id, name, icon, color, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![group.id, group.name, group.icon, group.color, group.created_at],
                );
            }
            NetworkEvent::WorkspaceCreated(ws) => {
                println!("Received workspace from network: {}", ws.name);
                let _ = db.execute(
                    "INSERT OR IGNORE INTO workspaces (id, group_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![ws.id, ws.group_id, ws.name, ws.created_at],
                );
            }
            NetworkEvent::ObjectCreated(obj) => {
                println!("Received object from network");
                // Borrow obj to avoid partial moves; can still serialize &obj below
                match &obj {
                    Object::Whiteboard { id, workspace_id, .. } => {
                        let _ = db.execute(
                            "INSERT OR IGNORE INTO objects (id, workspace_id, type, data, created_at)
                             VALUES (?1, ?2, 'whiteboard', ?3, ?4)",
                            params![
                                id,
                                workspace_id,
                                serde_json::to_vec(&obj).unwrap(),
                                chrono::Utc::now().timestamp()
                            ],
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// --- FFI Helpers ---
fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn trim_utf8(bytes: &[u8]) -> String {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

fn parse_group_bin(buf: &[u8]) -> Option<(String, String, String, String, i64)> {
    if buf.len() < 220 {
        return None;
    }
    let gid = bytes_to_hex(&buf[0..32]);
    let name = trim_utf8(&buf[64..128]);
    let icon = trim_utf8(&buf[128..160]);
    // simple rgba -> hex
    let r = buf[160];
    let g = buf[161];
    let b = buf[162];
    let a = buf[163];
    let color = format!("#{r:02x}{g:02x}{b:02x}{a:02x}");
    let created_at = {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&buf[172..180]);
        i64::from_le_bytes(arr)
    };
    Some((gid, name, icon, color, created_at))
}

fn parse_workspace_bin(buf: &[u8]) -> Option<(String, String, String, i64)> {
    if buf.len() < 184 {
        return None;
    }
    let wid = bytes_to_hex(&buf[0..32]);
    let gid = bytes_to_hex(&buf[32..64]);
    let name = trim_utf8(&buf[64..128]);
    let created_at = {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&buf[136..144]);
        i64::from_le_bytes(arr)
    };
    Some((wid, gid, name, created_at))
}

fn parse_board_bin(buf: &[u8]) -> Option<(String, String, String, String, i64)> {
    if buf.len() < 256 {
        return None;
    }
    let bid = bytes_to_hex(&buf[0..32]);
    let wid = bytes_to_hex(&buf[32..64]);
    let gid = bytes_to_hex(&buf[64..96]);
    let name = trim_utf8(&buf[96..160]);
    let created_at = {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&buf[176..184]);
        i64::from_le_bytes(arr)
    };
    Some((bid, wid, gid, name, created_at))
}

// --- FFI: data dir / discovery / init ---
#[no_mangle]
pub extern "C" fn cyan_set_data_dir(path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    let path_cstr = unsafe { CStr::from_ptr(path) };
    let path_str = match path_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    if std::fs::create_dir_all(path_str).is_err() {
        return false;
    }
    if std::env::set_current_dir(path_str).is_err() {
        return false;
    }
    true
}

#[no_mangle]
pub extern "C" fn cyan_set_discovery_key(key: *const c_char) -> bool {
    if key.is_null() {
        return false;
    }
    let key_cstr = unsafe { CStr::from_ptr(key) };
    let key_str = match key_cstr.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    DISCOVERY_KEY.set(key_str).is_ok()
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

        RUNTIME.set(runtime).ok();

        let rt = RUNTIME.get().unwrap();
        let system = rt.block_on(async { CyanSystem::new().await });

        match system {
            Ok(s) => {
                println!("Cyan initialized with ID: {}", s.xaero_id);
                SYSTEM.set(Arc::new(s)).is_ok()
            }
            Err(e) => {
                eprintln!("Failed to init: {}", e);
                false
            }
        }
    })
        .join();

    result.unwrap_or(false)
}

// --- FFI: legacy group create (CLI) ---
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
    let name = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    let icon = match unsafe { CStr::from_ptr(icon) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    let color = match unsafe { CStr::from_ptr(color) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };

    let cmd = Command::CreateGroup { name, icon, color };
    let json = serde_json::to_vec(&cmd).unwrap();

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    let ffi_cmd = FFICommand {
        cmd_type: 1,
        data: json,
        callback_id,
    };
    system.command_tx.send(ffi_cmd).is_ok()
}

// --- FFI: binary create (FileTree) ---
#[no_mangle]
pub extern "C" fn cyan_create_group_bin(data: *const u8, len: usize) -> bool {
    if data.is_null() || len == 0 {
        return false;
    }
    let buf = unsafe { slice::from_raw_parts(data, len) };
    let (id, name, icon, color, created_at) = match parse_group_bin(buf) {
        Some(x) => x,
        None => return false,
    };

    let cmd = Command::CreateGroupEx {
        id,
        name,
        icon,
        color,
        created_at,
    };
    let json = serde_json::to_vec(&cmd).unwrap();

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    let ffi_cmd = FFICommand {
        cmd_type: 2,
        data: json,
        callback_id: 0,
    };
    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_create_workspace_bin(data: *const u8, len: usize) -> bool {
    if data.is_null() || len == 0 {
        return false;
    }
    let buf = unsafe { slice::from_raw_parts(data, len) };
    let (id, group_id, name, created_at) = match parse_workspace_bin(buf) {
        Some(x) => x,
        None => return false,
    };

    let cmd = Command::CreateWorkspace {
        id,
        group_id,
        name,
        created_at,
    };
    let json = serde_json::to_vec(&cmd).unwrap();

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    let ffi_cmd = FFICommand {
        cmd_type: 3,
        data: json,
        callback_id: 0,
    };
    system.command_tx.send(ffi_cmd).is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_create_board_bin(data: *const u8, len: usize) -> bool {
    if data.is_null() || len == 0 {
        return false;
    }
    let buf = unsafe { slice::from_raw_parts(data, len) };
    let (board_id, workspace_id, _group_id, name, _created_at) = match parse_board_bin(buf) {
        Some(x) => x,
        None => return false,
    };

    let obj = Object::Whiteboard {
        id: board_id,
        workspace_id: workspace_id.clone(),
        name,
        data: Vec::new(),
    };

    let cmd = Command::CreateObject {
        workspace_id,
        object: obj,
    };

    let json = serde_json::to_vec(&cmd).unwrap();

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    let ffi_cmd = FFICommand {
        cmd_type: 4,
        data: json,
        callback_id: 0,
    };
    system.command_tx.send(ffi_cmd).is_ok()
}

// --- FFI: uploads from FileTree drops ---
#[no_mangle]
pub extern "C" fn cyan_upload_file_to_workspace(workspace_hex: *const c_char, path: *const c_char) -> bool {
    if workspace_hex.is_null() || path.is_null() {
        return false;
    }
    let workspace_id = match unsafe { CStr::from_ptr(workspace_hex) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    system
        .network_tx
        .send(NetworkCommand::UploadFile {
            target: UploadTarget::Workspace(workspace_id),
            path,
        })
        .is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_upload_file_to_board(board_hex: *const c_char, path: *const c_char) -> bool {
    if board_hex.is_null() || path.is_null() {
        return false;
    }
    let board_id = match unsafe { CStr::from_ptr(board_hex) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    system
        .network_tx
        .send(NetworkCommand::UploadFile {
            target: UploadTarget::Board(board_id),
            path,
        })
        .is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_upload_file_to_group(group_hex: *const c_char, path: *const c_char) -> bool {
    if group_hex.is_null() || path.is_null() {
        return false;
    }
    let group_id = match unsafe { CStr::from_ptr(group_hex) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };

    let system = match SYSTEM.get() {
        Some(s) => s,
        None => return false,
    };
    system
        .network_tx
        .send(NetworkCommand::UploadFile {
            target: UploadTarget::Group(group_id),
            path,
        })
        .is_ok()
}

#[no_mangle]
pub extern "C" fn cyan_get_xaero_id() -> *const c_char {
    match SYSTEM.get() {
        Some(s) => s.xaero_id.as_ptr() as *const c_char,
        None => std::ptr::null(),
    }
}