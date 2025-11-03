// ffi.rs - Complete FFI interface with all drawing operations
use std::ffi::c_char;
use std::{ffi::CStr, ptr, slice};
use std::error::Error;
use tracing_subscriber::EnvFilter;
use xaeroflux_actors::XaeroFlux;
use bytemuck::Zeroable;
use xaeroflux::hash::blake_hash_slice;
use crate::{objects::*, storage::*, BoardStandard, CommentStandard, DrawingPathStandard, GroupStandard, WorkspaceStandard};


// INITIALIZATION
#[unsafe(no_mangle)]
pub extern "C" fn cyan_init(
    xaero_id_data: *const u8,
    xaero_id_size: usize,
    data_dir: *const c_char,
) -> bool {
    unsafe {
        let xid_bytes = slice::from_raw_parts(xaero_id_data, xaero_id_size);
        let xaero_id = bytemuck::from_bytes::<xaeroid::XaeroID>(xid_bytes);

        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::new(
                    "error,cyan_backend=trace,xaeroflux_actors=info",
                ))
                .init();
        });

        let dir = if data_dir.is_null() {
            DATA_DIR.to_string()
        } else {
            CStr::from_ptr(data_dir).to_string_lossy().into_owned()
        };

        let res = XaeroFlux::initialize(*xaero_id, &dir).is_ok();
        // match create_test_data_structure(blake_hash_slice(&xid_bytes)){
        //     Ok(_) => {
        //         tracing::info!("Created test data structure");
        //     }
        //     Err(e) => {
        //         panic!("Failed to create test data: {}", e);
        //     }
        // }
        res
    }
}

