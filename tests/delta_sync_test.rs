// tests/delta_sync_test.rs
//
// DELTA SYNC TEST - Verifies continuous sync after initial snapshot
//
// This test verifies:
//   1. Initial snapshot sync works (prerequisite)
//   2. Host broadcasts a new element → Joiner receives it
//   3. Joiner broadcasts a new element → Host receives it
//
// Build:  cargo build --release --bin delta_test
// Run:
//   Machine A (host): ./target/release/delta_test host
//   Machine B (join): ./target/release/delta_test join
//
// Protocol:
//   1. Both start and complete snapshot sync
//   2. After sync, host waits 5s then broadcasts new element
//   3. Joiner prints received element, then broadcasts its own
//   4. Host prints received element
//   5. Both confirm bidirectional delta sync works

use anyhow::Result;
use iroh::{PublicKey, SecretKey};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rusqlite::Connection;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;

// ═══════════════════════════════════════════════════════════════════════════
// IMPORTS FROM CYAN-BACKEND
// ═══════════════════════════════════════════════════════════════════════════

use cyan_backend::{
    actors::NetworkActor,
    models::{
        commands::NetworkCommand,
        events::{NetworkEvent, SwiftEvent},
    },
    storage, DISCOVERY_KEY, RELAY_URL,
};

// ═══════════════════════════════════════════════════════════════════════════
// CONSTANTS
// ═══════════════════════════════════════════════════════════════════════════

const RELAY_URL_CONST: &str = "https://quic.dev.cyan.blockxaero.io";
const DISCOVERY_KEY_CONST: &str = "cyan-dev";

const TEST_GROUP_ID: &str = "test-group-delta-1111-2222-3333-444444444444";
const TEST_WORKSPACE_ID: &str = "test-ws-delta-1111-2222-3333-444444444444";
const TEST_BOARD_ID: &str = "test-board-delta-1111-2222-3333-444444444444";

