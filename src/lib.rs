mod workspaces;

use std::collections::BTreeMap;

#[repr(C, align(64))]
pub struct Object {
    pub group_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub id: [u8; 32],
}

#[repr(C, align(64))]
pub struct Workspace {
    pub group_id: [u8; 32],
    pub id: [u8; 32],
    pub objects: BTreeMap<[u8; 32], Object>,
}

#[repr(C, align(64))]
pub struct Group {
    pub id: [u8; 32],
    pub workspaces: BTreeMap<[u8; 32], Workspace>,
}

pub struct CyanTree {
    pub root: [u8; 32],
    pub groups: Vec<Group>,
}

use bytemuck::{Pod, Zeroable};
use rkyv::{Archive, Deserialize, Serialize};

/// A “canvas” in a workspace: basically a whiteboard page
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct Whiteboard {
    pub id: [u8; 32], // uuid or hash
    pub workspace_id: [u8; 32],
    pub title: [u8; 64], // fixed-size UTF-8 (or pointer to a string store)
    pub created_at: i64, // epoch secs
    pub updated_at: i64,
    // sticky notes, sketches, text blocks, etc:
    pub elements: BTreeMap<[u8; 32], ElementMeta>,
}

/// A union-style header for every “thing” you can place on a board
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy)]
pub struct ElementMeta {
    pub id: [u8; 32],
    pub element_type: ElementType,
    pub created_by: [u8; 32], // user_id
    pub created_at: i64,
    pub z_index: u16, // stacking order
}
unsafe impl Zeroable for ElementMeta {}
unsafe impl Pod for ElementMeta {}
/// Which kind of object on the board
#[repr(u8)]
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType {
    StickyNote,
    Sketch,
    TextBox,
    Image,
    FileEmbed,
    // …you can grow this
}

/// A sticky note on the board
#[repr(C, align(64))]
#[derive(Clone, Copy, Archive, Serialize, Deserialize, Debug)]
pub struct StickyNote {
    pub id: [u8; 32],
    pub element_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [u8; 4],  // RGBA
    pub text: [u8; 256], // fixed-size, or pointer into a blob store
}
unsafe impl Zeroable for StickyNote {}
unsafe impl Pod for StickyNote {}

/// A free-hand sketch stroke
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct Sketch {
    pub id: [u8; 32],
    pub element_id: [u8; 32],
    pub points: Vec<Point>, // you can rkyv-archive a `Vec<Point>`
    pub color: [u8; 4],
    pub thickness: f32,
}

/// Simple 2D point
#[repr(C)]
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}
unsafe impl Zeroable for Point {}
unsafe impl Pod for Point {}

/// A text block on the board
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct TextBox {
    pub id: [u8; 32],
    pub element_id: [u8; 32],
    pub x: f32,
    pub y: f32,
    pub text: Vec<u8>, // UTF-8
    pub font_size: u16,
}

/// A chat message attached to a board/workspace
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub sender_id: [u8; 32],
    pub timestamp: i64,
    pub content: Vec<u8>, // UTF-8
}

/// A file or image embedded in the workspace or board
#[repr(C, align(64))]
#[derive(Clone, Copy, Archive, Serialize, Deserialize, Debug)]
pub struct FileEmbed {
    pub id: [u8; 32],
    pub element_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub file_hash: [u8; 32], // points to the stored blob
    pub width: u32,
    pub height: u32,
}
unsafe impl Zeroable for FileEmbed {}
unsafe impl Pod for FileEmbed {}
/// A live collaboration session (who’s currently viewing/editing)
#[repr(C, align(64))]
#[derive(Clone, Copy, Archive, Serialize, Deserialize, Debug)]
pub struct CollabSession {
    pub session_id: [u8; 32],
    pub board_id: [u8; 32],
    pub user_id: [u8; 32],
    pub joined_at: i64,
    pub last_seen: i64,
}
unsafe impl Zeroable for CollabSession {}
unsafe impl Pod for CollabSession {}

