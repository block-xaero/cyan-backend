// storage.rs - Complete with hierarchical event chain support

use bytemuck::Zeroable;
use xaeroflux_actors::{read_api::ReadApi, XaeroFlux};
use xaeroflux_core::pool::XaeroInternalEvent;

use crate::{objects::*, Board, Comment, Group, Workspace};

pub const DATA_DIR: &str = "/tmp/cyan-data";

// Event types for Cyan operations
const EVENT_TYPE_GROUP: u32 = 100;
const EVENT_TYPE_WORKSPACE: u32 = 101;
const EVENT_TYPE_BOARD: u32 = 102;
const EVENT_TYPE_COMMENT: u32 = 103;
const EVENT_TYPE_WHITEBOARD_OBJECT: u32 = 104;
const EVENT_TYPE_INVITATION: u32 = 105;

// ================================================================================================
// HIERARCHICAL EVENT HELPERS
// ================================================================================================

pub fn build_hierarchical_event(
    current_id: [u8; 32],
    ancestry: Vec<[u8; 32]>,
    data: &[u8],
) -> Vec<u8> {
    let mut event = Vec::with_capacity(32 * (ancestry.len() + 2) + data.len());

    // Add current ID
    event.extend_from_slice(&current_id);

    // Add ancestry chain
    for ancestor in ancestry {
        event.extend_from_slice(&ancestor);
    }

    // Add null terminator for hierarchy
    event.extend_from_slice(&[0u8; 32]);

    // Add actual data
    event.extend_from_slice(data);

    event
}

pub fn parse_hierarchical_event(event_data: &[u8]) -> (Vec<[u8; 32]>, &[u8]) {
    let mut ancestry = Vec::new();
    let mut offset = 0;

    // Parse hierarchy chain until we hit null terminator
    while offset + 32 <= event_data.len() {
        let mut id = [0u8; 32];
        id.copy_from_slice(&event_data[offset..offset + 32]);

        if id == [0u8; 32] {
            offset += 32;
            break;
        }

        ancestry.push(id);
        offset += 32;
    }

    (ancestry, &event_data[offset..])
}

fn create_comment<const N: usize>() -> Comment<N> {
    unsafe { std::mem::zeroed() }
}

// ================================================================================================
// GROUP OPERATIONS
// ================================================================================================

pub fn create_group<const N: usize>(
    creator: [u8; 32],
    name: &str,
    icon: &str,
    color: [u8; 4],
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let group_id = blake3::hash(
        format!(
            "{:?}{}{}",
            creator,
            name,
            xaeroflux_core::date_time::emit_nanos()
        )
        .as_bytes(),
    )
    .into();

    let mut group: Group<N> = Group::zeroed();
    group.group_id = group_id;
    group.created_by = creator;

    // Copy name
    let name_bytes = name.as_bytes();
    let copy_len = std::cmp::min(name_bytes.len(), 64);
    group.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    // Copy icon
    let icon_bytes = icon.as_bytes();
    let icon_len = std::cmp::min(icon_bytes.len(), 32);
    group.icon[..icon_len].copy_from_slice(&icon_bytes[..icon_len]);

    group.color = color;
    group.created_at = xaeroflux_core::date_time::emit_secs();

    // Build hierarchical event (group has no parent)
    let event = build_hierarchical_event(
        group_id,
        vec![], // No ancestry for top-level group
        bytemuck::bytes_of(&group),
    );

    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_GROUP))?;

    tracing::info!(
        "Created group {} with id {:?}",
        name,
        hex::encode(&group_id)
    );
    Ok(group_id)
}

pub fn list_all_groups<const N: usize>() -> Result<Vec<Group<N>>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    use xaeroflux_actors::read_api::{RangeQuery, ReadApi};

    let query = RangeQuery::<{ rusted_ring::S_TSHIRT_SIZE }> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_GROUP,
    };

    let events = xf.range_query(query)?;

    let mut groups = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() == std::mem::size_of::<Group<N>>() {
            let group = bytemuck::from_bytes::<Group<N>>(data);
            groups.push(*group);
        }
    }

    tracing::info!("Listed {} groups", groups.len());
    Ok(groups)
}

pub fn get_group_by_group_id<const N: usize>(
    group_id: [u8; 32],
) -> Result<Group<N>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    match xf.find_current_state_by_eid::<N>(group_id)? {
        Some(event) => {
            let (_ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            let group = bytemuck::from_bytes::<Group<N>>(data);
            Ok(*group)
        }
        None => Err("Group not found".into()),
    }
}

// ================================================================================================
// WORKSPACE OPERATIONS
// ================================================================================================

