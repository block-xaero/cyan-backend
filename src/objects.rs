// Object data types sized to fit different payload sizes

use bytemuck::{Pod, Zeroable};
use rusted_ring::{L_TSHIRT_SIZE, M_TSHIRT_SIZE, S_TSHIRT_SIZE, XL_TSHIRT_SIZE};

use crate::objects::file::FileAttachmentData;
// ================================================================================================
// OBJECT TYPE CONSTANTS
// ================================================================================================

pub const WHITEBOARD_TYPE: u8 = 1;
pub const STICKY_NOTE_TYPE: u8 = 2;
pub const SKETCH_TYPE: u8 = 3;
pub const TEXT_BOX_TYPE: u8 = 7;
pub const TEXT_LABEL_TYPE: u8 = 8;
pub const RECTANGLE_TYPE: u8 = 9;
pub const CIRCLE_TYPE: u8 = 10;
pub const CONNECTOR_TYPE: u8 = 11;
pub const CHAT_MESSAGE_TYPE: u8 = 12;
pub const FILE_ATTACHMENT_TYPE: u8 = 13;
pub const IMAGE_TYPE: u8 = 14;
pub const EXCEL_EMBED_TYPE: u8 = 15;
pub const POWERPOINT_SLIDE_TYPE: u8 = 16;
pub const USER_CURSOR_TYPE: u8 = 17;
pub const SELECTION_BOX_TYPE: u8 = 18;

// ================================================================================================
// OBJECT DATA TYPES (WHAT GOES IN THE PAYLOAD FIELD)
// ================================================================================================

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct StickyNoteData {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z_index: u16,
    pub background_color: [u8; 4],
    pub text_color: [u8; 4],
    pub font_size: u16,
    pub text: [u8; 60], // Fits in 96-byte payload
    pub _padding: [u8; 6],
}
unsafe impl Pod for StickyNoteData {}
unsafe impl Zeroable for StickyNoteData {}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct SketchData {
    pub stroke_color: [u8; 4],
    pub stroke_width: f32,
    pub point_count: u32,
    pub points: [(f32, f32); 60],
    pub bounding_box: (f32, f32, f32, f32),
    pub _padding: [u8; 4],
}
unsafe impl Pod for SketchData {}
unsafe impl Zeroable for SketchData {}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct WhiteboardData {
    pub title: [u8; 64],
    pub canvas_width: f32,
    pub canvas_height: f32,
    pub background_color: [u8; 4],
    pub zoom_level: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub grid_enabled: bool,
    pub _padding: [u8; 39],
}
unsafe impl Pod for WhiteboardData {}
unsafe impl Zeroable for WhiteboardData {}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct TextBoxData {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub z_index: u16,
    pub text_color: [u8; 4],
    pub background_color: [u8; 4],
    pub border_color: [u8; 4],
    pub font_size: u16,
    pub font_weight: u8, // 0=normal, 1=bold
    pub text_align: u8,  // 0=left, 1=center, 2=right
    pub text: [u8; 200], // Medium text capacity
    pub _padding: [u8; 18],
}
unsafe impl Pod for TextBoxData {}
unsafe impl Zeroable for TextBoxData {}

/// Shape stuff
pub mod shape {
    use bytemuck::{Pod, Zeroable};

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct RectangleData {
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub rotation: f32,
        pub z_index: u16,
        pub fill_color: [u8; 4],
        pub border_color: [u8; 4],
        pub border_width: f32,
        pub corner_radius: f32,
        pub label_text: [u8; 32],
        pub _padding: [u8; 14],
    }
    unsafe impl Pod for RectangleData {}
    unsafe impl Zeroable for RectangleData {}

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct CircleData {
        pub center_x: f32,
        pub center_y: f32,
        pub radius: f32,
        pub z_index: u16,
        pub fill_color: [u8; 4],
        pub border_color: [u8; 4],
        pub border_width: f32,
        pub label_text: [u8; 32],
        pub _padding: [u8; 18],
    }
    unsafe impl Pod for CircleData {}
    unsafe impl Zeroable for CircleData {}

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct ConnectorData {
        pub from_object_id: [u8; 32],
        pub to_object_id: [u8; 32],
        pub from_point: (f32, f32),
        pub to_point: (f32, f32),
        pub control_points: [(f32, f32); 2], // For curved lines
        pub line_color: [u8; 4],
        pub line_width: f32,
        pub arrow_start: bool,
        pub arrow_end: bool,
        pub line_style: u8, // 0=solid, 1=dashed, 2=dotted
        pub _padding: [u8; 7],
    }
    unsafe impl Pod for ConnectorData {}
    unsafe impl Zeroable for ConnectorData {}
}

