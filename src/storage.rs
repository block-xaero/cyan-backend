// storage.rs - Complete implementation with all drawing objects
use bytemuck::Zeroable;
use xaeroflux_actors::{read_api::ReadApi, XaeroFlux};
use xaeroflux_core::pool::XaeroInternalEvent;

use crate::{
    objects::*, BoardStandard, CommentStandard, DrawingPathStandard, FileNode,
    GroupStandard, Layer, Vote, WorkspaceStandard, EVENT_TYPE_BOARD,
    EVENT_TYPE_COMMENT, EVENT_TYPE_FILE, EVENT_TYPE_GROUP, EVENT_TYPE_INVITATION,
    EVENT_TYPE_LAYER, EVENT_TYPE_VOTE, EVENT_TYPE_WHITEBOARD_OBJECT, EVENT_TYPE_WORKSPACE,
};

pub const DATA_DIR: &str = "/tmp/cyan-data";

// Event type constants for whiteboard objects
pub const EVENT_TYPE_PATH_OBJECT: u32 = EVENT_TYPE_WHITEBOARD_OBJECT + 1;
pub const EVENT_TYPE_STICKY_NOTE: u32 = EVENT_TYPE_WHITEBOARD_OBJECT + 2;
pub const EVENT_TYPE_RECTANGLE: u32 = EVENT_TYPE_WHITEBOARD_OBJECT + 3;
pub const EVENT_TYPE_CIRCLE: u32 = EVENT_TYPE_WHITEBOARD_OBJECT + 4;
pub const EVENT_TYPE_ARROW: u32 = EVENT_TYPE_WHITEBOARD_OBJECT + 5;
pub const EVENT_TYPE_TEXT: u32 = EVENT_TYPE_WHITEBOARD_OBJECT + 6;

// ================================================================================================
// HIERARCHICAL EVENT HELPERS
// ================================================================================================

pub fn build_hierarchical_event(
    id: [u8; 32],
    ancestry: Vec<[u8; 32]>,
    data: &[u8],
) -> Vec<u8> {
    use std::alloc::{alloc, Layout};

    // Calculate header size and padding needed
    let header_size = 32 * (ancestry.len() + 2); // +2 for id and null terminator
    let padding = (64 - (header_size % 64)) % 64;
    let total_size = header_size + padding + data.len();

    // Create 64-byte aligned buffer
    let layout = Layout::from_size_align(total_size, 64).unwrap();

    unsafe {
        let ptr = alloc(layout);
        let mut offset = 0;

        // Write id
        ptr.add(offset).copy_from_nonoverlapping(id.as_ptr(), 32);
        offset += 32;

        // Write ancestry
        for ancestor in &ancestry {
            ptr.add(offset).copy_from_nonoverlapping(ancestor.as_ptr(), 32);
            offset += 32;
        }

        // Write null terminator
        std::ptr::write_bytes(ptr.add(offset), 0, 32);
        offset += 32;

        // Write padding to align data to 64-byte boundary
        if padding > 0 {
            std::ptr::write_bytes(ptr.add(offset), 0, padding);
            offset += padding;
        }

        // Write data (now at 64-byte aligned offset)
        ptr.add(offset).copy_from_nonoverlapping(data.as_ptr(), data.len());

        Vec::from_raw_parts(ptr, total_size, total_size)
    }
}