pub fn create_workspace<const N: usize>(
    creator: [u8; 32],
    group_id: [u8; 32],
    name: &str,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let workspace_id = blake3::hash(
        format!(
            "{:?}{:?}{}{}",
            creator,
            group_id,
            name,
            xaeroflux_core::date_time::emit_nanos()
        )
        .as_bytes(),
    )
    .into();

    let mut workspace = Workspace::<N>::zeroed();
    workspace.workspace_id = workspace_id;
    workspace.group_id = group_id;

    // Copy name
    let name_bytes = name.as_bytes();
    let copy_len = std::cmp::min(name_bytes.len(), 64);
    workspace.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    workspace.created_at = xaeroflux_core::date_time::emit_secs();

    // Build hierarchical event
    let event = build_hierarchical_event(
        workspace_id,
        vec![group_id], // Parent is group
        bytemuck::bytes_of(&workspace),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_WORKSPACE)?;

    tracing::info!(
        "Created workspace {} with id {:?}",
        name,
        hex::encode(&workspace_id)
    );
    Ok(workspace_id)
}

pub fn list_workspaces_for_group<const N: usize>(
    group_id: [u8; 32],
) -> Result<Vec<Workspace<N>>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    use xaeroflux_actors::read_api::{RangeQuery, ReadApi};

    let query = RangeQuery::<{ rusted_ring::M_TSHIRT_SIZE }> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_WORKSPACE,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::M_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &xaeroflux_core::pool::XaeroInternalEvent<{rusted_ring
        ::M_TSHIRT_SIZE}>| {
            let (ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            // Check if group_id is in ancestry (should be at index 1)
            if ancestry.len() >= 2 && ancestry[1] == group_id {
                data.len() == std::mem::size_of::<Workspace<{rusted_ring::M_TSHIRT_SIZE}>>()
            } else {
                false
            }
        }),
    )?;

    let mut workspaces = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        let workspace = bytemuck::from_bytes::<Workspace<N>>(data);
        workspaces.push(*workspace);
    }

    tracing::info!("Listed {} workspaces for group", workspaces.len());
    Ok(workspaces)
}

pub fn get_workspaces_for_group<const N: usize, const M: usize>(
    group_id: [u8; 32],
) -> Result<([Workspace<N>; M], usize), Box<dyn std::error::Error>> {
    let workspaces_vec = list_workspaces_for_group::<N>(group_id)?;

    let mut workspaces: [Workspace<N>; M] = unsafe { std::mem::zeroed() };
    let count = std::cmp::min(workspaces_vec.len(), M);

    for i in 0..count {
        workspaces[i] = workspaces_vec[i];
    }

    Ok((workspaces, count))
}

// ================================================================================================
// BOARD OPERATIONS
// ================================================================================================

pub fn create_board<const N: usize>(
    creator: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    name: &str,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let board_id = blake3::hash(
        format!(
            "{:?}{:?}{}{}",
            creator,
            workspace_id,
            name,
            xaeroflux_core::date_time::emit_nanos()
        )
        .as_bytes(),
    )
    .into();

    let mut board = Board::<N>::zeroed();
    board.board_id = board_id;
    board.workspace_id = workspace_id;
    board.group_id = group_id;

    // Copy name
    let name_bytes = name.as_bytes();
    let copy_len = std::cmp::min(name_bytes.len(), 64);
    board.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    board.created_at = xaeroflux_core::date_time::emit_secs();

    // Build hierarchical event
    let event = build_hierarchical_event(
        board_id,
        vec![workspace_id, group_id], // Parent chain
        bytemuck::bytes_of(&board),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_BOARD)?;

    tracing::info!(
        "Created board {} with id {:?}",
        name,
        hex::encode(&board_id)
    );
    Ok(board_id)
}

// ================================================================================================
// WHITEBOARD OBJECT OPERATIONS
// ================================================================================================