/// chat messages
pub mod chat {
    use bytemuck::{Pod, Zeroable};

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct ChatMessageData {
        pub message_id: [u8; 32],
        pub sender_id: [u8; 32],    // XaeroID of sender
        pub workspace_id: [u8; 32], // Workspace
        pub timestamp: u64,
        pub message_type: u8,        // 0=text, 1=system, 2=attachment
        pub reply_to: [u8; 32],      // ID of message being replied to (0 if none)
        pub message_text: [u8; 400], // Main message content
        pub _padding: [u8; 107],
    }
    unsafe impl Pod for ChatMessageData {}
    unsafe impl Zeroable for ChatMessageData {}
}

pub mod file {
    use bytemuck::{Pod, Zeroable};

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct FileAttachmentData {
        pub file_id: [u8; 32],
        pub workspace_id: [u8; 32],
        pub uploaded_by: [u8; 32],
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub rotation: f32,
        pub z_index: u16,
        pub file_type: u8,        // 0=image, 1=pdf, 2=excel, 3=powerpoint, 4=video
        pub file_size: u64,       // Size in bytes
        pub file_name: [u8; 128], // Original filename
        pub file_hash: [u8; 32],  // Blake3 hash of file contents
        pub thumbnail_data: [u8; 4096], // Small preview image
        pub metadata: [u8; 11776], // File-specific metadata
        pub _padding: [u8; 96],
    }
    unsafe impl Pod for FileAttachmentData {}
    unsafe impl Zeroable for FileAttachmentData {}

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct ImageData {
        pub image_id: [u8; 32],
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub rotation: f32,
        pub z_index: u16,
        pub opacity: u8,
        pub image_hash: [u8; 32], // Points to blob storage
        pub original_width: u32,
        pub original_height: u32,
        pub image_format: u8,      // 0=png, 1=jpg, 2=gif, 3=svg
        pub thumbnail: [u8; 8192], // Small preview
        pub alt_text: [u8; 128],   // Accessibility
        pub _padding: [u8; 7979],
    }
    unsafe impl Pod for ImageData {}
    unsafe impl Zeroable for ImageData {}

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct ExcelEmbedData {
        pub embed_id: [u8; 32],
        pub file_hash: [u8; 32],
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub z_index: u16,
        pub sheet_name: [u8; 64],
        pub cell_range: [u8; 32],        // e.g., "A1:D10"
        pub worksheet_data: [u8; 15872], // Subset of spreadsheet data
        pub formula_cache: [u8; 256],    // Cached calculations
        pub _padding: [u8; 78],
    }
    unsafe impl Pod for ExcelEmbedData {}
    unsafe impl Zeroable for ExcelEmbedData {}

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct PowerPointSlideData {
        pub slide_id: [u8; 32],
        pub presentation_hash: [u8; 32],
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub z_index: u16,
        pub slide_number: u32,
        pub slide_title: [u8; 128],
        pub slide_thumbnail: [u8; 8192], // Preview image
        pub slide_notes: [u8; 512],      // Speaker notes
        pub animation_data: [u8; 7168],  // Slide animations
        pub _padding: [u8; 78],
    }
    unsafe impl Pod for PowerPointSlideData {}
    unsafe impl Zeroable for PowerPointSlideData {}
}

pub mod user_presence {
    use bytemuck::{Pod, Zeroable};

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct UserCursorData {
        pub user_id: [u8; 32],
        pub cursor_x: f32,
        pub cursor_y: f32,
        pub cursor_color: [u8; 4],
        pub user_name: [u8; 32],
        pub is_active: bool,
        pub last_seen: u64,
        pub current_tool: u8, // 0=select, 1=pen, 2=text, etc.
        pub _padding: [u8; 171],
    }
    unsafe impl Pod for UserCursorData {}
    unsafe impl Zeroable for UserCursorData {}

    #[repr(C)]
    #[derive(Copy, Clone, Debug)]
    pub struct SelectionBoxData {
        pub user_id: [u8; 32],
        pub start_x: f32,
        pub start_y: f32,
        pub end_x: f32,
        pub end_y: f32,
        pub selection_color: [u8; 4],
        pub selected_objects: [[u8; 32]; 4], // Up to 4 selected object IDs
        pub _padding: [u8; 80],
    }
    unsafe impl Pod for SelectionBoxData {}
    unsafe impl Zeroable for SelectionBoxData {}
}

// ================================================================================================
// COMPILE-TIME SIZE VALIDATION
// ================================================================================================

const _: () = assert!(std::mem::size_of::<WhiteboardData>() <= 128);

const _: () = assert!(std::mem::size_of::<StickyNoteData>() <= S_TSHIRT_SIZE);
const _: () = assert!(std::mem::size_of::<SketchData>() <= M_TSHIRT_SIZE);
const _: () = assert!(std::mem::size_of::<WhiteboardData>() <= L_TSHIRT_SIZE);
const _: () = assert!(std::mem::size_of::<FileAttachmentData>() <= XL_TSHIRT_SIZE);
