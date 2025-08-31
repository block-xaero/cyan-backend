// objects.rs - Updated with parity to Swift WhiteboardView

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
// DRAWING PATH DATA (matching Swift's DrawnPath)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct PathData<const MAX_POINTS: usize> {
    pub path_id: [u8; 32],
    pub board_id: [u8; 32],
    pub layer_index: u32,
    pub path_type: u8,
    pub stencil_type: u8,      // Only used if path_type is STENCIL
    pub stroke_color: [u8; 4], // RGBA
    pub fill_color: [u8; 4],   // RGBA
    pub stroke_width: f32,
    pub start_point: [f32; 2], // x, y
    pub end_point: [f32; 2],   // x, y
    pub transform: [f32; 2],   // Translation x, y
    pub text: [u8; 256],       // For text/sticky/math content
    pub point_count: u32,
    pub points: [[f32; 2]; MAX_POINTS], // For freehand paths
    pub created_at: u64,
    pub created_by: [u8; 32], // XaeroID
    pub _padding: [u8; 2],
}

unsafe impl<const MAX_POINTS: usize> Pod for PathData<MAX_POINTS> {}
unsafe impl<const MAX_POINTS: usize> Zeroable for PathData<MAX_POINTS> {}

// ================================================================================================
// LAYER DATA (matching Swift's CanvasLayer)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct LayerData {
    pub layer_id: [u8; 32],
    pub board_id: [u8; 32],
    pub name: [u8; 64],
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32, // 0.0 to 1.0
    pub z_index: u32,
    pub object_count: u32, // Number of objects in this layer
    pub created_at: u64,
    pub _padding: [u8; 42],
}

unsafe impl Pod for LayerData {}
unsafe impl Zeroable for LayerData {}

// ================================================================================================
// WHITEBOARD DATA (matching Swift's WhiteboardView state)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct WhiteboardData {
    pub board_id: [u8; 32],
    pub title: [u8; 128],
    pub canvas_width: f32,
    pub canvas_height: f32,
    pub background_color: [u8; 4],
    pub zoom_level: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub grid_enabled: bool,
    pub grid_size: f32,
    pub current_layer: u32,
    pub total_layers: u32,
    pub total_objects: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 39],
}

unsafe impl Pod for WhiteboardData {}
unsafe impl Zeroable for WhiteboardData {}

// ================================================================================================
// STICKY NOTE DATA (matching Swift's sticky note functionality)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct StickyNoteData {
    pub note_id: [u8; 32],
    pub board_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z_index: u32,
    pub background_color: [u8; 4], // Default yellow
    pub text_color: [u8; 4],       // Default black
    pub font_size: u16,
    pub text: [u8; 400], // Note content
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 6],
}

unsafe impl Pod for StickyNoteData {}
unsafe impl Zeroable for StickyNoteData {}

// ================================================================================================
// TEXT BOX DATA (matching Swift's text functionality)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct TextBoxData {
    pub text_id: [u8; 32],
    pub board_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub text_color: [u8; 4],
    pub font_size: u16,
    pub font_weight: u8, // 0=normal, 1=bold
    pub text_align: u8,  // 0=left, 1=center, 2=right
    pub text: [u8; 256],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 92],
}

unsafe impl Pod for TextBoxData {}
unsafe impl Zeroable for TextBoxData {}

// ================================================================================================
// MATH SYMBOL DATA
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct MathSymbolData {
    pub symbol_id: [u8; 32],
    pub board_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub symbol: [u8; 8], // UTF-8 encoded math symbol
    pub font_size: u16,
    pub color: [u8; 4],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 114],
}

unsafe impl Pod for MathSymbolData {}
unsafe impl Zeroable for MathSymbolData {}

// ================================================================================================
// SHAPE DATA (matching Swift's shape tools)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct RectangleData {
    pub shape_id: [u8; 32],
    pub board_id: [u8; 32],
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
    pub _padding: [u8; 88],
}

unsafe impl Pod for RectangleData {}
unsafe impl Zeroable for RectangleData {}

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct CircleData {
    pub shape_id: [u8; 32],
    pub board_id: [u8; 32],
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
    pub stroke_color: [u8; 4],
    pub fill_color: [u8; 4],
    pub stroke_width: f32,
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 100],
}

unsafe impl Pod for CircleData {}
unsafe impl Zeroable for CircleData {}

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct ArrowData {
    pub arrow_id: [u8; 32],
    pub board_id: [u8; 32],
    pub start_x: f32,
    pub start_y: f32,
    pub end_x: f32,
    pub end_y: f32,
    pub stroke_color: [u8; 4],
    pub stroke_width: f32,
    pub arrow_head_length: f32,
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 96],
}

unsafe impl Pod for ArrowData {}
unsafe impl Zeroable for ArrowData {}

// ================================================================================================
// COMMENT DATA (matching Swift's Reddit-style comments)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct CommentData {
    pub comment_id: [u8; 32],
    pub board_id: [u8; 32],
    pub parent_id: [u8; 32], // For threading (0 if top-level)
    pub author_id: [u8; 32], // XaeroID
    pub author_name: [u8; 64],
    pub content: [u8; 500],
    pub upvotes: u32,
    pub downvotes: u32,
    pub depth: u8, // Reply depth
    pub is_collapsed: bool,
    pub has_upvoted: bool, // Current user's vote state
    pub has_downvoted: bool,
    pub created_at: u64,
    pub _padding: [u8; 16],
}

unsafe impl Pod for CommentData {}
unsafe impl Zeroable for CommentData {}

// ================================================================================================
// FILE ATTACHMENT DATA (matching Swift's FileNode)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct FileAttachmentData {
    pub file_id: [u8; 32],
    pub board_id: [u8; 32],
    pub name: [u8; 256],
    pub file_type: u8, // 0=image, 1=pdf, 2=text, 3=code, 4=data
    pub file_size: u64,
    pub blake_hash: [u8; 32], // Content hash
    pub color: [u8; 4],       // Display color based on type
    pub icon: [u8; 32],       // Icon name for display
    pub uploaded_by: [u8; 32],
    pub uploaded_at: u64,
    pub _padding: [u8; 59],
}

unsafe impl Pod for FileAttachmentData {}
unsafe impl Zeroable for FileAttachmentData {}

// ================================================================================================
// USER PRESENCE DATA (for collaborative features)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct UserCursorData {
    pub user_id: [u8; 32],
    pub board_id: [u8; 32],
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_color: [u8; 4],
    pub user_name: [u8; 64],
    pub is_active: bool,
    pub current_tool: u8, // Canvas mode
    pub last_seen: u64,
    pub _padding: [u8; 106],
}

unsafe impl Pod for UserCursorData {}
unsafe impl Zeroable for UserCursorData {}

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct SelectionData {
    pub user_id: [u8; 32],
    pub board_id: [u8; 32],
    pub selected_objects: [[u8; 32]; 10], // Up to 10 selected object IDs
    pub selection_count: u32,
    pub _padding: [u8; 60],
}

unsafe impl Pod for SelectionData {}
unsafe impl Zeroable for SelectionData {}

// ================================================================================================
// GRAPH DATA (for graph tool)
// ================================================================================================

#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct GraphData {
    pub graph_id: [u8; 32],
    pub board_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub equation: [u8; 128], // Math equation string
    pub color: [u8; 4],
    pub created_at: u64,
    pub created_by: [u8; 32],
    pub _padding: [u8; 44],
}

unsafe impl Pod for GraphData {}
unsafe impl Zeroable for GraphData {}
