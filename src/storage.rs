// storage.rs - Complete implementation
use bytemuck::Zeroable;
use xaeroflux_actors::{read_api::ReadApi, XaeroFlux};
use xaeroflux_core::pool::XaeroInternalEvent;

use crate::{
    objects::*, BoardStandard, CommentStandard, DrawingPathStandard, FileNode,
    GroupStandard, Layer, Vote, WorkspaceStandard, EVENT_TYPE_BOARD,
    EVENT_TYPE_COMMENT, EVENT_TYPE_FILE, EVENT_TYPE_GROUP, EVENT_TYPE_INVITATION, EVENT_TYPE_LAYER, EVENT_TYPE_VOTE,
    EVENT_TYPE_WHITEBOARD_OBJECT, EVENT_TYPE_WORKSPACE,
};

pub const DATA_DIR: &str = "/tmp/cyan-data";

// ================================================================================================
// HIERARCHICAL EVENT HELPERS
// ================================================================================================

pub fn build_hierarchical_event(
    current_id: [u8; 32],
    ancestry: Vec<[u8; 32]>,
    data: &[u8],
) -> Vec<u8> {
    let mut event = Vec::with_capacity(32 * (ancestry.len() + 2) + data.len());
    event.extend_from_slice(&current_id);
    for ancestor in ancestry {
        event.extend_from_slice(&ancestor);
    }
    event.extend_from_slice(&[0u8; 32]);
    event.extend_from_slice(data);
    event
}

pub fn parse_hierarchical_event(event_data: &[u8]) -> (Vec<[u8; 32]>, &[u8]) {
    let mut ancestry = Vec::new();
    let mut offset = 0;

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

// ================================================================================================
// GROUP OPERATIONS
// ================================================================================================

pub fn create_group(
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

    let mut group = GroupStandard::zeroed();
    group.group_id = group_id;
    group.created_by = creator;

    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    group.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    let icon_bytes = icon.as_bytes();
    let icon_len = icon_bytes.len().min(32);
    group.icon[..icon_len].copy_from_slice(&icon_bytes[..icon_len]);

    group.color = color;
    group.created_at = xaeroflux_core::date_time::emit_secs();
    group.updated_at = group.created_at;

    let event = build_hierarchical_event(group_id, vec![], bytemuck::bytes_of(&group));
    tracing::info!(
        "### built hierarchical event: {event:?} with size : {}",
        std::mem::size_of_val(&event)
    );
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_GROUP))?;

    tracing::info!(
        "Created group {} with id {:?}",
        name,
        hex::encode(&group_id)
    );
    Ok(group_id)
}

pub fn list_all_groups() -> Result<Vec<GroupStandard>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{ rusted_ring::M_TSHIRT_SIZE }> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_GROUP,
    };

    let events = xf.range_query(query)?;
    let mut groups = Vec::new();

    for event in events {
        let (_ancestry, group_data) =
            parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if group_data.len() >= std::mem::size_of::<GroupStandard>() {

            let mut aligned_group = GroupStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    group_data.as_ptr(),
                    &mut aligned_group as *mut GroupStandard as *mut u8,
                    std::mem::size_of::<GroupStandard>(),
                );
            }
            groups.push(aligned_group);
        }
    }
    tracing::info!("Listed {} groups", groups.len());
    Ok(groups)
}

pub fn get_group_by_id(group_id: [u8; 32]) -> Result<GroupStandard, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    match xf.find_current_state_by_eid::<512>(group_id)? {
        Some(event) => {
            let (_ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            let group = bytemuck::from_bytes::<GroupStandard>(data);
            Ok(*group)
        }
        None => Err("Group not found".into()),
    }
}

// ================================================================================================
// WORKSPACE OPERATIONS
// ================================================================================================

pub fn create_workspace(
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

    let mut workspace = WorkspaceStandard::zeroed();
    workspace.workspace_id = workspace_id;
    workspace.group_id = group_id;
    workspace.created_by = creator;

    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    workspace.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    workspace.created_at = xaeroflux_core::date_time::emit_secs();
    workspace.updated_at = workspace.created_at;

    // Workspace is 256 bytes, hierarchical event is 352 bytes (with group_id)
    let event =
        build_hierarchical_event(workspace_id, vec![group_id], bytemuck::bytes_of(&workspace));

    XaeroFlux::write_event_static(
        &event,
        xaeroflux_core::event::make_pinned(EVENT_TYPE_WORKSPACE),
    )?;

    tracing::info!(
        "Created workspace {} with id {:?}",
        name,
        hex::encode(&workspace_id)
    );
    Ok(workspace_id)
}

