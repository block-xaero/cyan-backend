use std::ffi::{c_char, CStr};
use std::ptr;
use std::slice;

use crate::storage::*;
use crate::objects::*;
use crate::{Group, Workspace, Board, Comment};
use xaeroid::XaeroID;
use xaeroflux_actors::XaeroFlux;

// ================================================================================================
// INITIALIZATION
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_init(
    xaero_id_data: *const u8,
    xaero_id_size: usize,
    data_dir: *const c_char,
) -> bool {
    unsafe {
        // Parse XaeroID
        eprintln!("cyan_init called with size: {}", xaero_id_size);
        eprintln!("Expected XaeroID size: {}", std::mem::size_of::<XaeroID>());
        let xid_bytes = slice::from_raw_parts(xaero_id_data, xaero_id_size);
        let xaero_id =  bytemuck::from_bytes::<xaeroid::XaeroID>(xid_bytes);

        // Get data directory
        let dir = if data_dir.is_null() {
            crate::storage::DATA_DIR.to_string()
        } else {
            CStr::from_ptr(data_dir).to_string_lossy().into_owned()
        };

        // Initialize XaeroFlux - pass by value, not reference
        XaeroFlux::initialize(*xaero_id, &dir).is_ok()
    }
}

// ================================================================================================
// GROUP OPERATIONS
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_group(
    creator_id: *const u8,
    name: *const c_char,
    icon: *const c_char,
    color_r: u8,
    color_g: u8,
    color_b: u8,
    color_a: u8,
    out_group_id: *mut u8,
) -> bool {
    unsafe {
        let mut creator = [0u8; 32];
        creator.copy_from_slice(slice::from_raw_parts(creator_id, 32));

        let name_str = CStr::from_ptr(name).to_string_lossy();
        let icon_str = CStr::from_ptr(icon).to_string_lossy();
        let color = [color_r, color_g, color_b, color_a];

        match create_group::<16>(creator, &name_str, &icon_str, color) {
            Ok(group_id) => {
                ptr::copy_nonoverlapping(group_id.as_ptr(), out_group_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_group(
    group_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        match get_group_by_group_id::<16>(gid) {
            Ok(group) => {
                let data = bytemuck::bytes_of(&group);
                if data.len() > buffer_size {
                    return false;
                }
                ptr::copy_nonoverlapping(data.as_ptr(), out_buffer, data.len());
                *actual_size = data.len();
                true
            }
            Err(_) => false,
        }
    }
}

// ================================================================================================
// WORKSPACE OPERATIONS
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_workspace(
    creator_id: *const u8,
    group_id: *const u8,
    name: *const c_char,
    out_workspace_id: *mut u8,
) -> bool {
    unsafe {
        let mut creator = [0u8; 32];
        creator.copy_from_slice(slice::from_raw_parts(creator_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        let name_str = CStr::from_ptr(name).to_string_lossy();

        match create_workspace::<32>(creator, gid, &name_str) {
            Ok(workspace_id) => {
                ptr::copy_nonoverlapping(workspace_id.as_ptr(), out_workspace_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_workspaces_for_group(
    group_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        match get_workspaces_for_group::<32, 16>(gid) {
            Ok((workspaces, count)) => {
                // Serialize workspaces array
                let data_size = count * std::mem::size_of::<Workspace<32>>();
                if data_size > buffer_size {
                    return false;
                }

                let mut offset = 0;
                for i in 0..count {
                    let ws_bytes = bytemuck::bytes_of(&workspaces[i]);
                    ptr::copy_nonoverlapping(
                        ws_bytes.as_ptr(),
                        out_buffer.add(offset),
                        ws_bytes.len(),
                    );
                    offset += ws_bytes.len();
                }

                *actual_size = data_size;
                true
            }
            Err(_) => false,
        }
    }
}

// ================================================================================================
// BOARD/WHITEBOARD OPERATIONS
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_board(
    creator_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    name: *const c_char,
    out_board_id: *mut u8,
) -> bool {
    unsafe {
        let mut creator = [0u8; 32];
        creator.copy_from_slice(slice::from_raw_parts(creator_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        let name_str = CStr::from_ptr(name).to_string_lossy();

        match create_board::<32>(creator, wid, gid, &name_str) {
            Ok(board_id) => {
                ptr::copy_nonoverlapping(board_id.as_ptr(), out_board_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_workspace_objects(
    workspace_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        match get_workspace_with_all_objects(wid) {
            Ok(workspace) => {
                // Serialize workspace data in a format Swift can parse
                // For now, return a simple count structure
                if buffer_size < 8 {
                    return false;
                }

                // Write whiteboard count
                let wb_count = workspace.whiteboards.len() as u32;
                ptr::copy_nonoverlapping(
                    (&wb_count as *const u32) as *const u8,
                    out_buffer,
                    4,
                );

                *actual_size = 4;
                true
            }
            Err(_) => false,
        }
    }
}

// ================================================================================================
// WHITEBOARD OBJECT OPERATIONS
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_path_object(
    board_id: *const u8,
    path_data: *const u8,
    path_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        // Parse PathData from bytes
        if path_size != std::mem::size_of::<PathData<512>>() {
            return false;
        }

        let path_bytes = slice::from_raw_parts(path_data, path_size);
        let path = bytemuck::from_bytes::<PathData<512>>(path_bytes);

        add_whiteboard_object(bid, OBJECT_TYPE_PATH, path).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_sticky_note(
    board_id: *const u8,
    sticky_data: *const u8,
    sticky_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        if sticky_size != std::mem::size_of::<StickyNoteData>() {
            return false;
        }

        let sticky_bytes = slice::from_raw_parts(sticky_data, sticky_size);
        let sticky = bytemuck::from_bytes::<StickyNoteData>(sticky_bytes);

        add_whiteboard_object(bid, OBJECT_TYPE_STICKY, sticky).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_rectangle(
    board_id: *const u8,
    rect_data: *const u8,
    rect_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        if rect_size != std::mem::size_of::<RectangleData>() {
            return false;
        }

        let rect_bytes = slice::from_raw_parts(rect_data, rect_size);
        let rect = bytemuck::from_bytes::<RectangleData>(rect_bytes);

        add_whiteboard_object(bid, OBJECT_TYPE_RECTANGLE, rect).is_ok()
    }
}

// ================================================================================================
// COMMENT OPERATIONS
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_comment(
    board_id: *const u8,
    author_id: *const u8,
    author_name: *const c_char,
    content: *const c_char,
    parent_id: *const u8,  // Can be null for top-level comments
    out_comment_id: *mut u8,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut aid = [0u8; 32];
        aid.copy_from_slice(slice::from_raw_parts(author_id, 32));

        let author_str = CStr::from_ptr(author_name).to_string_lossy();
        let content_str = CStr::from_ptr(content).to_string_lossy();

        let parent = if parent_id.is_null() {
            None
        } else {
            let mut pid = [0u8; 32];
            pid.copy_from_slice(slice::from_raw_parts(parent_id, 32));
            Some(pid)
        };

        match add_comment::<512>(bid, aid, &author_str, &content_str, parent) {
            Ok(comment_id) => {
                ptr::copy_nonoverlapping(comment_id.as_ptr(), out_comment_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

// ================================================================================================
// UTILITY FUNCTIONS
// ================================================================================================

#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_current_timestamp() -> u64 {
    xaeroflux_core::date_time::emit_secs()
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_hash_data(
    data: *const u8,
    data_size: usize,
    out_hash: *mut u8,
) -> bool {
    unsafe {
        let bytes = slice::from_raw_parts(data, data_size);
        let hash = blake3::hash(bytes);
        ptr::copy_nonoverlapping(hash.as_bytes().as_ptr(), out_hash, 32);
        true
    }
}