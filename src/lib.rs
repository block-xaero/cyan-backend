// lib.rs - Updated with full parity to Swift UI

mod ffi;
mod objects;

use bytemuck::{Pod, Zeroable};
use rkyv::{Archive, Deserialize, Serialize};

// Event type constants matching Swift UI needs
pub const GROUP_EVENT: u32 = 1;
pub const WORKSPACE_EVENT: u32 = 2;
pub const WHITEBOARD_EVENT: u32 = 3;
pub const STICKY_NOTE_EVENT: u32 = 4;
pub const SKETCH_EVENT: u32 = 5;
pub const COMMENT_EVENT: u32 = 6;
pub const LAYER_EVENT: u32 = 7;
pub const DRAWING_PATH_EVENT: u32 = 8;

pub const TOMBSTONE_OFFSET: u32 = 1000;
pub const UPDATE_OFFSET: u32 = 2000;
pub const PIN_FLAG: u32 = 0x80000000;

// Group structure matching Swift's GroupNode
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Group<const MAX_WORKSPACES: usize> {
    pub group_id: [u8; 32],
    pub parent_id: [u8; 32],
    pub name: [u8; 64], // Group name
    pub icon: [u8; 32], // Icon name (e.g., "paintbrush")
    pub color: [u8; 4], // RGBA color
    pub workspace_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32], // XaeroID of creator
    pub workspaces: [[u8; 32]; MAX_WORKSPACES],
    pub _padding: [u8; 23],
}

unsafe impl<const MAX_WORKSPACES: usize> Pod for Group<MAX_WORKSPACES> {}
unsafe impl<const MAX_WORKSPACES: usize> Zeroable for Group<MAX_WORKSPACES> {}

// Workspace structure matching Swift's WorkspaceNode
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Workspace<const MAX_BOARDS: usize> {
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub name: [u8; 64], // Workspace name
    pub board_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32],           // XaeroID of creator
    pub boards: [[u8; 32]; MAX_BOARDS], // Board IDs (whiteboards)
    pub _padding: [u8; 24],
}

unsafe impl<const MAX_BOARDS: usize> Pod for Workspace<MAX_BOARDS> {}
unsafe impl<const MAX_BOARDS: usize> Zeroable for Workspace<MAX_BOARDS> {}

// Board/Whiteboard structure matching Swift's BoardNode
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Board<const MAX_FILES: usize> {
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub name: [u8; 64],
    pub upvotes: u32,
    pub comment_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32],
    pub last_modified_by: [u8; 32],
    pub files: [[u8; 32]; MAX_FILES],
    pub file_count: u32,
    pub _padding: [u8; 20],
}

unsafe impl<const MAX_FILES: usize> Pod for Board<MAX_FILES> {}
unsafe impl<const MAX_FILES: usize> Zeroable for Board<MAX_FILES> {}

// Comment structure for Reddit-style commenting
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Comment<const MAX_TEXT: usize> {
    pub comment_id: [u8; 32],
    pub board_id: [u8; 32],  // Which board this comment belongs to
    pub parent_id: [u8; 32], // Parent comment for threading
    pub author_id: [u8; 32], // XaeroID of author
    pub author_name: [u8; 64],
    pub content: [u8; MAX_TEXT],
    pub upvotes: u32,
    pub downvotes: u32,
    pub depth: u8, // Reply depth (0 for top-level)
    pub is_collapsed: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub _padding: [u8; 6],
}

unsafe impl<const MAX_TEXT: usize> Pod for Comment<MAX_TEXT> {}
unsafe impl<const MAX_TEXT: usize> Zeroable for Comment<MAX_TEXT> {}

// Layer structure for whiteboard layers
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Layer {
    pub layer_id: [u8; 32],
    pub board_id: [u8; 32],
    pub name: [u8; 64],
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32, // 0.0 to 1.0
    pub z_index: u32,
    pub created_at: u64,
    pub _padding: [u8; 50],
}

unsafe impl Pod for Layer {}
unsafe impl Zeroable for Layer {}

// DrawingPath for whiteboard drawings
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct DrawingPath<const MAX_POINTS: usize> {
    pub path_id: [u8; 32],
    pub board_id: [u8; 32],
    pub layer_id: [u8; 32],
    pub path_type: u8,         // 0=freehand, 1=rectangle, 2=circle, etc.
    pub stroke_color: [u8; 4], // RGBA
    pub fill_color: [u8; 4],   // RGBA
    pub stroke_width: f32,
    pub start_point: [f32; 2], // x, y
    pub end_point: [f32; 2],   // x, y
    pub transform: [f32; 6],   // 2D transform matrix
    pub point_count: u32,
    pub points: [[f32; 2]; MAX_POINTS], // Path points for freehand
    pub text: [u8; 256],                // For text/sticky notes
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 3],
}

unsafe impl<const MAX_POINTS: usize> Pod for DrawingPath<MAX_POINTS> {}
unsafe impl<const MAX_POINTS: usize> Zeroable for DrawingPath<MAX_POINTS> {}