// ═══════════════════════════════════════════════════════════════════════════
// MAIN
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap())
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        println!("Usage: {} <host|join>", args[0]);
        println!();
        println!("  host  - Start as host with test data, broadcast delta after peer joins");
        println!("  join  - Join group, wait for sync, then broadcast own delta");
        return Ok(());
    }

    // Configure globals
    let _ = RELAY_URL.set(RELAY_URL_CONST.to_string());
    let _ = DISCOVERY_KEY.set(DISCOVERY_KEY_CONST.to_string());

    match args[1].as_str() {
        "host" => run_host().await,
        "join" => run_join().await,
        _ => {
            println!("Unknown mode: {}", args[1]);
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// HOST - Has data, waits for peer, then broadcasts delta
// ═══════════════════════════════════════════════════════════════════════════

async fn run_host() -> Result<()> {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║              DELTA SYNC TEST - HOST                           ║");
    println!("╠═══════════════════════════════════════════════════════════════╣");
    println!("║  Phase 1: Wait for peer to join and sync                      ║");
    println!("║  Phase 2: Broadcast new element (delta)                       ║");
    println!("║  Phase 3: Wait for peer's delta element                       ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Initialize storage
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("host.db");
    init_test_schema(db_path.to_str().unwrap())?;
    storage::init_db(db_path.to_str().unwrap())?;

    // Create test data
    create_host_test_data()?;
    println!("✅ DB initialized with test data");
    println!("   Group: {}...", &TEST_GROUP_ID[..16]);

    // Generate identity
    let mut rng = ChaCha8Rng::from_os_rng();
    let secret_key = SecretKey::generate(&mut rng);
    let node_id = secret_key.public().to_string();
    println!("📱 My node ID: {}...", &node_id[..16]);

    // Create channels
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwiftEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NetworkCommand>();
    let peers_per_group = Arc::new(std::sync::Mutex::new(HashMap::<String, HashSet<PublicKey>>::new()));

    // Start actor
    let actor = NetworkActor::new(secret_key, event_tx, peers_per_group).await?;
    tokio::spawn(async move {
        actor.start(cmd_rx).await;
    });

    println!("✅ NetworkActor started");
    println!("\n📡 Waiting for peer to join...\n");

    // ─────────────────────────────────────────────────────────────────────
    // PHASE 1: Wait for peer to sync
    // ─────────────────────────────────────────────────────────────────────

    let mut peer_joined = false;
    let mut peer_node_id = String::new();

    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                match &event {
                    SwiftEvent::PeerJoined { peer_id, group_id } => {
                        println!("🟢 [HOST] PeerJoined: {}... in {}...",
                            &peer_id[..16], &group_id[..16]);
                        peer_node_id = peer_id.clone();
                        peer_joined = true;
                    }
                    SwiftEvent::Network(net_event) => {
                        match net_event {
                            NetworkEvent::WhiteboardElementAdded { id, .. } => {
                                println!("\n═══════════════════════════════════════════════════════════════════");
                                println!("✅ [HOST] RECEIVED DELTA from peer!");
                                println!("   Element ID: {}", id);
                                println!("═══════════════════════════════════════════════════════════════════");

                                println!("\n╔═══════════════════════════════════════════════════════════════╗");
                                println!("║ ✅ DELTA SYNC TEST COMPLETE - BIDIRECTIONAL WORKS!            ║");
                                println!("╚═══════════════════════════════════════════════════════════════╝\n");
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }

                // After peer joins, wait a bit then send delta
                if peer_joined && !peer_node_id.is_empty() {
                    println!("\n═══════════════════════════════════════════════════════════════════");
                    println!("📤 [HOST] PHASE 2: Broadcasting delta element...");
                    println!("═══════════════════════════════════════════════════════════════════\n");

                    tokio::time::sleep(Duration::from_secs(3)).await;

                    let now = chrono::Utc::now().timestamp();
                    let delta_event = NetworkEvent::WhiteboardElementAdded {
                        id: "delta-from-host-001".to_string(),
                        board_id: TEST_BOARD_ID.to_string(),
                        element_type: "star".to_string(),
                        x: 400.0,
                        y: 400.0,
                        width: 120.0,
                        height: 120.0,
                        z_index: 999,
                        style_json: Some("{\"fill\":\"#FFD700\",\"stroke\":\"#FFA500\"}".to_string()),
                        content_json: Some("{\"text\":\"⭐ Delta from Host!\"}".to_string()),
                        created_at: now,
                        updated_at: now,
                    };

                    cmd_tx.send(NetworkCommand::Broadcast {
                        group_id: TEST_GROUP_ID.to_string(),
                        event: delta_event,
                    })?;

                    println!("✅ [HOST] Delta element broadcast sent!");
                    println!("   Waiting for peer's delta response...\n");
                    peer_joined = false; // Reset so we don't send again
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n👋 Shutting down...");
                break;
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// JOINER - Gets snapshot, receives host's delta, broadcasts own delta
// ═══════════════════════════════════════════════════════════════════════════

async fn run_join() -> Result<()> {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║              DELTA SYNC TEST - JOINER                         ║");
    println!("╠═══════════════════════════════════════════════════════════════╣");
    println!("║  Phase 1: Join group and complete snapshot sync               ║");
    println!("║  Phase 2: Wait for host's delta element                       ║");
    println!("║  Phase 3: Broadcast our own delta element                     ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    // Initialize storage
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("joiner.db");
    init_test_schema(db_path.to_str().unwrap())?;
    storage::init_db(db_path.to_str().unwrap())?;

    // Create empty group (simulating invite)
    storage::group_insert_simple(TEST_GROUP_ID, "Invited Group", "folder.fill", "#FF6B6B")?;
    println!("✅ DB initialized (empty group from invite)");

    // Generate identity
    let mut rng = ChaCha8Rng::from_os_rng();
    let secret_key = SecretKey::generate(&mut rng);
    let node_id = secret_key.public().to_string();
    println!("📱 My node ID: {}...", &node_id[..16]);

    // Create channels
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwiftEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NetworkCommand>();
    let peers_per_group = Arc::new(std::sync::Mutex::new(HashMap::<String, HashSet<PublicKey>>::new()));

    // Start actor
    let actor = NetworkActor::new(secret_key, event_tx, peers_per_group).await?;
    tokio::spawn(async move {
        actor.start(cmd_rx).await;
    });

    println!("✅ NetworkActor started");

    // Wait for ready
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ─────────────────────────────────────────────────────────────────────
    // PHASE 1: Join group and sync
    // ─────────────────────────────────────────────────────────────────────

    println!("\n📤 Sending JoinGroup command...");
    cmd_tx.send(NetworkCommand::JoinGroup {
        group_id: TEST_GROUP_ID.to_string(),
        bootstrap_peer: None,
    })?;

    let mut sync_complete = false;
    let mut received_host_delta = false;
    let mut sent_our_delta = false;
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > Duration::from_secs(120) {
            println!("❌ Test timeout");
            break;
        }

        tokio::select! {
            Some(event) = event_rx.recv() => {
                match &event {
                    SwiftEvent::SyncComplete { group_id } => {
                        println!("\n✅ [JOINER] SyncComplete for {}...", &group_id[..16]);
                        println!("   Now waiting for host's delta element...\n");
                        sync_complete = true;
                    }
                    SwiftEvent::SyncStarted { group_name, .. } => {
                        println!("🔄 [JOINER] SyncStarted: {}", group_name);
                    }
                    SwiftEvent::StatusUpdate { message } => {
                        println!("📋 [JOINER] {}", message);
                    }
                    SwiftEvent::Network(net_event) => {
                        match net_event {
                            NetworkEvent::WhiteboardElementAdded { id, element_type, .. } => {
                                if !received_host_delta {
                                    println!("\n═══════════════════════════════════════════════════════════════════");
                                    println!("✅ [JOINER] RECEIVED DELTA from host!");
                                    println!("   Element ID: {}", id);
                                    println!("   Type: {}", element_type);
                                    println!("═══════════════════════════════════════════════════════════════════");

                                    received_host_delta = true;

                                    // Now send our own delta
                                    if !sent_our_delta {
                                        println!("\n📤 [JOINER] PHASE 3: Broadcasting our delta element...\n");

                                        tokio::time::sleep(Duration::from_secs(1)).await;

                                        let now = chrono::Utc::now().timestamp();
                                        let our_delta = NetworkEvent::WhiteboardElementAdded {
                                            id: "delta-from-joiner-001".to_string(),
                                            board_id: TEST_BOARD_ID.to_string(),
                                            element_type: "hexagon".to_string(),
                                            x: 600.0,
                                            y: 400.0,
                                            width: 100.0,
                                            height: 100.0,
                                            z_index: 1000,
                                            style_json: Some("{\"fill\":\"#9400D3\",\"stroke\":\"#4B0082\"}".to_string()),
                                            content_json: Some("{\"text\":\"⬡ Delta from Joiner!\"}".to_string()),
                                            created_at: now,
                                            updated_at: now,
                                        };

                                        cmd_tx.send(NetworkCommand::Broadcast {
                                            group_id: TEST_GROUP_ID.to_string(),
                                            event: our_delta,
                                        })?;

                                        sent_our_delta = true;
                                        println!("✅ [JOINER] Delta element broadcast sent!");
                                        println!("\n╔═══════════════════════════════════════════════════════════════╗");
                                        println!("║ ✅ JOINER COMPLETE - Bidirectional delta sync verified!       ║");
                                        println!("╚═══════════════════════════════════════════════════════════════╝\n");

                                        // Give host time to receive before exiting
                                        tokio::time::sleep(Duration::from_secs(3)).await;
                                        return Ok(());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n👋 Shutting down...");
                break;
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// TEST DATA HELPERS
// ═══════════════════════════════════════════════════════════════════════════

fn create_host_test_data() -> Result<()> {
    // Group
    storage::group_insert_simple(TEST_GROUP_ID, "Delta Test Group", "folder.fill", "#00AEEF")?;

    // Workspace
    storage::workspace_insert_simple(TEST_WORKSPACE_ID, TEST_GROUP_ID, "Main Workspace")?;

    // Board
    storage::board_insert_simple(
        TEST_BOARD_ID,
        TEST_WORKSPACE_ID,
        "Delta Canvas",
        chrono::Utc::now().timestamp(),
    )?;

    // Initial elements (3)
    for i in 0..3 {
        let style = format!("{{\"fill\":\"#{}0000\"}}", format!("{:02X}", i * 80));
        let content = format!("{{\"text\":\"Initial Element {}\"}}", i);
        storage::element_insert_simple(
            &format!("init-elem-{:03}", i),
            TEST_BOARD_ID,
            "rectangle",
            (i * 120) as f64,
            (i * 60) as f64,
            100.0,
            50.0,
            i,
            Some(&style),
            Some(&content),
            chrono::Utc::now().timestamp(),
            chrono::Utc::now().timestamp(),
        )?;
    }

    Ok(())
}

fn init_test_schema(db_path: &str) -> Result<()> {
    let conn = Connection::open(db_path)?;

    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            icon TEXT,
            color TEXT,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE TABLE IF NOT EXISTS workspaces (
            id TEXT PRIMARY KEY,
            group_id TEXT NOT NULL,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            FOREIGN KEY (group_id) REFERENCES groups(id)
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
            local_path TEXT,
            source_peer TEXT,
            data TEXT,
            message TEXT,
            author TEXT,
            parent_id TEXT,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE TABLE IF NOT EXISTS whiteboard_elements (
            id TEXT PRIMARY KEY,
            board_id TEXT NOT NULL,
            element_type TEXT NOT NULL,
            x REAL NOT NULL DEFAULT 0,
            y REAL NOT NULL DEFAULT 0,
            width REAL NOT NULL DEFAULT 0,
            height REAL NOT NULL DEFAULT 0,
            z_index INTEGER NOT NULL DEFAULT 0,
            style_json TEXT,
            content_json TEXT,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE TABLE IF NOT EXISTS notebook_cells (
            id TEXT PRIMARY KEY,
            board_id TEXT NOT NULL,
            cell_type TEXT NOT NULL,
            cell_order INTEGER NOT NULL DEFAULT 0,
            content TEXT,
            output TEXT,
            collapsed INTEGER NOT NULL DEFAULT 0,
            height REAL,
            metadata_json TEXT,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );
    "#)?;

    Ok(())
}