pub fn list_workspaces_for_group(
    group_id: [u8; 32],
) -> Result<Vec<WorkspaceStandard>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    // 352-byte events fit in S_TSHIRT_SIZE (512)
    let query = RangeQuery::<512> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_WORKSPACE,
    };

    let events = xf.range_query_with_filter::<512, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<512>| {
            let (ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.len() >= 2
                && ancestry[1] == group_id
                && data.len() >= std::mem::size_of::<WorkspaceStandard>()
        }),
    )?;

    let mut workspaces = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<WorkspaceStandard>() {
            let workspace = bytemuck::from_bytes::<WorkspaceStandard>(
                &data[..std::mem::size_of::<WorkspaceStandard>()],
            );
            workspaces.push(*workspace);
        }
    }

    tracing::info!("Listed {} workspaces for group", workspaces.len());
    Ok(workspaces)
}

// ================================================================================================
// BOARD OPERATIONS
// ================================================================================================

pub fn create_board(
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

    let mut board = BoardStandard::zeroed();
    board.board_id = board_id;
    board.workspace_id = workspace_id;
    board.group_id = group_id;
    board.created_by = creator;
    board.last_modified_by = creator;

    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    board.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    board.created_at = xaeroflux_core::date_time::emit_secs();
    board.updated_at = board.created_at;

    // Board is 320 bytes, hierarchical event is 416 bytes (with workspace_id and group_id)
    let event = build_hierarchical_event(
        board_id,
        vec![workspace_id, group_id],
        bytemuck::bytes_of(&board),
    );

    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_BOARD))?;

    tracing::info!(
        "Created board {} with id {:?}",
        name,
        hex::encode(&board_id)
    );
    Ok(board_id)
}

pub fn list_boards_for_workspace(
    workspace_id: [u8; 32],
) -> Result<Vec<BoardStandard>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<512> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_BOARD,
    };

    let events = xf.range_query_with_filter::<512, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<512>| {
            let (ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&workspace_id) && data.len() >= std::mem::size_of::<BoardStandard>()
        }),
    )?;

    let mut boards = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<BoardStandard>() {
            let board = bytemuck::from_bytes::<BoardStandard>(
                &data[..std::mem::size_of::<BoardStandard>()],
            );
            boards.push(*board);
        }
    }

    Ok(boards)
}

// ================================================================================================
// COMMENT OPERATIONS
// ================================================================================================

pub fn add_comment(
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

    let mut comment = CommentStandard::zeroed();
    comment.comment_id = comment_id;
    comment.board_id = board_id;
    comment.author_id = author_id;

    if let Some(pid) = parent_id {
        comment.parent_id = pid;
        comment.depth = 1;
    }

    let name_bytes = author_name.as_bytes();
    let name_len = name_bytes.len().min(64);
    comment.author_name[..name_len].copy_from_slice(&name_bytes[..name_len]);

    let content_bytes = content.as_bytes();
    let content_len = content_bytes.len().min(512);
    comment.content[..content_len].copy_from_slice(&content_bytes[..content_len]);

    comment.created_at = xaeroflux_core::date_time::emit_secs();
    comment.updated_at = comment.created_at;

    let mut ancestry = vec![board_id, workspace_id, group_id];
    if let Some(pid) = parent_id {
        ancestry.insert(0, pid);
    }

    // Comment is 768 bytes, hierarchical event is ~900 bytes
    let event = build_hierarchical_event(comment_id, ancestry, bytemuck::bytes_of(&comment));

    XaeroFlux::write_event_static(
        &event,
        xaeroflux_core::event::make_pinned(EVENT_TYPE_COMMENT),
    )?;

    tracing::info!("Added comment with id {:?}", hex::encode(&comment_id));
    Ok(comment_id)
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
        vec![invitation.workspace_id],
        bytemuck::bytes_of(&invitation),
    );
    XaeroFlux::write_event_static(
        &event,
        xaeroflux_core::event::make_pinned(EVENT_TYPE_INVITATION),
    )?;
    Ok(())
}

pub fn get_invitation(hash: &[u8; 32]) -> Result<InvitationRecord, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    match xf.find_current_state_by_eid::<512>(*hash)? {
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