pub fn add_whiteboard_object<T: bytemuck::Pod + HierarchicalEvent>(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    object_type: u8,
    object_data: &T,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let object_id = object_data.get_id();

    // Build hierarchical event with full ancestry
    let event = build_hierarchical_event(
        object_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(object_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_WHITEBOARD_OBJECT + object_type as u32)?;

    tracing::info!(
        "Added object type {} to board {:?}",
        object_type,
        hex::encode(&board_id)
    );
    Ok(object_id)
}

// ================================================================================================
// COMMENT OPERATIONS
// ================================================================================================

pub fn add_comment<const N: usize>(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    author_id: [u8; 32],
    author_name: &str,
    content: &str,
    parent_id: Option<[u8; 32]>,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let comment_id = blake3::hash(
        format!(
            "{:?}{:?}{}{}",
            board_id,
            author_id,
            content,
            xaeroflux_core::date_time::emit_nanos()
        )
        .as_bytes(),
    )
    .into();

    let mut comment = crate::storage::create_comment::<N>();
    comment.comment_id = comment_id;
    comment.board_id = board_id;
    comment.author_id = author_id;

    // Copy author name
    let name_bytes = author_name.as_bytes();
    let name_len = std::cmp::min(name_bytes.len(), 64);
    comment.author_name[..name_len].copy_from_slice(&name_bytes[..name_len]);

    // Copy content
    let content_bytes = content.as_bytes();
    let content_len = std::cmp::min(content_bytes.len(), N);
    comment.content[..content_len].copy_from_slice(&content_bytes[..content_len]);

    if let Some(pid) = parent_id {
        comment.parent_id = pid;
    }

    comment.created_at = xaeroflux_core::date_time::emit_secs();

    // Build hierarchical event
    let mut ancestry = vec![board_id, workspace_id, group_id];
    if let Some(pid) = parent_id {
        ancestry.insert(0, pid); // Parent comment is immediate parent
    }

    let event = build_hierarchical_event(comment_id, ancestry, bytemuck::bytes_of(&comment));

    XaeroFlux::write_event_static(&event, EVENT_TYPE_COMMENT)?;

    tracing::info!("Added comment with id {:?}", hex::encode(&comment_id));
    Ok(comment_id)
}

// ================================================================================================
// INVITATION OPERATIONS
// ================================================================================================

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InvitationRecord {
    pub invitation_hash: [u8; 32],
    pub workspace_id: [u8; 32],
    pub inviter_id: [u8; 32],
    pub invitee_id: [u8; 32],
    pub expiry_time: u64,
    pub created_at: u64,
    pub _padding: [u8; 48],
}

pub fn store_invitation(invitation: InvitationRecord) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        invitation.invitation_hash,
        vec![invitation.workspace_id], // Parent is workspace
        bytemuck::bytes_of(&invitation),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_INVITATION)?;
    Ok(())
}

pub fn get_invitation<const N: usize>(
    hash: &[u8; 32],
) -> Result<InvitationRecord, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    match xf.find_current_state_by_eid::<N>(*hash)? {
        Some(event) => {
            let (_ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            let invitation = bytemuck::from_bytes::<InvitationRecord>(data);
            Ok(*invitation)
        }
        None => Err("Invitation not found".into()),
    }
}

pub fn add_user_to_workspace(
    workspace_id: [u8; 32],
    user_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(
        "Added user {:?} to workspace {:?}",
        hex::encode(&user_id),
        hex::encode(&workspace_id)
    );
    Ok(())
}

// ================================================================================================
// WORKSPACE RETRIEVAL
// ================================================================================================

pub struct WorkspaceWithObjects {
    pub workspace: Workspace<{rusted_ring::M_TSHIRT_SIZE}>,
    pub whiteboards: Vec<Board<32>>,
}

pub fn get_workspace_with_all_objects(
    workspace_id: [u8; 32],
) -> Result<WorkspaceWithObjects, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    // Get workspace
    let workspace = match xf.find_current_state_by_eid::<{rusted_ring::M_TSHIRT_SIZE}>
(workspace_id)? {
        Some(event) => {
            let (_ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            let ws = bytemuck::from_bytes::<Workspace<{rusted_ring::M_TSHIRT_SIZE}>>(data);
            *ws
        }
        None => return Err("Workspace not found".into()),
    };

    // Get boards for workspace
    use xaeroflux_actors::read_api::{RangeQuery, ReadApi};

    let query = RangeQuery::<{ rusted_ring::M_TSHIRT_SIZE }> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_BOARD,
    };

    let events = xf.range_query_with_filter::<{ rusted_ring::M_TSHIRT_SIZE }, _>(
        query,
        Box::new(
            move |event: &XaeroInternalEvent<{ rusted_ring::M_TSHIRT_SIZE }>| {
                let (ancestry, data) =
                    parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
                // Check if workspace_id is in ancestry
                if ancestry.len() >= 2 && ancestry.contains(&workspace_id) {
                    data.len() == std::mem::size_of::<Board<32>>()
                } else {
                    false
                }
            },
        ),
    )?;

    let mut whiteboards = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        let board = bytemuck::from_bytes::<Board<32>>(data);
        whiteboards.push(*board);
    }

    Ok(WorkspaceWithObjects {
        workspace,
        whiteboards,
    })
}
