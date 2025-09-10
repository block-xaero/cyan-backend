// objects.rs - Complete with hierarchical support

use bytemuck::{Pod, Zeroable};

// ================================================================================================
// CANVAS MODE TYPES (matching Swift's CanvasMode enum)
// ================================================================================================

pub const CANVAS_MODE_SELECT: u8 = 0;
pub const CANVAS_MODE_DRAW: u8 = 1;
pub const CANVAS_MODE_RECTANGLE: u8 = 2;
pub const CANVAS_MODE_CIRCLE: u8 = 3;
pub const CANVAS_MODE_ARROW: u8 = 4;
pub const CANVAS_MODE_ERASER: u8 = 5;
pub const CANVAS_MODE_TEXT: u8 = 6;
pub const CANVAS_MODE_STICKY: u8 = 7;
pub const CANVAS_MODE_MATH_SYMBOL: u8 = 8;
pub const CANVAS_MODE_FRACTION: u8 = 9;
pub const CANVAS_MODE_GRAPH: u8 = 10;
pub const CANVAS_MODE_STENCIL: u8 = 11;

// ================================================================================================
// OBJECT TYPES
// ================================================================================================

pub const OBJECT_TYPE_PATH: u8 = 1;
pub const OBJECT_TYPE_STICKY: u8 = 2;
pub const OBJECT_TYPE_RECTANGLE: u8 = 3;
pub const OBJECT_TYPE_CIRCLE: u8 = 4;
pub const OBJECT_TYPE_ARROW: u8 = 5;
pub const OBJECT_TYPE_TEXT: u8 = 6;
pub const OBJECT_TYPE_MATH: u8 = 7;
pub const OBJECT_TYPE_GRAPH: u8 = 8;
pub const OBJECT_TYPE_FILE: u8 = 9;
pub const OBJECT_TYPE_COMMENT: u8 = 10;

// ================================================================================================
// PATH TYPES (matching Swift's PathType enum)
// ================================================================================================

pub const PATH_TYPE_FREEHAND: u8 = 0;
pub const PATH_TYPE_RECTANGLE: u8 = 1;
pub const PATH_TYPE_CIRCLE: u8 = 2;
pub const PATH_TYPE_ARROW: u8 = 3;
pub const PATH_TYPE_TEXT: u8 = 4;
pub const PATH_TYPE_STICKY: u8 = 5;
pub const PATH_TYPE_MATH_SYMBOL: u8 = 6;
pub const PATH_TYPE_FRACTION: u8 = 7;
pub const PATH_TYPE_STENCIL: u8 = 8;

// ================================================================================================
// STENCIL TYPES (matching Swift's StencilType enum)
// ================================================================================================

pub const STENCIL_FLOWCHART_BOX: u8 = 0;
pub const STENCIL_FLOWCHART_DIAMOND: u8 = 1;
pub const STENCIL_FLOWCHART_CIRCLE: u8 = 2;
pub const STENCIL_CLOUD_BUBBLE: u8 = 3;
pub const STENCIL_STAR: u8 = 4;
pub const STENCIL_HEXAGON: u8 = 5;

// ================================================================================================
// HIERARCHICAL EVENT TRAIT
// ================================================================================================

pub trait HierarchicalEvent {
    fn get_id(&self) -> [u8; 32];
    fn get_parent_id(&self) -> Option<[u8; 32]>;
    fn get_ancestry(&self) -> Vec<[u8; 32]> {
        let mut chain = vec![self.get_id()];
        if let Some(parent) = self.get_parent_id() {
            chain.push(parent);
        }
        chain
    }
}

// ================================================================================================
// DRAWING PATH DATA (matching Swift's DrawnPath) - Size L (4096 bytes)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct PathData<const MAX_POINTS: usize> {
    pub path_id: [u8; 32],
    pub board_id: [u8; 32],      // parent
    pub workspace_id: [u8; 32],   // grandparent
    pub group_id: [u8; 32],       // great-grandparent
    pub layer_index: u32,
    pub path_type: u8,
    pub stencil_type: u8,
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
    pub _padding: [u8; 2],
}

unsafe impl<const MAX_POINTS: usize> Pod for PathData<MAX_POINTS> {}
unsafe impl<const MAX_POINTS: usize> Zeroable for PathData<MAX_POINTS> {}

impl<const MAX_POINTS: usize> HierarchicalEvent for PathData<MAX_POINTS> {
    fn get_id(&self) -> [u8; 32] { self.path_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

// ================================================================================================
// STICKY NOTE DATA - Size S (256 bytes)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct StickyNoteData {
    pub note_id: [u8; 32],
    pub board_id: [u8; 32],       // parent
    pub workspace_id: [u8; 32],   // grandparent
    pub group_id: [u8; 32],       // great-grandparent
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z_index: u32,
    pub background_color: [u8; 4],
    pub text_color: [u8; 4],
    pub font_size: u16,
    pub text: [u8; 80],           // Reduced to fit in S size
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 6],
}

unsafe impl Pod for StickyNoteData {}
unsafe impl Zeroable for StickyNoteData {}

impl HierarchicalEvent for StickyNoteData {
    fn get_id(&self) -> [u8; 32] { self.note_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

// ================================================================================================
// SHAPE DATA - Size S (256 bytes)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct RectangleData {
    pub shape_id: [u8; 32],
    pub board_id: [u8; 32],       // parent
    pub workspace_id: [u8; 32],   // grandparent
    pub group_id: [u8; 32],       // great-grandparent
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
    fn get_id(&self) -> [u8; 32] { self.shape_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct CircleData {
    pub shape_id: [u8; 32],
    pub board_id: [u8; 32],       // parent
    pub workspace_id: [u8; 32],   // grandparent
    pub group_id: [u8; 32],       // great-grandparent
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
    fn get_id(&self) -> [u8; 32] { self.shape_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct ArrowData {
    pub arrow_id: [u8; 32],
    pub board_id: [u8; 32],       // parent
    pub workspace_id: [u8; 32],   // grandparent
    pub group_id: [u8; 32],       // great-grandparent
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
    fn get_id(&self) -> [u8; 32] { self.arrow_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

// ================================================================================================
// COMMENT DATA - Size M (512 bytes)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct CommentData {
    pub comment_id: [u8; 32],
    pub board_id: [u8; 32],       // parent (board being commented on)
    pub parent_comment_id: [u8; 32], // parent comment (for threading)
    pub workspace_id: [u8; 32],   // grandparent
    pub group_id: [u8; 32],       // great-grandparent
    pub author_id: [u8; 32],
    pub author_name: [u8; 64],
    pub content: [u8; 200],       // Adjusted for size
    pub upvotes: u32,
    pub downvotes: u32,
    pub depth: u8,
    pub is_collapsed: bool,
    pub created_at: u64,
    pub _padding: [u8; 14],
}

unsafe impl Pod for CommentData {}
unsafe impl Zeroable for CommentData {}

impl HierarchicalEvent for CommentData {
    fn get_id(&self) -> [u8; 32] { self.comment_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> {
        if self.parent_comment_id != [0u8; 32] {
            Some(self.parent_comment_id)
        } else {
            Some(self.board_id)
        }
    }
}