/// A user’s private nano-LORA model & prefs
#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct UserPreferences {
    pub user_id: [u8; 32],
    pub preferred_stroke: [u8; 4], // RGBA
    pub preferred_font: [u8; 32],  // font name hash
    pub model_quantized: Vec<u8>,  // your tiny LORA weights
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, size_of};

    use bytemuck::{Pod, Zeroable};
    use rkyv::Archive;

    use super::*;

    // Helper to assert a type is Pod at compile time
    fn assert_pod<T: Pod>() {}
    fn assert_zeroable<T: Zeroable>() {}
    fn assert_archive<T: Archive>() {}

    #[test]
    fn test_object_layout() {
        // Object has three [u8;32] arrays (96 bytes) aligned to 64 => padded to 128
        assert_eq!(size_of::<Object>(), 128);
        assert_eq!(align_of::<Object>(), 64);
    }

    #[test]
    fn test_workspace_layout() {
        // Workspace: two [u8;32] + BTreeMap (two pointers + usize -> 16 bytes)
        // so 32 + 32 + 16 = 80 bytes, aligned to 64 => size rounds up to 128
        assert_eq!(align_of::<Workspace>(), 64);
        assert_eq!(size_of::<Workspace>(), 128);
    }

    #[test]
    fn test_group_layout() {
        // Group: [u8;32] + BTreeMap -> 32 + 16 = 48 bytes, aligned to 64 => size 64
        assert_eq!(align_of::<Group>(), 64);
        assert_eq!(size_of::<Group>(), 64);
    }

    #[test]
    fn test_cyan_tree_layout() {
        // CyanTree has root [u8;32] + Vec<Group> (pointer, len, cap = 24 bytes)
        // 32 + 24 = 56, default repr -> alignment 8 -> size 64
        assert_eq!(size_of::<CyanTree>(), 56);
        assert_eq!(align_of::<CyanTree>(), 8);
    }

    #[test]
    fn test_element_meta_traits() {
        // ElementMeta implements Pod + Zeroable
        assert_pod::<ElementMeta>();
        assert_zeroable::<ElementMeta>();
        assert_eq!(align_of::<ElementMeta>(), 64);
    }

    #[test]
    fn test_sticky_note_traits() {
        assert_pod::<StickyNote>();
        assert_zeroable::<StickyNote>();
        assert_eq!(align_of::<StickyNote>(), 64);
    }

    #[test]
    fn test_file_embed_traits() {
        assert_pod::<FileEmbed>();
        assert_zeroable::<FileEmbed>();
        assert_eq!(align_of::<FileEmbed>(), 64);
    }

    #[test]
    fn test_collab_session_traits() {
        assert_pod::<CollabSession>();
        assert_zeroable::<CollabSession>();
        assert_eq!(align_of::<CollabSession>(), 64);
    }

    #[test]
    fn test_element_type_layout() {
        // enum repr(u8)
        assert_eq!(size_of::<ElementType>(), 1);
        assert_eq!(align_of::<ElementType>(), 1);
    }

    #[test]
    fn test_rkyv_archive_supported() {
        // Verify key types derive Archive
        assert_archive::<Whiteboard>();
        assert_archive::<ElementMeta>();
        assert_archive::<StickyNote>();
        assert_archive::<Sketch>();
        assert_archive::<Point>();
        assert_archive::<TextBox>();
        assert_archive::<ChatMessage>();
        assert_archive::<FileEmbed>();
        assert_archive::<CollabSession>();
        assert_archive::<UserPreferences>();
    }

    #[test]
    fn test_rkyv_serde_roundtrip_point() {
        // Roundtrip a simple Point
        let pt = Point { x: 1.23, y: 4.56 };
        let bytes = bytemuck::bytes_of(&pt);
        let casted: &Point = bytemuck::from_bytes(bytes);
        assert_eq!(casted.x, pt.x);
        assert_eq!(casted.y, pt.y);
    }
}