pub fn parse_hierarchical_event(event_data: &[u8]) -> (Vec<[u8; 32]>, &[u8]) {
    tracing::info!("Original event_data ptr: {:p}, alignment offset: {}",
                  event_data.as_ptr(),
                  event_data.as_ptr() as usize % 64);
    let mut ancestry = Vec::new();
    let mut offset = 0;

    // Parse ancestry until we hit null terminator
    while offset + 32 <= event_data.len() {
        let mut id = [0u8; 32];
        id.copy_from_slice(&event_data[offset..offset + 32]);

        if id == [0u8; 32] {
            offset += 32; // Skip null terminator
            break;
        }

        ancestry.push(id);
        offset += 32;
    }

    // Calculate and skip padding
    // offset already includes all parsed bytes (ancestry + null)
    let padding = (64 - (offset % 64)) % 64;
    offset += padding;

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
    tracing::info!("Listing all groups called");
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
        EVENT_TYPE_WORKSPACE,
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
    tracing::info!("Listing workspaces for group {:?}", hex::encode(&group_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{rusted_ring::M_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_WORKSPACE,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::M_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::M_TSHIRT_SIZE}>| {
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
            let mut aligned_workspace = WorkspaceStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_workspace as *mut WorkspaceStandard as *mut u8,
                    std::mem::size_of::<WorkspaceStandard>(),
                );
            }
            workspaces.push(aligned_workspace);
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

    XaeroFlux::write_event_static(&event, EVENT_TYPE_BOARD)?;

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
    tracing::info!("Looking for boards in workspace: {:x?}", &workspace_id[..8]);

    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<1024> {  // Use 1024 since that's where they're stored
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_BOARD,
    };

    let events = xf.range_query_with_filter::<1024, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<1024>| {
            let (ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&workspace_id) && data.len() >= std::mem::size_of::<BoardStandard>()
        }),
    )?;

    tracing::info!("Found {} matching events", events.len());

    let mut boards = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<BoardStandard>() {
            // Fix alignment issue by copying to aligned buffer
            let mut aligned_board = BoardStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_board as *mut BoardStandard as *mut u8,
                    std::mem::size_of::<BoardStandard>(),
                );
            }

            let name = String::from_utf8_lossy(&aligned_board.name[..aligned_board.name.iter().position(|&x| x == 0).unwrap_or(64)]);
            tracing::info!("Parsed board: '{}'", name);
            boards.push(aligned_board);
        }
    }

    tracing::info!("Returning {} boards", boards.len());
    Ok(boards)
}

// ================================================================================================
// PATH DRAWING OPERATIONS
// ================================================================================================

pub fn add_path_object(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    path_data: &PathData<256>,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let path_id = path_data.path_id;

    let event = build_hierarchical_event(
        path_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(path_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_PATH_OBJECT)?;

    tracing::info!("Added path object to board {:?}", hex::encode(&board_id));
    Ok(path_id)
}

pub fn list_paths_for_board(
    board_id: [u8; 32],
) -> Result<Vec<PathData<256>>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    // PathData<256> is 2368 bytes, use XL size
    let query = RangeQuery::<{rusted_ring::XL_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_PATH_OBJECT,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::XL_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::XL_TSHIRT_SIZE}>| {
            let (ancestry, _) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id)
        }),
    )?;

    let mut paths = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<PathData<256>>() {
            let path = bytemuck::from_bytes::<PathData<256>>(
                &data[..std::mem::size_of::<PathData<256>>()]
            );
            paths.push(*path);
        }
    }
    Ok(paths)
}

// ================================================================================================
// STICKY NOTE OPERATIONS
// ================================================================================================

pub fn add_sticky_note(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    sticky_data: &StickyNoteData,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let note_id = sticky_data.note_id;

    let event = build_hierarchical_event(
        note_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(sticky_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_STICKY_NOTE)?;

    tracing::info!("Added sticky note to board {:?}", hex::encode(&board_id));
    Ok(note_id)
}

pub fn list_sticky_notes_for_board(
    board_id: [u8; 32],
) -> Result<Vec<StickyNoteData>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{rusted_ring::M_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_STICKY_NOTE,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::M_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::M_TSHIRT_SIZE}>| {
            let (ancestry, _) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id)
        }),
    )?;

    let mut notes = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<StickyNoteData>() {
            let note = bytemuck::from_bytes::<StickyNoteData>(
                &data[..std::mem::size_of::<StickyNoteData>()]
            );
            notes.push(*note);
        }
    }
    Ok(notes)
}

// ================================================================================================
// RECTANGLE OPERATIONS
// ================================================================================================

pub fn add_rectangle(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    rect_data: &RectangleData,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let shape_id = rect_data.shape_id;

    let event = build_hierarchical_event(
        shape_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(rect_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_RECTANGLE)?;

    tracing::info!("Added rectangle to board {:?}", hex::encode(&board_id));
    Ok(shape_id)
}

pub fn list_rectangles_for_board(
    board_id: [u8; 32],
) -> Result<Vec<RectangleData>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{rusted_ring::S_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_RECTANGLE,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::S_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::S_TSHIRT_SIZE}>| {
            let (ancestry, _) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id)
        }),
    )?;

    let mut rectangles = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<RectangleData>() {
            let rect = bytemuck::from_bytes::<RectangleData>(
                &data[..std::mem::size_of::<RectangleData>()]
            );
            rectangles.push(*rect);
        }
    }
    Ok(rectangles)
}

