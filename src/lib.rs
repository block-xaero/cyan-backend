// lib.rs - Complete with all structures and sensible sizes
#![feature(generic_const_exprs)]
#![allow(incomplete_features)]
mod ffi;
mod objects;
mod storage;

use bytemuck::{Pod, Zeroable};
use rkyv::{Archive, Deserialize, Serialize};
use xaeroflux_actors::XaeroFlux;
use xaeroid::XaeroID;

use crate::storage::DATA_DIR;

// Constants for text/content sizes only
pub const COMMENT_MAX_TEXT: usize = 512;      // 512 bytes for comment text
pub const PATH_MAX_POINTS: usize = 256;       // 256 points for drawing paths

// Standard types used everywhere (no more generics!)
pub type GroupStandard = Group;
pub type WorkspaceStandard = Workspace;
pub type BoardStandard = Board;
pub type CommentStandard = Comment;
pub type DrawingPathStandard = DrawingPath<PATH_MAX_POINTS>;

// Event type constants
pub const EVENT_TYPE_GROUP: u32 = 100;
pub const EVENT_TYPE_WORKSPACE: u32 = 101;
pub const EVENT_TYPE_BOARD: u32 = 102;
pub const EVENT_TYPE_COMMENT: u32 = 103;
pub const EVENT_TYPE_WHITEBOARD_OBJECT: u32 = 104;
pub const EVENT_TYPE_INVITATION: u32 = 105;
pub const EVENT_TYPE_LAYER: u32 = 106;
pub const EVENT_TYPE_VOTE: u32 = 107;
pub const EVENT_TYPE_FILE: u32 = 108;

pub const TOMBSTONE_OFFSET: u32 = 1000;
pub const UPDATE_OFFSET: u32 = 2000;
pub const PIN_FLAG: u32 = 0x80000000;

pub fn initialize(xaero_id: XaeroID) {
    XaeroFlux::initialize(xaero_id, DATA_DIR)
        .expect("Cyan App failed to initialize due to Xaeroflux failure!");
}

// Group structure - 256 bytes
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Group {
    pub group_id: [u8; 32],
    pub parent_id: [u8; 32],
    pub name: [u8; 64],
    pub icon: [u8; 32],
    pub color: [u8; 4],
    pub workspace_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 28],
}

unsafe impl Pod for Group {}
unsafe impl Zeroable for Group {}

// Workspace structure - 256 bytes (no pre-allocated boards!)
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Workspace {
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub name: [u8; 64],
    pub board_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 56],
}

unsafe impl Pod for Workspace {}
unsafe impl Zeroable for Workspace {}

// Board structure - 320 bytes (no pre-allocated files!)
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Board {
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub name: [u8; 64],
    pub upvotes: u32,
    pub comment_count: u32,
    pub file_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32],
    pub last_modified_by: [u8; 32],
    pub _padding: [u8; 48],
}

unsafe impl Pod for Board {}
unsafe impl Zeroable for Board {}

// Comment structure - 768 bytes (includes 512 bytes of text)
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Comment {
    pub comment_id: [u8; 32],
    pub board_id: [u8; 32],
    pub parent_id: [u8; 32],
    pub author_id: [u8; 32],
    pub author_name: [u8; 64],
    pub content: [u8; 512],  // Fixed size for text content
    pub upvotes: u32,
    pub downvotes: u32,
    pub depth: u8,
    pub is_collapsed: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub _padding: [u8; 6],
}

unsafe impl Pod for Comment {}
unsafe impl Zeroable for Comment {}

// Layer structure - 192 bytes
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Layer {
    pub layer_id: [u8; 32],
    pub board_id: [u8; 32],
    pub name: [u8; 64],
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub z_index: u32,
    pub created_at: u64,
    pub _padding: [u8; 46],
}

unsafe impl Pod for Layer {}
unsafe impl Zeroable for Layer {}

// DrawingPath - 2624 bytes with MAX_POINTS=256
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct DrawingPath<const MAX_POINTS: usize> {
    pub path_id: [u8; 32],
    pub board_id: [u8; 32],
    pub layer_id: [u8; 32],
    pub path_type: u8,
    pub stroke_color: [u8; 4],
    pub fill_color: [u8; 4],
    pub stroke_width: f32,
    pub start_point: [f32; 2],
    pub end_point: [f32; 2],
    pub transform: [f32; 6],
    pub point_count: u32,
    pub points: [[f32; 2]; MAX_POINTS],
    pub text: [u8; 256],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 3],
}

unsafe impl<const MAX_POINTS: usize> Pod for DrawingPath<MAX_POINTS> {}
unsafe impl<const MAX_POINTS: usize> Zeroable for DrawingPath<MAX_POINTS> {}

// File node - 448 bytes
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct FileNode {
    pub file_id: [u8; 32],
    pub board_id: [u8; 32],
    pub name: [u8; 256],
    pub file_type: u8,
    pub file_size: u64,
    pub blake_hash: [u8; 32],
    pub uploaded_by: [u8; 32],
    pub uploaded_at: u64,
    pub _padding: [u8; 95],
}

unsafe impl Pod for FileNode {}
unsafe impl Zeroable for FileNode {}

// Vote - 128 bytes
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Vote {
    pub vote_id: [u8; 32],
    pub target_id: [u8; 32],
    pub voter_id: [u8; 32],
    pub vote_type: u8,
    pub created_at: u64,
    pub _padding: [u8; 23],
}

unsafe impl Pod for Vote {}
unsafe impl Zeroable for Vote {}

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