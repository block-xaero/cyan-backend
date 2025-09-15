// ffi.rs - Complete FFI interface
use std::{ffi::CStr, ptr, slice};

use tracing_subscriber::EnvFilter;
use xaeroflux_actors::XaeroFlux;
use xaeroid::XaeroID;

use crate::{
    BoardStandard, CommentStandard, DrawingPathStandard, FileNode, GroupStandard, Layer,
    PATH_MAX_POINTS, Vote, WorkspaceStandard, objects::*, storage::*,
};

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

        XaeroFlux::initialize(*xaero_id, &dir).is_ok()
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
pub extern "C" fn cyan_list_groups(
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
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
pub extern "C" fn cyan_list_workspaces(
    group_id: *const u8,
    out_buffer: *mut u8,
    buffer_size: usize,
    actual_size: *mut usize,
) -> bool {
    unsafe {
        let mut gid = [0u8; 32];
        gid.copy_from_slice(slice::from_raw_parts(group_id, 32));

        match list_workspaces_for_group(gid) {
            Ok(workspaces) => {
                let total_size = workspaces.len() * std::mem::size_of::<WorkspaceStandard>();
                if total_size > buffer_size {
                    *actual_size = total_size;
                    return false;
                }

                let mut offset = 0;
                for workspace in workspaces {
                    let ws_bytes = bytemuck::bytes_of(&workspace);
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

// PATH OBJECT OPERATIONS
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

        if path_size != std::mem::size_of::<DrawingPathStandard>() {
            return false;
        }

        let path_bytes = slice::from_raw_parts(path_data, path_size);
        let path = bytemuck::from_bytes::<DrawingPathStandard>(path_bytes);

        add_whiteboard_object(bid, wid, gid, OBJECT_TYPE_PATH, path).is_ok()
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

        let sticky_bytes = slice::from_raw_parts(sticky_data, sticky_size);
        let sticky = bytemuck::from_bytes::<StickyNoteData>(sticky_bytes);

        add_whiteboard_object(bid, wid, gid, OBJECT_TYPE_STICKY, sticky).is_ok()
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

        if rect_size != std::mem::size_of::<RectangleData>() {
            return false;
        }

        let rect_bytes = slice::from_raw_parts(rect_data, rect_size);
        let rect = bytemuck::from_bytes::<RectangleData>(rect_bytes);

        add_whiteboard_object(bid, wid, gid, OBJECT_TYPE_RECTANGLE, rect).is_ok()
    }
}

// INVITATION OPERATIONS
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

        // Create invitation using XaeroID's ZK capabilities
        let workspace_fr = Fr::from_le_bytes_mod_order(&wid);
        let invitee_hash = blake3::hash(invitee_bytes);
        let invitee_fr = Fr::from_le_bytes_mod_order(invitee_hash.as_bytes());
        let expiry_fr = Fr::from(expiry_time);

        // Generate invitation components
        let invitation_code = Fr::from_le_bytes_mod_order(&inviter.secret_key[..32]);
        let invitation_nonce = Fr::from(rand::random::<u64>());

        // Compute invitation hash
        let invitation_hash =
            invitation_code + invitation_nonce * invitee_fr + workspace_fr * expiry_fr;

        // Prepare output
        let mut output = Vec::new();
        output.extend_from_slice(&invitation_code.into_bigint().to_bytes_le()[..32]);
        output.extend_from_slice(&invitation_nonce.into_bigint().to_bytes_le()[..32]);
        output.extend_from_slice(&invitation_hash.into_bigint().to_bytes_le()[..32]);

        if output.len() > buffer_size {
            return false;
        }

        ptr::copy_nonoverlapping(output.as_ptr(), out_invitation, output.len());
        *actual_size = output.len();

        // Store invitation in LMDB for validation
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

        // Parse inputs
        let code_bytes = slice::from_raw_parts(invitation_code, 32);
        let nonce_bytes = slice::from_raw_parts(invitation_nonce, 32);
        let hash_bytes = slice::from_raw_parts(invitation_hash, 32);

        let mut wid = [0u8; 32];
        wid.copy_from_slice(slice::from_raw_parts(workspace_id, 32));

        let claimer_bytes = slice::from_raw_parts(claimer_xaero_id, 2572);
        let claimer = bytemuck::from_bytes::<xaeroid::XaeroID>(claimer_bytes);

        // Create the hash array properly
        let mut hash_array = [0u8; 32];
        hash_array.copy_from_slice(hash_bytes);

        // Verify invitation exists and is valid
        match crate::storage::get_invitation(&hash_array) {
            Ok(invitation) => {
                if invitation.expiry_time < xaeroflux_core::date_time::emit_secs() {
                    return false; // Expired
                }

                // Create ZK proof of invitation claim
                let code_fr = Fr::from_le_bytes_mod_order(code_bytes);
                let nonce_fr = Fr::from_le_bytes_mod_order(nonce_bytes);
                let hash_fr = Fr::from_le_bytes_mod_order(hash_bytes);
                let workspace_fr = Fr::from_le_bytes_mod_order(&wid);
                let claimer_hash = blake3::hash(&claimer.did_peer[..claimer.did_peer_len as usize]);
                let claimer_fr = Fr::from_le_bytes_mod_order(claimer_hash.as_bytes());
                let expiry_fr = Fr::from(expiry_time);

                // Generate and verify proof
                let computed_hash = code_fr + nonce_fr * claimer_fr + workspace_fr * expiry_fr;

                if computed_hash == hash_fr {
                    // Add user to workspace
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

// ffi.rs - Add these dummy functions to your cyan-backend

use std::ffi::c_char;

// MARK: - Tombstone Operations
#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_group(group_id: *const u8) -> bool {
    // Dummy: Just return success
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_workspace(workspace_id: *const u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_tombstone_board(board_id: *const u8) -> bool {
    true
}

// MARK: - Rename Operations
#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_group(group_id: *const u8, new_name: *const c_char) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_workspace(workspace_id: *const u8, new_name: *const c_char) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn cyan_rename_board(board_id: *const u8, new_name: *const c_char) -> bool {
    true
}

// MARK: - File Operations
#[unsafe(no_mangle)]
pub extern "C" fn cyan_add_file_to_group(
    group_id: *const u8,
    file_path: *const c_char,
    file_data: *const u8,
    file_size: usize,
    out_file_id: *mut u8,
) -> bool {
    unsafe {
        // Generate dummy file ID (32 bytes)
        let dummy_id = b"file12345678901234567890123456789";
        std::ptr::copy_nonoverlapping(dummy_id.as_ptr(), out_file_id, 32);
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
    unsafe {
        // Generate dummy file ID (32 bytes)
        let dummy_id = b"file98765432109876543210987654321";
        std::ptr::copy_nonoverlapping(dummy_id.as_ptr(), out_file_id, 32);
    }
    true
}