// File attachment for boards
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct FileNode {
    pub file_id: [u8; 32],
    pub board_id: [u8; 32],
    pub name: [u8; 256],
    pub file_type: u8, // 0=image, 1=pdf, 2=text, 3=code, 4=data
    pub file_size: u64,
    pub blake_hash: [u8; 32], // Content hash
    pub uploaded_by: [u8; 32],
    pub uploaded_at: u64,
    pub _padding: [u8; 95],
}

unsafe impl Pod for FileNode {}
unsafe impl Zeroable for FileNode {}

// Vote tracking for upvotes/downvotes
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Vote {
    pub vote_id: [u8; 32],
    pub target_id: [u8; 32], // Board or Comment ID
    pub voter_id: [u8; 32],  // XaeroID of voter
    pub vote_type: u8,       // 0=upvote, 1=downvote
    pub created_at: u64,
    pub _padding: [u8; 55],
}

unsafe impl Pod for Vote {}
unsafe impl Zeroable for Vote {}

// Traits for operations
pub trait GroupOps<const MAX_WORKSPACES: usize> {
    fn create_group(
        &mut self,
        creator_id: [u8; 32],
        name: &str,
        icon: &str,
        color: [u8; 4],
    ) -> Result<Group<MAX_WORKSPACES>, Box<dyn std::error::Error>>;

    fn update_group(
        &mut self,
        group_id: [u8; 32],
        name: Option<&str>,
        icon: Option<&str>,
        color: Option<[u8; 4]>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn tombstone_group(&mut self, group_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>>;

    fn list_groups(&self) -> Result<Vec<Group<MAX_WORKSPACES>>, Box<dyn std::error::Error>>;
}

pub trait WorkspaceOps<const MAX_BOARDS: usize> {
    fn create_workspace(
        &mut self,
        creator_id: [u8; 32],
        group_id: [u8; 32],
        name: &str,
    ) -> Result<Workspace<MAX_BOARDS>, Box<dyn std::error::Error>>;

    fn update_workspace(
        &mut self,
        workspace_id: [u8; 32],
        name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn tombstone_workspace(
        &mut self,
        workspace_id: [u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn list_workspaces(
        &self,
        group_id: [u8; 32],
    ) -> Result<Vec<Workspace<MAX_BOARDS>>, Box<dyn std::error::Error>>;
}

pub trait BoardOps<const MAX_FILES: usize> {
    fn create_board(
        &mut self,
        creator_id: [u8; 32],
        workspace_id: [u8; 32],
        group_id: [u8; 32],
        name: &str,
    ) -> Result<Board<MAX_FILES>, Box<dyn std::error::Error>>;

    fn update_board(
        &mut self,
        board_id: [u8; 32],
        name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn tombstone_board(&mut self, board_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>>;

    fn list_boards(
        &self,
        workspace_id: [u8; 32],
    ) -> Result<Vec<Board<MAX_FILES>>, Box<dyn std::error::Error>>;
}

pub trait CommentOps<const MAX_TEXT: usize> {
    fn add_comment(
        &mut self,
        board_id: [u8; 32],
        author_id: [u8; 32],
        author_name: &str,
        content: &str,
        parent_id: Option<[u8; 32]>,
    ) -> Result<Comment<MAX_TEXT>, Box<dyn std::error::Error>>;

    fn upvote_comment(
        &mut self,
        comment_id: [u8; 32],
        voter_id: [u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn downvote_comment(
        &mut self,
        comment_id: [u8; 32],
        voter_id: [u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn list_comments(
        &self,
        board_id: [u8; 32],
    ) -> Result<Vec<Comment<MAX_TEXT>>, Box<dyn std::error::Error>>;
}

pub trait DrawingOps<const MAX_POINTS: usize> {
    fn add_drawing_path(
        &mut self,
        board_id: [u8; 32],
        layer_id: [u8; 32],
        path: DrawingPath<MAX_POINTS>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn update_drawing_path(
        &mut self,
        path_id: [u8; 32],
        transform: Option<[f32; 6]>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn delete_drawing_path(&mut self, path_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>>;

    fn list_drawing_paths(
        &self,
        board_id: [u8; 32],
    ) -> Result<Vec<DrawingPath<MAX_POINTS>>, Box<dyn std::error::Error>>;
}

pub trait LayerOps {
    fn create_layer(
        &mut self,
        board_id: [u8; 32],
        name: &str,
    ) -> Result<Layer, Box<dyn std::error::Error>>;

    fn update_layer(
        &mut self,
        layer_id: [u8; 32],
        name: Option<&str>,
        visible: Option<bool>,
        locked: Option<bool>,
        opacity: Option<f32>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn delete_layer(&mut self, layer_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>>;

    fn list_layers(&self, board_id: [u8; 32]) -> Result<Vec<Layer>, Box<dyn std::error::Error>>;
}

// Helper functions
pub fn is_tombstone_event(event_type: u32) -> bool {
    (event_type & TOMBSTONE_OFFSET) != 0
}

pub fn is_update_event(event_type: u32) -> bool {
    (event_type & UPDATE_OFFSET) != 0
}

pub fn is_pinned_event(event_type: u32) -> bool {
    (event_type & PIN_FLAG) != 0
}
