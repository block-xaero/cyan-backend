use crate::models::events::NetworkEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum NetworkCommand {
    /// Requests group snapshot
    RequestSnapshot {
        from_peer: String,
    },
    JoinGroup {
        group_id: String,
        bootstrap_peer: Option<String>,  // Node ID of peer to bootstrap from (e.g., inviter)
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
    /// Request file download from peer (with resume support)
    RequestFileDownload {
        file_id: String,
        hash: String,
        source_peer: String,
        resume_offset: u64,  // 0 for new, >0 for resume
    },
    /// Resume all pending file transfers
    ResumePendingTransfers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CommandMsg {
    // ═══════════════════════════════════════════════════════════════════════
    // GROUP COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
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

    // ═══════════════════════════════════════════════════════════════════════
    // WORKSPACE COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
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

    // ═══════════════════════════════════════════════════════════════════════
    // BOARD COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
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

    // ═══════════════════════════════════════════════════════════════════════
    // CHAT COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
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

    // ═══════════════════════════════════════════════════════════════════════
    // DIRECT MESSAGE COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
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

    // ═══════════════════════════════════════════════════════════════════════
    // WHITEBOARD ELEMENT COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
    CreateWhiteboardElement {
        board_id: String,
        element_type: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        z_index: i32,
        style_json: Option<String>,
        content_json: Option<String>,
    },
    UpdateWhiteboardElement {
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
    },
    DeleteWhiteboardElement {
        id: String,
        board_id: String,
    },
    ClearWhiteboard {
        board_id: String,
    },

    // ═══════════════════════════════════════════════════════════════════════
    // NOTEBOOK CELL COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
    AddNotebookCell {
        board_id: String,
        cell_type: String,
        cell_order: i32,
        content: Option<String>,
    },
    UpdateNotebookCell {
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
    DeleteNotebookCell {
        id: String,
        board_id: String,
    },
    ReorderNotebookCells {
        board_id: String,
        cell_ids: Vec<String>,
    },

    // ═══════════════════════════════════════════════════════════════════════
    // BOARD METADATA COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
    UpdateBoardMetadata {
        board_id: String,
        labels: Vec<String>,
        rating: i32,
        view_count: i32,
        contains_model: Option<String>,
        contains_skills: Vec<String>,
        board_type: Option<String>,
        last_accessed: Option<i64>,
        is_pinned: bool,
    },
    IncrementBoardViewCount {
        board_id: String,
    },
    SetBoardPinned {
        board_id: String,
        is_pinned: bool,
    },

    // ═══════════════════════════════════════════════════════════════════════
    // INTEGRATION COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
    AddIntegration {
        scope_type: String,
        scope_id: String,
        integration_type: String,
        config: serde_json::Value,
    },
    RemoveIntegration {
        id: String,
    },

    // ═══════════════════════════════════════════════════════════════════════
    // PROFILE COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
    UpdateProfile {
        display_name: String,
        avatar_hash: Option<String>,
    },

    // ═══════════════════════════════════════════════════════════════════════
    // SYSTEM COMMANDS
    // ═══════════════════════════════════════════════════════════════════════
    Snapshot {},
    SeedDemoIfEmpty,
}