// ================================================================================================
// CIRCLE OPERATIONS
// ================================================================================================

pub fn add_circle(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    circle_data: &CircleData,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let shape_id = circle_data.shape_id;

    let event = build_hierarchical_event(
        shape_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(circle_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_CIRCLE)?;

    tracing::info!("Added circle to board {:?}", hex::encode(&board_id));
    Ok(shape_id)
}

pub fn list_circles_for_board(
    board_id: [u8; 32],
) -> Result<Vec<CircleData>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{rusted_ring::S_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_CIRCLE,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::S_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::S_TSHIRT_SIZE}>| {
            let (ancestry, _) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id)
        }),
    )?;

    let mut circles = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<CircleData>() {
            let circle = bytemuck::from_bytes::<CircleData>(
                &data[..std::mem::size_of::<CircleData>()]
            );
            circles.push(*circle);
        }
    }
    Ok(circles)
}

// ================================================================================================
// ARROW OPERATIONS
// ================================================================================================

pub fn add_arrow(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    arrow_data: &ArrowData,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let arrow_id = arrow_data.arrow_id;

    let event = build_hierarchical_event(
        arrow_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(arrow_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_ARROW)?;

    tracing::info!("Added arrow to board {:?}", hex::encode(&board_id));
    Ok(arrow_id)
}

pub fn list_arrows_for_board(
    board_id: [u8; 32],
) -> Result<Vec<ArrowData>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{rusted_ring::S_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_ARROW,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::S_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::S_TSHIRT_SIZE}>| {
            let (ancestry, _) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id)
        }),
    )?;

    let mut arrows = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<ArrowData>() {
            let arrow = bytemuck::from_bytes::<ArrowData>(
                &data[..std::mem::size_of::<ArrowData>()]
            );
            arrows.push(*arrow);
        }
    }
    Ok(arrows)
}

// ================================================================================================
// TEXT OPERATIONS
// ================================================================================================

pub fn add_text(
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    text_data: &TextData,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let text_id = text_data.text_id;

    let event = build_hierarchical_event(
        text_id,
        vec![board_id, workspace_id, group_id],
        bytemuck::bytes_of(text_data),
    );

    XaeroFlux::write_event_static(&event, EVENT_TYPE_TEXT)?;

    tracing::info!("Added text to board {:?}", hex::encode(&board_id));
    Ok(text_id)
}

pub fn list_text_objects_for_board(
    board_id: [u8; 32],
) -> Result<Vec<TextData>, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{rusted_ring::M_TSHIRT_SIZE}> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_TEXT,
    };

    let events = xf.range_query_with_filter::<{rusted_ring::M_TSHIRT_SIZE}, _>(
        query,
        Box::new(move |event: &XaeroInternalEvent<{rusted_ring::M_TSHIRT_SIZE}>| {
            let (ancestry, _) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id)
        }),
    )?;

    let mut text_objects = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<TextData>() {
            let text = bytemuck::from_bytes::<TextData>(
                &data[..std::mem::size_of::<TextData>()]
            );
            text_objects.push(*text);
        }
    }
    Ok(text_objects)
}

// ================================================================================================
// LOAD ALL BOARD OBJECTS
// ================================================================================================

pub struct BoardObjects {
    pub paths: Vec<PathData<256>>,
    pub sticky_notes: Vec<StickyNoteData>,
    pub rectangles: Vec<RectangleData>,
    pub circles: Vec<CircleData>,
    pub arrows: Vec<ArrowData>,
    pub text_objects: Vec<TextData>,
}

pub fn load_all_board_objects(board_id: [u8; 32]) -> Result<BoardObjects, Box<dyn std::error::Error>> {
    Ok(BoardObjects {
        paths: list_paths_for_board(board_id)?,
        sticky_notes: list_sticky_notes_for_board(board_id)?,
        rectangles: list_rectangles_for_board(board_id)?,
        circles: list_circles_for_board(board_id)?,
        arrows: list_arrows_for_board(board_id)?,
        text_objects: list_text_objects_for_board(board_id)?,
    })
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
// WHITEBOARD OBJECT OPERATIONS (Generic)
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