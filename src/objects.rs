// objects.rs - Fixed with correct sizes
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

// Rectangle - 256 bytes (204 data + auto padding to 256)
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct RectangleData {
    pub shape_id: [u8; 32],      // 32
    pub board_id: [u8; 32],      // 32
    pub workspace_id: [u8; 32],  // 32
    pub group_id: [u8; 32],      // 32 = 128 total
    pub x: f32,                  // 4
    pub y: f32,                  // 4
    pub width: f32,              // 4
    pub height: f32,             // 4 = 16 total
    pub rotation: f32,           // 4
    pub stroke_color: [u8; 4],   // 4
    pub fill_color: [u8; 4],     // 4
    pub stroke_width: f32,       // 4
    pub corner_radius: f32,      // 4 = 20 total
    pub created_at: u64,         // 8
    pub created_by: [u8; 32],    // 32
    pub _padding: [u8; 52],      // 52 to make 256 total
}

unsafe impl Zeroable for RectangleData {}
unsafe impl Pod for RectangleData {}

// Circle - 256 bytes (196 data + 60 padding)
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct CircleData {
    pub shape_id: [u8; 32],      // 32
    pub board_id: [u8; 32],      // 32
    pub workspace_id: [u8; 32],  // 32
    pub group_id: [u8; 32],      // 32 = 128 total
    pub center_x: f32,           // 4
    pub center_y: f32,           // 4
    pub radius: f32,             // 4 = 12 total
    pub stroke_color: [u8; 4],   // 4
    pub fill_color: [u8; 4],     // 4
    pub stroke_width: f32,       // 4 = 12 total
    pub created_at: u64,         // 8
    pub created_by: [u8; 32],    // 32
    pub _padding: [u8; 60],      // 60 to make 256 total
}

unsafe impl Zeroable for CircleData {}
unsafe impl Pod for CircleData {}

// Arrow - 256 bytes (192 data + 64 padding)
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct ArrowData {
    pub arrow_id: [u8; 32],       // 32
    pub board_id: [u8; 32],       // 32
    pub workspace_id: [u8; 32],   // 32
    pub group_id: [u8; 32],       // 32 = 128 total
    pub start_x: f32,             // 4
    pub start_y: f32,             // 4
    pub end_x: f32,               // 4
    pub end_y: f32,               // 4 = 16 total
    pub stroke_color: [u8; 4],    // 4
    pub stroke_width: f32,        // 4
    pub arrow_head_length: f32,   // 4 = 12 total
    pub created_at: u64,          // 8
    pub created_by: [u8; 32],     // 32
    pub _padding: [u8; 64],       // 64 to make 256 total
}

unsafe impl Zeroable for ArrowData {}
unsafe impl Pod for ArrowData {}

// Sticky note - 512 bytes (458 data + 54 padding)
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct StickyNoteData {
    pub note_id: [u8; 32],         // 32
    pub board_id: [u8; 32],        // 32
    pub workspace_id: [u8; 32],    // 32
    pub group_id: [u8; 32],        // 32 = 128 total
    pub x: f32,                    // 4
    pub y: f32,                    // 4
    pub width: f32,                // 4
    pub height: f32,               // 4 = 16 total
    pub rotation: f32,             // 4
    pub z_index: u32,              // 4
    pub background_color: [u8; 4], // 4
    pub text_color: [u8; 4],       // 4 = 16 total
    pub font_size: u16,            // 2
    pub text: [u8; 256],           // 256
    pub created_at: u64,           // 8
    pub created_by: [u8; 32],      // 32
    pub _padding: [u8; 54],        // 54 to make 512 total
}
unsafe impl Zeroable for StickyNoteData {}
unsafe impl Pod for StickyNoteData {}
// Text object - 512 bytes (466 data + 46 padding)
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct TextData {
    pub text_id: [u8; 32],      // 32
    pub board_id: [u8; 32],     // 32
    pub workspace_id: [u8; 32], // 32
    pub group_id: [u8; 32],     // 32 = 128 total
    pub x: f32,                 // 4
    pub y: f32,                 // 4
    pub width: f32,             // 4
    pub height: f32,            // 4 = 16 total
    pub font_size: u16,         // 2
    pub font_family: [u8; 32],  // 32
    pub text_color: [u8; 4],    // 4
    pub text: [u8; 256],        // 256
    pub created_at: u64,        // 8
    pub created_by: [u8; 32],   // 32
    pub _padding: [u8; 46],     // 46 to make 512 total
}
unsafe impl Zeroable for TextData {}
unsafe impl Pod for TextData {}

// Path data - 2432 bytes with 256 points
#[repr(C, align(64))]
#[derive(Copy, Clone, Debug)]
pub struct PathData<const MAX_POINTS: usize> {
    pub path_id: [u8; 32],         // 32
    pub board_id: [u8; 32],        // 32
    pub workspace_id: [u8; 32],    // 32
    pub group_id: [u8; 32],        // 32 = 128 total
    pub layer_index: u32,          // 4
    pub path_type: u8,             // 1
    pub stroke_color: [u8; 4],     // 4
    pub fill_color: [u8; 4],       // 4
    pub stroke_width: f32,         // 4
    pub start_point: [f32; 2],     // 8
    pub end_point: [f32; 2],       // 8
    pub transform: [f32; 2],       // 8
    pub text: [u8; 256],           // 256
    pub point_count: u32,          // 4
    pub points: [[f32; 2]; MAX_POINTS], // 256 * 8 = 2048
    pub created_at: u64,           // 8
    pub created_by: [u8; 32],      // 32
    pub _padding: [u8; 3],         // 3 for alignment
}

unsafe impl<const MAX_POINTS:usize > Zeroable for PathData<MAX_POINTS> {}
unsafe impl<const MAX_POINTS:usize > Pod for PathData<MAX_POINTS> {}
// Implement HierarchicalEvent for all types
impl HierarchicalEvent for RectangleData {
    fn get_id(&self) -> [u8; 32] { self.shape_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

impl HierarchicalEvent for CircleData {
    fn get_id(&self) -> [u8; 32] { self.shape_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

impl HierarchicalEvent for ArrowData {
    fn get_id(&self) -> [u8; 32] { self.arrow_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

impl HierarchicalEvent for StickyNoteData {
    fn get_id(&self) -> [u8; 32] { self.note_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

impl HierarchicalEvent for TextData {
    fn get_id(&self) -> [u8; 32] { self.text_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

impl<const MAX_POINTS: usize> HierarchicalEvent for PathData<MAX_POINTS> {
    fn get_id(&self) -> [u8; 32] { self.path_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

impl<const N: usize> HierarchicalEvent for DrawingPath<N> {
    fn get_id(&self) -> [u8; 32] { self.path_id }
    fn get_parent_id(&self) -> Option<[u8; 32]> { Some(self.board_id) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sizes() {
        assert_eq!(std::mem::size_of::<RectangleData>(), 256);
        assert_eq!(std::mem::size_of::<CircleData>(), 256);
        assert_eq!(std::mem::size_of::<ArrowData>(), 256);
        assert_eq!(std::mem::size_of::<StickyNoteData>(), 512);
        assert_eq!(std::mem::size_of::<TextData>(), 512);
        assert_eq!(std::mem::size_of::<PathData<256>>(), 2432);
    }
}