// GROUP OPERATIONS
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

        match create_group(creator, &name_str, &icon_str, color) {
            Ok(group_id) => {
                ptr::copy_nonoverlapping(group_id.as_ptr(), out_group_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_group(
    group_id: *const u8,
    name: *const c_char,
    icon: *const c_char,
    color_r: u8,
    color_g: u8,
    color_b: u8,
    color_a: u8,
) -> bool {
    unsafe {
        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        let name_opt = if name.is_null() {
            None
        } else {
            Some(CStr::from_ptr(name).to_string_lossy())
        };

        let icon_opt = if icon.is_null() {
            None
        } else {
            Some(CStr::from_ptr(icon).to_string_lossy())
        };

        let color_opt = if color_r == 0 && color_g == 0 && color_b == 0 && color_a == 0 {
            None
        } else {
            Some([color_r, color_g, color_b, color_a])
        };

        update_group(gid, name_opt.as_deref(), icon_opt.as_deref(), color_opt).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_group(group_id: *const u8) -> bool {
    unsafe {
        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));
        tombstone_group(gid).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_group(group_id: *const u8, new_name: *const c_char) -> bool {
    unsafe {
        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));
        let name_str = CStr::from_ptr(new_name).to_string_lossy();
        update_group(gid, Some(&name_str), None, None).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_list_groups(
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        if let Err(e) = debug_scan_all_events() {
            eprintln!("Debug scan failed: {}", e);
        }
        match list_all_groups() {
            Ok(groups) => {
                let total_size = groups.len() * std::mem::size_of::<GroupStandard>();
                if total_size > buffer_size {
                    *actual_size = total_size;
                    return false;
                }

                let mut offset = 0;
                for group in groups.iter() {
                    let group_bytes = bytemuck::bytes_of(group);
                    ptr::copy_nonoverlapping(
                        group_bytes.as_ptr(),
                        out_buffer.add(offset),
                        group_bytes.len(),
                    );
                    offset += group_bytes.len();
                }

                *actual_size = total_size;
                true
            }
            Err(_) => false,
        }
    }
}

// WORKSPACE OPERATIONS
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

        match create_workspace(creator, gid, &name_str) {
            Ok(workspace_id) => {
                ptr::copy_nonoverlapping(workspace_id.as_ptr(), out_workspace_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_workspace(
    workspace_id: *const u8,
    name: *const c_char,
) -> bool {
    unsafe {
        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let name_opt = if name.is_null() {
            None
        } else {
            Some(CStr::from_ptr(name).to_string_lossy())
        };

        update_workspace(wid, name_opt.as_deref()).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_workspace(workspace_id: *const u8) -> bool {
    unsafe {
        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));
        tombstone_workspace(wid).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_workspace(workspace_id: *const u8, new_name: *const c_char) -> bool {
    unsafe {
        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));
        let name_str = CStr::from_ptr(new_name).to_string_lossy();
        update_workspace(wid, Some(&name_str)).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_list_workspaces(
    group_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut g_id = [0u8; 32];
        g_id.copy_from_slice(slice::from_raw_parts(group_id, 32));
        match list_workspaces_for_group(g_id) {
            Ok(workspaces) => {
                let total_size = workspaces.len() * std::mem::size_of::<WorkspaceStandard>();
                if total_size > buffer_size {
                    *actual_size = total_size;
                    return false;
                }

                let mut offset = 0;
                for workspace in workspaces.iter() {
                    let ws_bytes = bytemuck::bytes_of(workspace);
                    ptr::copy_nonoverlapping(
                        ws_bytes.as_ptr(),
                        out_buffer.add(offset),
                        ws_bytes.len(),
                    );
                    offset += ws_bytes.len();
                }

                *actual_size = total_size;
                true
            }
            Err(_) => false,
        }
    }
}

// BOARD OPERATIONS
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

        match create_board(creator, wid, gid, &name_str) {
            Ok(board_id) => {
                ptr::copy_nonoverlapping(board_id.as_ptr(), out_board_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_board(
    board_id: *const u8,
    name: *const c_char,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let name_opt = if name.is_null() {
            None
        } else {
            Some(CStr::from_ptr(name).to_string_lossy())
        };

        update_board(bid, name_opt.as_deref()).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_board(board_id: *const u8) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));
        tombstone_board(bid).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_board(board_id: *const u8, new_name: *const c_char) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));
        let name_str = CStr::from_ptr(new_name).to_string_lossy();
        update_board(bid, Some(&name_str)).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_workspace_boards(
    group_id: *const u8,
    workspace_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        match list_boards_for_workspace(wid) {
            Ok(boards) => {
                let total_size = boards.len() * std::mem::size_of::<BoardStandard>();
                if total_size > buffer_size {
                    *actual_size = total_size;
                    return false;
                }

                let mut offset = 0;
                for board in boards.iter() {
                    let board_bytes = bytemuck::bytes_of(board);
                    ptr::copy_nonoverlapping(
                        board_bytes.as_ptr(),
                        out_buffer.add(offset),
                        board_bytes.len(),
                    );
                    offset += board_bytes.len();
                }

                *actual_size = total_size;
                true
            }
            Err(_) => false,
        }
    }
}

// PATH DRAWING OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_path_object(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    path_data: *const u8,
    path_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        if path_size != std::mem::size_of::<PathData<256>>() {
            tracing::error!("Path size mismatch: expected {}, got {}",
                std::mem::size_of::<PathData<256>>(), path_size);
            return false;
        }

        // Copy to aligned buffer
        let path_bytes = slice::from_raw_parts(path_data, path_size);
        let mut aligned_path = PathData::<256>::zeroed();
        std::ptr::copy_nonoverlapping(
            path_bytes.as_ptr(),
            &mut aligned_path as *mut PathData<256> as *mut u8,
            std::mem::size_of::<PathData<256>>(),
        );

        add_path_object(bid, wid, gid, &aligned_path).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_path_object(
    path_data: *const u8,
    path_size: usize,
) -> bool {
    unsafe {
        if path_size != std::mem::size_of::<PathData<256>>() {
            return false;
        }

        // Copy to aligned buffer
        let path_bytes = slice::from_raw_parts(path_data, path_size);
        let mut aligned_path = PathData::<256>::zeroed();
        std::ptr::copy_nonoverlapping(
            path_bytes.as_ptr(),
            &mut aligned_path as *mut PathData<256> as *mut u8,
            std::mem::size_of::<PathData<256>>(),
        );

        update_path_object(&aligned_path).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_path_object(
    path_id: *const u8,
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
) -> bool {
    unsafe {
        let mut pid = [0u8; 32];
        pid.copy_from_slice(slice::from_raw_parts(path_id, 32));

        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        tombstone_path_object(pid, bid, wid, gid).is_ok()
    }
}

// STICKY NOTE OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_sticky_note(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    sticky_data: *const u8,
    sticky_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        if sticky_size != std::mem::size_of::<StickyNoteData>() {
            return false;
        }

        // Copy to aligned buffer
        let sticky_bytes = slice::from_raw_parts(sticky_data, sticky_size);
        let mut aligned_sticky = StickyNoteData::zeroed();
        std::ptr::copy_nonoverlapping(
            sticky_bytes.as_ptr(),
            &mut aligned_sticky as *mut StickyNoteData as *mut u8,
            std::mem::size_of::<StickyNoteData>(),
        );

        add_sticky_note(bid, wid, gid, &aligned_sticky).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_sticky_note(
    sticky_data: *const u8,
    sticky_size: usize,
) -> bool {
    unsafe {
        if sticky_size != std::mem::size_of::<StickyNoteData>() {
            return false;
        }

        // Copy to aligned buffer
        let sticky_bytes = slice::from_raw_parts(sticky_data, sticky_size);
        let mut aligned_sticky = StickyNoteData::zeroed();
        std::ptr::copy_nonoverlapping(
            sticky_bytes.as_ptr(),
            &mut aligned_sticky as *mut StickyNoteData as *mut u8,
            std::mem::size_of::<StickyNoteData>(),
        );

        update_sticky_note(&aligned_sticky).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_sticky_note(
    note_id: *const u8,
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
) -> bool {
    unsafe {
        let mut nid = [0u8; 32];
        nid.copy_from_slice(slice::from_raw_parts(note_id, 32));

        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        tombstone_sticky_note(nid, bid, wid, gid).is_ok()
    }
}

// RECTANGLE OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_rectangle(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    rect_data: *const u8,
    rect_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));
        tracing::info!("==rectangle size provided : {} vs rectangle size struct : {} ", rect_size,
            std::mem::size_of::<RectangleData>());
        if rect_size != std::mem::size_of::<RectangleData>() {
            return false;
        }

        // Copy to aligned buffer
        let rect_bytes = slice::from_raw_parts(rect_data, rect_size);
        let mut aligned_rect = RectangleData::zeroed();
        std::ptr::copy_nonoverlapping(
            rect_bytes.as_ptr(),
            &mut aligned_rect as *mut RectangleData as *mut u8,
            std::mem::size_of::<RectangleData>(),
        );

        add_rectangle(bid, wid, gid, &aligned_rect).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_rectangle(
    rect_data: *const u8,
    rect_size: usize,
) -> bool {
    unsafe {
        if rect_size != std::mem::size_of::<RectangleData>() {
            return false;
        }

        // Copy to aligned buffer
        let rect_bytes = slice::from_raw_parts(rect_data, rect_size);
        let mut aligned_rect = RectangleData::zeroed();
        std::ptr::copy_nonoverlapping(
            rect_bytes.as_ptr(),
            &mut aligned_rect as *mut RectangleData as *mut u8,
            std::mem::size_of::<RectangleData>(),
        );

        update_rectangle(&aligned_rect).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_rectangle(
    shape_id: *const u8,
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
) -> bool {
    unsafe {
        let mut sid = [0u8; 32];
        sid.copy_from_slice(slice::from_raw_parts(shape_id, 32));

        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        tombstone_rectangle(sid, bid, wid, gid).is_ok()
    }
}

// CIRCLE OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_circle(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    circle_data: *const u8,
    circle_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        if circle_size != std::mem::size_of::<CircleData>() {
            tracing::error!("Circle size mismatch: expected {}, got {}",
                std::mem::size_of::<CircleData>(), circle_size);
            return false;
        }

        // Copy to aligned buffer instead of using from_bytes directly
        let circle_bytes = slice::from_raw_parts(circle_data, circle_size);
        let mut aligned_circle = CircleData::zeroed();
        std::ptr::copy_nonoverlapping(
            circle_bytes.as_ptr(),
            &mut aligned_circle as *mut CircleData as *mut u8,
            std::mem::size_of::<CircleData>(),
        );

        match add_circle(bid, wid, gid, &aligned_circle) {
            Ok(id) => {
                tracing::info!("Successfully added circle with id: {}", hex::encode(&id));
                true
            }
            Err(e) => {
                tracing::error!("Failed to add circle: {:?}", e);
                false
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_circle(
    circle_data: *const u8,
    circle_size: usize,
) -> bool {
    unsafe {
        if circle_size != std::mem::size_of::<CircleData>() {
            return false;
        }

        // Copy to aligned buffer
        let circle_bytes = slice::from_raw_parts(circle_data, circle_size);
        let mut aligned_circle = CircleData::zeroed();
        std::ptr::copy_nonoverlapping(
            circle_bytes.as_ptr(),
            &mut aligned_circle as *mut CircleData as *mut u8,
            std::mem::size_of::<CircleData>(),
        );

        update_circle(&aligned_circle).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_circle(
    shape_id: *const u8,
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
) -> bool {
    unsafe {
        let mut sid = [0u8; 32];
        sid.copy_from_slice(slice::from_raw_parts(shape_id, 32));

        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        tombstone_circle(sid, bid, wid, gid).is_ok()
    }
}

// ARROW OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_arrow(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    arrow_data: *const u8,
    arrow_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        if arrow_size != std::mem::size_of::<ArrowData>() {
            return false;
        }

        // Copy to aligned buffer
        let arrow_bytes = slice::from_raw_parts(arrow_data, arrow_size);
        let mut aligned_arrow = ArrowData::zeroed();
        std::ptr::copy_nonoverlapping(
            arrow_bytes.as_ptr(),
            &mut aligned_arrow as *mut ArrowData as *mut u8,
            std::mem::size_of::<ArrowData>(),
        );

        add_arrow(bid, wid, gid, &aligned_arrow).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_arrow(
    arrow_data: *const u8,
    arrow_size: usize,
) -> bool {
    unsafe {
        if arrow_size != std::mem::size_of::<ArrowData>() {
            return false;
        }

        // Copy to aligned buffer
        let arrow_bytes = slice::from_raw_parts(arrow_data, arrow_size);
        let mut aligned_arrow = ArrowData::zeroed();
        std::ptr::copy_nonoverlapping(
            arrow_bytes.as_ptr(),
            &mut aligned_arrow as *mut ArrowData as *mut u8,
            std::mem::size_of::<ArrowData>(),
        );

        update_arrow(&aligned_arrow).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_arrow(
    arrow_id: *const u8,
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
) -> bool {
    unsafe {
        let mut aid = [0u8; 32];
        aid.copy_from_slice(slice::from_raw_parts(arrow_id, 32));

        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        tombstone_arrow(aid, bid, wid, gid).is_ok()
    }
}

// TEXT OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_text(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    text_data: *const u8,
    text_size: usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        if text_size != std::mem::size_of::<TextData>() {
            return false;
        }

        // Copy to aligned buffer
        let text_bytes = slice::from_raw_parts(text_data, text_size);
        let mut aligned_text = TextData::zeroed();
        std::ptr::copy_nonoverlapping(
            text_bytes.as_ptr(),
            &mut aligned_text as *mut TextData as *mut u8,
            std::mem::size_of::<TextData>(),
        );

        add_text(bid, wid, gid, &aligned_text).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_update_text(
    text_data: *const u8,
    text_size: usize,
) -> bool {
    unsafe {
        if text_size != std::mem::size_of::<TextData>() {
            return false;
        }

        // Copy to aligned buffer
        let text_bytes = slice::from_raw_parts(text_data, text_size);
        let mut aligned_text = TextData::zeroed();
        std::ptr::copy_nonoverlapping(
            text_bytes.as_ptr(),
            &mut aligned_text as *mut TextData as *mut u8,
            std::mem::size_of::<TextData>(),
        );

        update_text(&aligned_text).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_text(
    text_id: *const u8,
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
) -> bool {
    unsafe {
        let mut tid = [0u8; 32];
        tid.copy_from_slice(slice::from_raw_parts(text_id, 32));

        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        tombstone_text(tid, bid, wid, gid).is_ok()
    }
}

// LOAD BOARD OBJECTS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_load_board_objects(
    board_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        match load_all_board_objects(bid) {
            Ok(objects) => {
                // Serialize board objects into buffer
                // Format: [count:4][type:1][size:4][data]... for each object type
                let mut offset = 0;

                // Total object count
                let total_count = (objects.paths.len() +
                    objects.sticky_notes.len() +
                    objects.rectangles.len() +
                    objects.circles.len() +
                    objects.arrows.len() +
                    objects.text_objects.len()) as u32;

                if offset + 4 <= buffer_size {
                    ptr::copy_nonoverlapping(
                        &total_count as *const u32 as *const u8,
                        out_buffer.add(offset),
                        4,
                    );
                    offset += 4;
                }

                // Write paths (type=1)
                for path in &objects.paths {
                    if offset + 5 + std::mem::size_of::<PathData<256>>() <= buffer_size {
                        // Type
                        out_buffer.add(offset).write(OBJECT_TYPE_PATH);
                        offset += 1;

                        // Size
                        let size = std::mem::size_of::<PathData<256>>() as u32;
                        ptr::copy_nonoverlapping(
                            &size as *const u32 as *const u8,
                            out_buffer.add(offset),
                            4,
                        );
                        offset += 4;

                        // Data
                        let path_bytes = bytemuck::bytes_of(path);
                        ptr::copy_nonoverlapping(
                            path_bytes.as_ptr(),
                            out_buffer.add(offset),
                            path_bytes.len(),
                        );
                        offset += path_bytes.len();
                    }
                }

                // Write sticky notes (type=2)
                for sticky in &objects.sticky_notes {
                    if offset + 5 + std::mem::size_of::<StickyNoteData>() <= buffer_size {
                        out_buffer.add(offset).write(OBJECT_TYPE_STICKY);
                        offset += 1;

                        let size = std::mem::size_of::<StickyNoteData>() as u32;
                        ptr::copy_nonoverlapping(
                            &size as *const u32 as *const u8,
                            out_buffer.add(offset),
                            4,
                        );
                        offset += 4;

                        let sticky_bytes = bytemuck::bytes_of(sticky);
                        ptr::copy_nonoverlapping(
                            sticky_bytes.as_ptr(),
                            out_buffer.add(offset),
                            sticky_bytes.len(),
                        );
                        offset += sticky_bytes.len();
                    }
                }

                // Write rectangles (type=3)
                for rect in &objects.rectangles {
                    if offset + 5 + std::mem::size_of::<RectangleData>() <= buffer_size {
                        out_buffer.add(offset).write(OBJECT_TYPE_RECTANGLE);
                        offset += 1;

                        let size = std::mem::size_of::<RectangleData>() as u32;
                        ptr::copy_nonoverlapping(
                            &size as *const u32 as *const u8,
                            out_buffer.add(offset),
                            4,
                        );
                        offset += 4;

                        let rect_bytes = bytemuck::bytes_of(rect);
                        ptr::copy_nonoverlapping(
                            rect_bytes.as_ptr(),
                            out_buffer.add(offset),
                            rect_bytes.len(),
                        );
                        offset += rect_bytes.len();
                    }
                }

                // Write circles (type=4)
                for circle in &objects.circles {
                    if offset + 5 + std::mem::size_of::<CircleData>() <= buffer_size {
                        out_buffer.add(offset).write(OBJECT_TYPE_CIRCLE);
                        offset += 1;

                        let size = std::mem::size_of::<CircleData>() as u32;
                        ptr::copy_nonoverlapping(
                            &size as *const u32 as *const u8,
                            out_buffer.add(offset),
                            4,
                        );
                        offset += 4;

                        let circle_bytes = bytemuck::bytes_of(circle);
                        ptr::copy_nonoverlapping(
                            circle_bytes.as_ptr(),
                            out_buffer.add(offset),
                            circle_bytes.len(),
                        );
                        offset += circle_bytes.len();
                    }
                }

                // Write arrows (type=5)
                for arrow in &objects.arrows {
                    if offset + 5 + std::mem::size_of::<ArrowData>() <= buffer_size {
                        out_buffer.add(offset).write(OBJECT_TYPE_ARROW);
                        offset += 1;

                        let size = std::mem::size_of::<ArrowData>() as u32;
                        ptr::copy_nonoverlapping(
                            &size as *const u32 as *const u8,
                            out_buffer.add(offset),
                            4,
                        );
                        offset += 4;

                        let arrow_bytes = bytemuck::bytes_of(arrow);
                        ptr::copy_nonoverlapping(
                            arrow_bytes.as_ptr(),
                            out_buffer.add(offset),
                            arrow_bytes.len(),
                        );
                        offset += arrow_bytes.len();
                    }
                }

                // Write text objects (type=6)
                for text in &objects.text_objects {
                    if offset + 5 + std::mem::size_of::<TextData>() <= buffer_size {
                        out_buffer.add(offset).write(OBJECT_TYPE_TEXT);
                        offset += 1;

                        let size = std::mem::size_of::<TextData>() as u32;
                        ptr::copy_nonoverlapping(
                            &size as *const u32 as *const u8,
                            out_buffer.add(offset),
                            4,
                        );
                        offset += 4;

                        let text_bytes = bytemuck::bytes_of(text);
                        ptr::copy_nonoverlapping(
                            text_bytes.as_ptr(),
                            out_buffer.add(offset),
                            text_bytes.len(),
                        );
                        offset += text_bytes.len();
                    }
                }

                *actual_size = offset;
                true
            }
            Err(_) => false,
        }
    }
}

// COMMENT OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_comment(
    board_id: *const u8,
    workspace_id: *const u8,
    group_id: *const u8,
    author_id: *const u8,
    author_name: *const c_char,
    content: *const c_char,
    parent_id: *const u8,
    out_comment_id: *mut u8,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

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

        match add_comment(bid, wid, gid, aid, &author_str, &content_str, parent) {
            Ok(comment_id) => {
                ptr::copy_nonoverlapping(comment_id.as_ptr(), out_comment_id, 32);
                true
            }
            Err(_) => false,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_list_comments_for_board(
    board_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut bid = [0u8; 32];
        bid.copy_from_slice(slice::from_raw_parts(board_id, 32));

        match list_comments_for_board(bid) {
            Ok(comments) => {
                let total_size = comments.len() * std::mem::size_of::<CommentStandard>();
                if total_size > buffer_size {
                    *actual_size = total_size;
                    return false;
                }

                let mut offset = 0;
                for comment in comments.iter() {
                    let comment_bytes = bytemuck::bytes_of(comment);
                    ptr::copy_nonoverlapping(
                        comment_bytes.as_ptr(),
                        out_buffer.add(offset),
                        comment_bytes.len(),
                    );
                    offset += comment_bytes.len();
                }

                *actual_size = total_size;
                true
            }
            Err(e) => {
                tracing::error!("Failed to list comments: {:?}", e);
                false
            }
        }
    }
}

// FILE OPERATIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_file_to_group(
    group_id: *const u8,
    file_path: *const c_char,
    file_data: *const u8,
    file_size: usize,
    out_file_id: *mut u8,
) -> bool {
    // TODO: Implement file storage for groups
    // For now, generate a file ID
    unsafe {
        let file_id = blake3::hash(&xaeroflux_core::date_time::emit_nanos().to_le_bytes());
        ptr::copy_nonoverlapping(file_id.as_bytes().as_ptr(), out_file_id, 32);
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_file_to_workspace(
    workspace_id: *const u8,
    file_path: *const c_char,
    file_data: *const u8,
    file_size: usize,
    out_file_id: *mut u8,
) -> bool {
    // TODO: Implement file storage for workspaces
    unsafe {
        let file_id = blake3::hash(&xaeroflux_core::date_time::emit_nanos().to_le_bytes());
        ptr::copy_nonoverlapping(file_id.as_bytes().as_ptr(), out_file_id, 32);
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_list_files_in_group(
    group_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        *actual_size = 0;
    }
    true
}

// UTILITY FUNCTIONS
#[unsafe(no_mangle)]
pub extern "C" fn cyan_get_current_timestamp() -> u64 {
    xaeroflux_core::date_time::emit_secs()
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_hash_data(data: *const u8, data_size: usize, out_hash: *mut u8) -> bool {
    unsafe {
        let bytes = slice::from_raw_parts(data, data_size);
        let hash = blake3::hash(bytes);
        ptr::copy_nonoverlapping(hash.as_bytes().as_ptr(), out_hash, 32);
        true
    }
}

// INVITATION OPERATIONS (keeping your existing implementation)
#[unsafe(no_mangle)]
pub extern "C" fn cyan_create_invitation(
    inviter_xaero_id: *const u8,
    workspace_id: *const u8,
    invitee_xaero_id: *const u8,
    expiry_time: u64,
    out_invitation: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        use ark_bn254::Fr;
        use ark_ff::{BigInteger, PrimeField};

        let inviter_bytes = slice::from_raw_parts(inviter_xaero_id, 2572);
        let inviter = bytemuck::from_bytes::<xaeroid::XaeroID>(inviter_bytes);

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let invitee_bytes = slice::from_raw_parts(invitee_xaero_id, 2572);

        let workspace_fr = Fr::from_le_bytes_mod_order(&wid);
        let invitee_hash = blake3::hash(invitee_bytes);
        let invitee_fr = Fr::from_le_bytes_mod_order(invitee_hash.as_bytes());
        let expiry_fr = Fr::from(expiry_time);

        let invitation_code = Fr::from_le_bytes_mod_order(&inviter.secret_key[..32]);
        let invitation_nonce = Fr::from(rand::random::<u64>());

        let invitation_hash =
            invitation_code + invitation_nonce * invitee_fr + workspace_fr * expiry_fr;

        let mut output = Vec::new();
        output.extend_from_slice(&invitation_code.into_bigint().to_bytes_le()[..32]);
        output.extend_from_slice(&invitation_nonce.into_bigint().to_bytes_le()[..32]);
        output.extend_from_slice(&invitation_hash.into_bigint().to_bytes_le()[..32]);

        if output.len() > buffer_size {
            return false;
        }

        ptr::copy_nonoverlapping(output.as_ptr(), out_invitation, output.len());
        *actual_size = output.len();

        let invitation_data = crate::storage::InvitationRecord {
            invitation_hash: invitation_hash.into_bigint().to_bytes_le()[..32]
                .try_into()
                .unwrap(),
            workspace_id: wid,
            inviter_id: inviter.did_peer[..32].try_into().unwrap(),
            invitee_id: invitee_hash.into(),
            expiry_time,
            created_at: xaeroflux_core::date_time::emit_secs(),
            _padding: [0u8; 48],
        };

        crate::storage::store_invitation(invitation_data).is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_accept_invitation(
    invitation_code: *const u8,
    invitation_nonce: *const u8,
    invitation_hash: *const u8,
    workspace_id: *const u8,
    claimer_xaero_id: *const u8,
    expiry_time: u64,
) -> bool {
    unsafe {
        use ark_bn254::Fr;
        use ark_ff::PrimeField;

        let code_bytes = slice::from_raw_parts(invitation_code, 32);
        let nonce_bytes = slice::from_raw_parts(invitation_nonce, 32);
        let hash_bytes = slice::from_raw_parts(invitation_hash, 32);

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let claimer_bytes = slice::from_raw_parts(claimer_xaero_id, 2572);
        let claimer = bytemuck::from_bytes::<xaeroid::XaeroID>(claimer_bytes);

        let mut hash_array = [0u8; 32];
        hash_array.copy_from_slice(hash_bytes);

        match crate::storage::get_invitation(&hash_array) {
            Ok(invitation) => {
                if invitation.expiry_time < xaeroflux_core::date_time::emit_secs() {
                    return false;
                }

                let code_fr = Fr::from_le_bytes_mod_order(code_bytes);
                let nonce_fr = Fr::from_le_bytes_mod_order(nonce_bytes);
                let hash_fr = Fr::from_le_bytes_mod_order(hash_bytes);
                let workspace_fr = Fr::from_le_bytes_mod_order(&wid);
                let claimer_hash = blake3::hash(&claimer.did_peer[..claimer.did_peer_len as usize]);
                let claimer_fr = Fr::from_le_bytes_mod_order(claimer_hash.as_bytes());
                let expiry_fr = Fr::from(expiry_time);

                let computed_hash = code_fr + nonce_fr * claimer_fr + workspace_fr * expiry_fr;

                if computed_hash == hash_fr {
                    crate::storage::add_user_to_workspace(
                        wid,
                        claimer.did_peer[..32].try_into().unwrap(),
                    )
                        .is_ok()
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }
}