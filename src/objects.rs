// objects.rs - Complete with all whiteboard objects and HierarchicalEvent implementations
use bytemuck::{Pod, Zeroable};

use crate::DrawingPath;

// Canvas mode constants
pub const CANVAS_MODE_SELECT: u8 = 0;
pub const CANVAS_MODE_DRAW: u8 = 1;
pub const CANVAS_MODE_RECTANGLE: u8 = 2;
pub const CANVAS_MODE_CIRCLE: u8 = 3;
pub const CANVAS_MODE_ARROW: u8 = 4;
pub const CANVAS_MODE_ERASER: u8 = 5;
pub const CANVAS_MODE_TEXT: u8 = 6;
pub const CANVAS_MODE_STICKY: u8 = 7;

// Object type constants
pub const OBJECT_TYPE_PATH: u8 = 1;
pub const OBJECT_TYPE_STICKY: u8 = 2;
pub const OBJECT_TYPE_RECTANGLE: u8 = 3;
pub const OBJECT_TYPE_CIRCLE: u8 = 4;
pub const OBJECT_TYPE_ARROW: u8 = 5;
pub const OBJECT_TYPE_TEXT: u8 = 6;

// Hierarchical event trait
pub trait HierarchicalEvent {
    fn get_id(&self) -> [u8; 32];
    fn get_parent_id(&self) -> Option<[u8; 32]>;
}

// Path data - 2368 bytes with 256 points
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct PathData<const MAX_POINTS: usize> {
    pub path_id: [u8; 32],
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub layer_index: u32,
    pub path_type: u8,
    pub stroke_color: [u8; 4],
    pub fill_color: [u8; 4],
    pub stroke_width: f32,
    pub start_point: [f32; 2],
    pub end_point: [f32; 2],
    pub transform: [f32; 2],
    pub text: [u8; 256],
    pub point_count: u32,
    pub points: [[f32; 2]; MAX_POINTS],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 3],
}

unsafe impl<const MAX_POINTS: usize> Pod for PathData<MAX_POINTS> {}
unsafe impl<const MAX_POINTS: usize> Zeroable for PathData<MAX_POINTS> {}

impl<const MAX_POINTS: usize> HierarchicalEvent for PathData<MAX_POINTS> {
    fn get_id(&self) -> [u8; 32] {
        self.path_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}

// Sticky note - 512 bytes
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct StickyNoteData {
    pub note_id: [u8; 32],
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z_index: u32,
    pub background_color: [u8; 4],
    pub text_color: [u8; 4],
    pub font_size: u16,
    pub text: [u8; 256],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 54],
}

unsafe impl Pod for StickyNoteData {}
unsafe impl Zeroable for StickyNoteData {}

impl HierarchicalEvent for StickyNoteData {
    fn get_id(&self) -> [u8; 32] {
        self.note_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}

// Rectangle - 256 bytes
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct RectangleData {
    pub shape_id: [u8; 32],
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub stroke_color: [u8; 4],
    pub fill_color: [u8; 4],
    pub stroke_width: f32,
    pub corner_radius: f32,
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 56],
}

unsafe impl Pod for RectangleData {}
unsafe impl Zeroable for RectangleData {}

impl HierarchicalEvent for RectangleData {
    fn get_id(&self) -> [u8; 32] {
        self.shape_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}

// Circle - 256 bytes
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct CircleData {
    pub shape_id: [u8; 32],
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
    pub stroke_color: [u8; 4],
    pub fill_color: [u8; 4],
    pub stroke_width: f32,
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 68],
}

unsafe impl Pod for CircleData {}
unsafe impl Zeroable for CircleData {}

impl HierarchicalEvent for CircleData {
    fn get_id(&self) -> [u8; 32] {
        self.shape_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}

// Arrow - 256 bytes
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct ArrowData {
    pub arrow_id: [u8; 32],
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub start_x: f32,
    pub start_y: f32,
    pub end_x: f32,
    pub end_y: f32,
    pub stroke_color: [u8; 4],
    pub stroke_width: f32,
    pub arrow_head_length: f32,
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 64],
}

unsafe impl Pod for ArrowData {}
unsafe impl Zeroable for ArrowData {}

impl HierarchicalEvent for ArrowData {
    fn get_id(&self) -> [u8; 32] {
        self.arrow_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}

// Text object - 512 bytes
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct TextData {
    pub text_id: [u8; 32],
    pub board_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub font_size: u16,
    pub font_family: [u8; 32],
    pub text_color: [u8; 4],
    pub text: [u8; 256],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 46],
}

unsafe impl Pod for TextData {}
unsafe impl Zeroable for TextData {}

impl HierarchicalEvent for TextData {
    fn get_id(&self) -> [u8; 32] {
        self.text_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}

impl<const N: usize> HierarchicalEvent for DrawingPath<N> {
    fn get_id(&self) -> [u8; 32] {
        self.path_id
    }

    fn get_parent_id(&self) -> Option<[u8; 32]> {
        Some(self.board_id)
    }
}
