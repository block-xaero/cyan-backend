// storage.rs - Complete implementation with all functions
use bytemuck::Zeroable;
use rusted_ring::M_TSHIRT_SIZE;
use xaeroflux::event::make_pinned;
use xaeroflux_actors::{read_api::ReadApi, XaeroFlux};
use xaeroflux_core::{
    event::{CREATE_EVENT_TYPE_BASE, TOMBSTONE_EVENT_TYPE_BASE, UPDATE_EVENT_TYPE_BASE},
    pool::XaeroInternalEvent,
};

use crate::{
    objects::*, BoardStandard, CommentStandard, DrawingPathStandard, FileNode, GroupStandard, Layer,
    Vote, WorkspaceStandard,
};

pub const DATA_DIR: &str = "/tmp/cyan-data";

// Base event types for Cyan application (starting from 1008)
pub const EVENT_TYPE_GROUP: u32 = CREATE_EVENT_TYPE_BASE; // 1008
pub const EVENT_TYPE_WORKSPACE: u32 = CREATE_EVENT_TYPE_BASE + 1; // 1009
pub const EVENT_TYPE_BOARD: u32 = CREATE_EVENT_TYPE_BASE + 2; // 1010
pub const EVENT_TYPE_COMMENT: u32 = CREATE_EVENT_TYPE_BASE + 3; // 1011
pub const EVENT_TYPE_FILE: u32 = CREATE_EVENT_TYPE_BASE + 4; // 1012
pub const EVENT_TYPE_INVITATION: u32 = CREATE_EVENT_TYPE_BASE + 5; // 1013
pub const EVENT_TYPE_LAYER: u32 = CREATE_EVENT_TYPE_BASE + 6; // 1014
pub const EVENT_TYPE_VOTE: u32 = CREATE_EVENT_TYPE_BASE + 7; // 1015

// Whiteboard object types
pub const EVENT_TYPE_PATH: u32 = CREATE_EVENT_TYPE_BASE + 10; // 1018
pub const EVENT_TYPE_STICKY_NOTE: u32 = CREATE_EVENT_TYPE_BASE + 11; // 1019
pub const EVENT_TYPE_RECTANGLE: u32 = CREATE_EVENT_TYPE_BASE + 12; // 1020
pub const EVENT_TYPE_CIRCLE: u32 = CREATE_EVENT_TYPE_BASE + 13; // 1021
pub const EVENT_TYPE_ARROW: u32 = CREATE_EVENT_TYPE_BASE + 14; // 1022
pub const EVENT_TYPE_TEXT: u32 = CREATE_EVENT_TYPE_BASE + 15; // 1023

// Update event types (starting from 2008)
pub const EVENT_TYPE_UPDATE_GROUP: u32 = UPDATE_EVENT_TYPE_BASE; // 2008
pub const EVENT_TYPE_UPDATE_WORKSPACE: u32 = UPDATE_EVENT_TYPE_BASE + 1; // 2009
pub const EVENT_TYPE_UPDATE_BOARD: u32 = UPDATE_EVENT_TYPE_BASE + 2; // 2010
pub const EVENT_TYPE_UPDATE_COMMENT: u32 = UPDATE_EVENT_TYPE_BASE + 3; // 2011
pub const EVENT_TYPE_UPDATE_PATH: u32 = UPDATE_EVENT_TYPE_BASE + 10; // 2018
pub const EVENT_TYPE_UPDATE_STICKY_NOTE: u32 = UPDATE_EVENT_TYPE_BASE + 11; // 2019
pub const EVENT_TYPE_UPDATE_RECTANGLE: u32 = UPDATE_EVENT_TYPE_BASE + 12; // 2020
pub const EVENT_TYPE_UPDATE_CIRCLE: u32 = UPDATE_EVENT_TYPE_BASE + 13; // 2021
pub const EVENT_TYPE_UPDATE_ARROW: u32 = UPDATE_EVENT_TYPE_BASE + 14; // 2022
pub const EVENT_TYPE_UPDATE_TEXT: u32 = UPDATE_EVENT_TYPE_BASE + 15; // 2023

// Tombstone event types (starting from 3008)
pub const EVENT_TYPE_TOMBSTONE_GROUP: u32 = TOMBSTONE_EVENT_TYPE_BASE; // 3008
pub const EVENT_TYPE_TOMBSTONE_WORKSPACE: u32 = TOMBSTONE_EVENT_TYPE_BASE + 1; // 3009
pub const EVENT_TYPE_TOMBSTONE_BOARD: u32 = TOMBSTONE_EVENT_TYPE_BASE + 2; // 3010
pub const EVENT_TYPE_TOMBSTONE_COMMENT: u32 = TOMBSTONE_EVENT_TYPE_BASE + 3; // 3011
pub const EVENT_TYPE_TOMBSTONE_PATH: u32 = TOMBSTONE_EVENT_TYPE_BASE + 10; // 3018
pub const EVENT_TYPE_TOMBSTONE_STICKY_NOTE: u32 = TOMBSTONE_EVENT_TYPE_BASE + 11; // 3019
pub const EVENT_TYPE_TOMBSTONE_RECTANGLE: u32 = TOMBSTONE_EVENT_TYPE_BASE + 12; // 3020
pub const EVENT_TYPE_TOMBSTONE_CIRCLE: u32 = TOMBSTONE_EVENT_TYPE_BASE + 13; // 3021
pub const EVENT_TYPE_TOMBSTONE_ARROW: u32 = TOMBSTONE_EVENT_TYPE_BASE + 14; // 3022
pub const EVENT_TYPE_TOMBSTONE_TEXT: u32 = TOMBSTONE_EVENT_TYPE_BASE + 15; // 3023

// Legacy event type mappings (for compatibility)
pub const EVENT_TYPE_WHITEBOARD_OBJECT: u32 = CREATE_EVENT_TYPE_BASE + 20; // 1028
pub const EVENT_TYPE_PATH_OBJECT: u32 = EVENT_TYPE_PATH;
pub const EVENT_TYPE_WHITEBOARD_PATH: u32 = EVENT_TYPE_PATH;
pub const EVENT_TYPE_WHITEBOARD_SHAPE: u32 = EVENT_TYPE_RECTANGLE;
pub const EVENT_TYPE_WHITEBOARD_TEXT: u32 = EVENT_TYPE_TEXT;
pub const EVENT_TYPE_WHITEBOARD_IMAGE: u32 = CREATE_EVENT_TYPE_BASE + 16;
pub const EVENT_TYPE_WHITEBOARD_DELETE: u32 = TOMBSTONE_EVENT_TYPE_BASE + 20;


// Add this debug function to storage.rs

pub fn debug_scan_all_events() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("=== DEBUG: Scanning all events in database ===");
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;
    use std::collections::HashMap;

    let mut event_type_counts: HashMap<u32, usize> = HashMap::new();
    let mut size_distribution: HashMap<&str, HashMap<u32, usize>> = HashMap::new();

    // Check S size (256)
    for event_type in 0..4000 {
        let query = RangeQuery::<{ rusted_ring::S_TSHIRT_SIZE }> {
            xaero_id: [0u8; 32],
            event_type,
        };
        if let Ok(events) = xf.range_query(query) {
            if !events.is_empty() {
                event_type_counts.insert(event_type, events.len());
                size_distribution.entry("S (256)").or_insert_with(HashMap::new).insert(event_type, events.len());
            }
        }
    }

    // Check STANDARD size (512)
    for event_type in 0..4000 {
        let query = RangeQuery::<512> {
            xaero_id: [0u8; 32],
            event_type,
        };
        if let Ok(events) = xf.range_query(query) {
            if !events.is_empty() {
                let count = event_type_counts.entry(event_type).or_insert(0);
                *count += events.len();
                size_distribution.entry("STANDARD (512)").or_insert_with(HashMap::new).insert(event_type, events.len());
            }
        }
    }

    // Check M size (1024)
    for event_type in 0..4000 {
        let query = RangeQuery::<{ rusted_ring::M_TSHIRT_SIZE }> {
            xaero_id: [0u8; 32],
            event_type,
        };
        if let Ok(events) = xf.range_query(query) {
            if !events.is_empty() {
                let count = event_type_counts.entry(event_type).or_insert(0);
                *count += events.len();
                size_distribution.entry("M (1024)").or_insert_with(HashMap::new).insert(event_type, events.len());
            }
        }
    }

    // Check L size (4096)
    for event_type in 0..4000 {
        let query = RangeQuery::<{ rusted_ring::L_TSHIRT_SIZE }> {
            xaero_id: [0u8; 32],
            event_type,
        };
        if let Ok(events) = xf.range_query(query) {
            if !events.is_empty() {
                let count = event_type_counts.entry(event_type).or_insert(0);
                *count += events.len();
                size_distribution.entry("L (4096)").or_insert_with(HashMap::new).insert(event_type, events.len());
            }
        }
    }

    // Check XL size (16384)
    for event_type in 0..4000 {
        let query = RangeQuery::<{ rusted_ring::XL_TSHIRT_SIZE }> {
            xaero_id: [0u8; 32],
            event_type,
        };
        if let Ok(events) = xf.range_query(query) {
            if !events.is_empty() {
                let count = event_type_counts.entry(event_type).or_insert(0);
                *count += events.len();
                size_distribution.entry("XL (16384)").or_insert_with(HashMap::new).insert(event_type, events.len());
            }
        }
    }

    // Print results with interpretation
    println!("\n=== EVENT TYPE ANALYSIS ===");
    let mut sorted_events: Vec<_> = event_type_counts.iter().collect();
    sorted_events.sort_by_key(|&(event_type, _)| event_type);

    for (event_type, count) in sorted_events {
        let name = match *event_type {
            // Expected event types
            1008 => "GROUP (1008)",
            1009 => "WORKSPACE (1009)",
            1010 => "BOARD (1010)",
            1011 => "COMMENT (1011)",
            1012 => "FILE (1012)",
            1013 => "INVITATION (1013)",
            1014 => "LAYER (1014)",
            1015 => "VOTE (1015)",
            1018 => "PATH (1018)",
            1019 => "STICKY_NOTE (1019)",
            1020 => "RECTANGLE (1020)",
            1021 => "CIRCLE (1021)",
            1022 => "ARROW (1022)",
            1023 => "TEXT (1023)",

            // Check for pinned versions
            _ if *event_type == xaeroflux_core::event::make_pinned(1008) => "GROUP (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1009) => "WORKSPACE (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1010) => "BOARD (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1011) => "COMMENT (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1018) => "PATH (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1019) => "STICKY_NOTE (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1020) => "RECTANGLE (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1021) => "CIRCLE (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1022) => "ARROW (PINNED)",
            _ if *event_type == xaeroflux_core::event::make_pinned(1023) => "TEXT (PINNED)",

            _ => "UNKNOWN",
        };

        println!("Event type {}: {} events - {}", event_type, count, name);
    }

    // Print size distribution
    println!("\n=== SIZE DISTRIBUTION ===");
    let size_order = vec!["S (256)", "STANDARD (512)", "M (1024)", "L (4096)", "XL (16384)"];
    for size in size_order {
        if let Some(events) = size_distribution.get(size) {
            println!("\n{} database:", size);
            let mut sorted: Vec<_> = events.iter().collect();
            sorted.sort_by_key(|&(event_type, _)| event_type);
            for (event_type, count) in sorted {
                let name = match *event_type {
                    1008 => "GROUP",
                    1009 => "WORKSPACE",
                    1010 => "BOARD",
                    1011 => "COMMENT",
                    1018 => "PATH",
                    1019 => "STICKY_NOTE",
                    1020 => "RECTANGLE",
                    1021 => "CIRCLE",
                    1022 => "ARROW",
                    1023 => "TEXT",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1008) => "GROUP (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1009) => "WORKSPACE (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1010) => "BOARD (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1011) => "COMMENT (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1018) => "PATH (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1019) => "STICKY_NOTE (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1020) => "RECTANGLE (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1021) => "CIRCLE (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1022) => "ARROW (PINNED)",
                    _ if *event_type == xaeroflux_core::event::make_pinned(1023) => "TEXT (PINNED)",
                    _ => "OTHER",
                };
                println!("  Event type {}: {} events ({})", event_type, count, name);
            }
        }
    }

    // Check what make_pinned actually returns
    println!("\n=== PINNED EVENT TYPE VALUES ===");
    println!("Regular CIRCLE: {}", EVENT_TYPE_CIRCLE);
    println!("Pinned CIRCLE: {}", xaeroflux_core::event::make_pinned(EVENT_TYPE_CIRCLE));
    println!("Regular RECTANGLE: {}", EVENT_TYPE_RECTANGLE);
    println!("Pinned RECTANGLE: {}", xaeroflux_core::event::make_pinned(EVENT_TYPE_RECTANGLE));
    println!("Regular ARROW: {}", EVENT_TYPE_ARROW);
    println!("Pinned ARROW: {}", xaeroflux_core::event::make_pinned(EVENT_TYPE_ARROW));
    println!("Regular TEXT: {}", EVENT_TYPE_TEXT);
    println!("Pinned TEXT: {}", xaeroflux_core::event::make_pinned(EVENT_TYPE_TEXT));
    println!("Regular PATH: {}", EVENT_TYPE_PATH);
    println!("Pinned PATH: {}", xaeroflux_core::event::make_pinned(EVENT_TYPE_PATH));
    println!("Regular STICKY: {}", EVENT_TYPE_STICKY_NOTE);
    println!("Pinned STICKY: {}", xaeroflux_core::event::make_pinned(EVENT_TYPE_STICKY_NOTE));

    Ok(())
}

// ================================================================================================
// HIERARCHICAL EVENT HELPERS
// ================================================================================================

pub fn build_hierarchical_event(id: [u8; 32], ancestry: Vec<[u8; 32]>, data: &[u8]) -> Vec<u8> {
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
            ptr.add(offset)
                .copy_from_nonoverlapping(ancestor.as_ptr(), 32);
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
        ptr.add(offset)
            .copy_from_nonoverlapping(data.as_ptr(), data.len());

        Vec::from_raw_parts(ptr, total_size, total_size)
    }
}

pub fn parse_hierarchical_event(event_data: &[u8]) -> (Vec<[u8; 32]>, &[u8]) {
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

fn query_both_event_types<const SIZE: usize>(
    xf: &XaeroFlux,
    event_type: u32,
    board_id: [u8; 32],
    min_data_size: usize,
) -> Result<Vec<XaeroInternalEvent<SIZE>>, Box<dyn std::error::Error>> {
    use xaeroflux_actors::read_api::RangeQuery;
    let mut all_events = Vec::new();
    let query2 = RangeQuery::<SIZE> {
        xaero_id: [0u8; 32],
        event_type: xaeroflux_core::event::make_pinned(event_type),
    };

    let board_id_copy2 = board_id; // Clone for second closure
    if let Ok(events) = xf.range_query_with_filter::<SIZE, _>(
        query2,
        Box::new(move |event: &XaeroInternalEvent<SIZE>| {
            let (ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            ancestry.contains(&board_id_copy2) && data.len() >= min_data_size
        }),
    ) {
        tracing::info!("Found {} events with pinned type {}", events.len(), xaeroflux_core::event::make_pinned(event_type));
        all_events.extend(events);
    }
    Ok(all_events)
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
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_GROUP))?;

    tracing::info!(
        "Created group {} with id {:?}",
        name,
        hex::encode(&group_id)
    );
    Ok(group_id)
}

pub fn update_group(
    group_id: [u8; 32],
    name: Option<&str>,
    icon: Option<&str>,
    color: Option<[u8; 4]>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut group = get_group_by_id(group_id)?;

    if let Some(name) = name {
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(64);
        group.name = [0u8; 64];
        group.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
    }

    if let Some(icon) = icon {
        let icon_bytes = icon.as_bytes();
        let icon_len = icon_bytes.len().min(32);
        group.icon = [0u8; 32];
        group.icon[..icon_len].copy_from_slice(&icon_bytes[..icon_len]);
    }

    if let Some(color) = color {
        group.color = color;
    }

    group.updated_at = xaeroflux_core::date_time::emit_secs();

    let event = build_hierarchical_event(group_id, vec![], bytemuck::bytes_of(&group));
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_GROUP)?;

    Ok(())
}

pub fn tombstone_group(group_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        group_id,
        vec![],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_GROUP)?;
    Ok(())
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

    let event =
        build_hierarchical_event(workspace_id, vec![group_id], bytemuck::bytes_of(&workspace));

    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_WORKSPACE))?;

    tracing::info!(
        "Created workspace {} with id {:?}",
        name,
        hex::encode(&workspace_id)
    );
    Ok(workspace_id)
}

pub fn update_workspace(
    workspace_id: [u8; 32],
    name: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut workspace = get_workspace_by_id(workspace_id)?;

    if let Some(name) = name {
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(64);
        workspace.name = [0u8; 64];
        workspace.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
    }

    workspace.updated_at = xaeroflux_core::date_time::emit_secs();

    let event = build_hierarchical_event(
        workspace_id,
        vec![workspace.group_id],
        bytemuck::bytes_of(&workspace),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_WORKSPACE)?;

    Ok(())
}

pub fn tombstone_workspace(workspace_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = get_workspace_by_id(workspace_id)?;
    let event = build_hierarchical_event(
        workspace_id,
        vec![workspace.group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_WORKSPACE)?;
    Ok(())
}

pub fn get_workspace_by_id(
    workspace_id: [u8; 32],
) -> Result<WorkspaceStandard, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    match xf.find_current_state_by_eid::<512>(workspace_id)? {
        Some(event) => {
            let (_ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            let mut aligned_workspace = WorkspaceStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_workspace as *mut WorkspaceStandard as *mut u8,
                    std::mem::size_of::<WorkspaceStandard>(),
                );
            }
            Ok(aligned_workspace)
        }
        None => Err("Workspace not found".into()),
    }
}

pub fn list_workspaces_for_group(
    group_id: [u8; 32],
) -> Result<Vec<WorkspaceStandard>, Box<dyn std::error::Error>> {
    tracing::info!("Listing workspaces for group {:?}", hex::encode(&group_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<{ rusted_ring::M_TSHIRT_SIZE }> {
        xaero_id: [0u8; 32],
        event_type: EVENT_TYPE_WORKSPACE,
    };

    let events = xf.range_query_with_filter::<{ rusted_ring::M_TSHIRT_SIZE }, _>(
        query,
        Box::new(
            move |event: &XaeroInternalEvent<{ rusted_ring::M_TSHIRT_SIZE }>| {
                let (ancestry, data) =
                    parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
                ancestry.len() >= 2
                    && ancestry[1] == group_id
                    && data.len() >= std::mem::size_of::<WorkspaceStandard>()
            },
        ),
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

pub fn update_board(
    board_id: [u8; 32],
    name: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut board = get_board_by_id(board_id)?;

    if let Some(name) = name {
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(64);
        board.name = [0u8; 64];
        board.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
    }

    board.updated_at = xaeroflux_core::date_time::emit_secs();

    let event = build_hierarchical_event(
        board_id,
        vec![board.workspace_id, board.group_id],
        bytemuck::bytes_of(&board),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_BOARD)?;

    Ok(())
}

pub fn tombstone_board(board_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>> {
    let board = get_board_by_id(board_id)?;
    let event = build_hierarchical_event(
        board_id,
        vec![board.workspace_id, board.group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_BOARD)?;
    Ok(())
}

pub fn get_board_by_id(board_id: [u8; 32]) -> Result<BoardStandard, Box<dyn std::error::Error>> {
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    match xf.find_current_state_by_eid::<1024>(board_id)? {
        Some(event) => {
            let (_ancestry, data) =
                parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
            let mut aligned_board = BoardStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_board as *mut BoardStandard as *mut u8,
                    std::mem::size_of::<BoardStandard>(),
                );
            }
            Ok(aligned_board)
        }
        None => Err("Board not found".into()),
    }
}

pub fn list_boards_for_workspace(
    workspace_id: [u8; 32],
) -> Result<Vec<BoardStandard>, Box<dyn std::error::Error>> {
    tracing::info!("Looking for boards in workspace: {:x?}", &workspace_id[..8]);

    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let query = RangeQuery::<1024> {
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
            let mut aligned_board = BoardStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_board as *mut BoardStandard as *mut u8,
                    std::mem::size_of::<BoardStandard>(),
                );
            }

            let name = String::from_utf8_lossy(
                &aligned_board.name[..aligned_board
                    .name
                    .iter()
                    .position(|&x| x == 0)
                    .unwrap_or(64)],
            );
            tracing::info!("Parsed board: '{}'", name);
            boards.push(aligned_board);
        }
    }

    tracing::info!("Returning {} boards", boards.len());
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

    let event = build_hierarchical_event(comment_id, ancestry, bytemuck::bytes_of(&comment));
    tracing::info!("add_comment: Built hierarchical event of size : {:?}", event.len());
    XaeroFlux::write_event_static(
        &event,
        xaeroflux_core::event::make_pinned(EVENT_TYPE_COMMENT),
    )?;

    tracing::info!("Added comment with id {:?}", hex::encode(&comment_id));
    Ok(comment_id)
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
    tracing::info!("add_path_object : Built hierarchical event of size : {:?}", size_of_val
        (&event));
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_PATH))?;

    tracing::info!("Added path object to board {:?}", hex::encode(&board_id));
    Ok(path_id)
}

pub fn update_path_object(path_data: &PathData<256>) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        path_data.path_id,
        vec![
            path_data.board_id,
            path_data.workspace_id,
            path_data.group_id,
        ],
        bytemuck::bytes_of(path_data),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_PATH)?;
    Ok(())
}

pub fn tombstone_path_object(
    path_id: [u8; 32],
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        path_id,
        vec![board_id, workspace_id, group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_PATH)?;
    Ok(())
}

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
    tracing::info!("add_sticky_note  of size : {:?}", event.len());
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_STICKY_NOTE))?;

    Ok(note_id)
}

pub fn update_sticky_note(sticky_data: &StickyNoteData) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        sticky_data.note_id,
        vec![
            sticky_data.board_id,
            sticky_data.workspace_id,
            sticky_data.group_id,
        ],
        bytemuck::bytes_of(sticky_data),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_STICKY_NOTE)?;
    Ok(())
}

pub fn tombstone_sticky_note(
    note_id: [u8; 32],
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        note_id,
        vec![board_id, workspace_id, group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_STICKY_NOTE)?;
    Ok(())
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
    tracing::info!("add_rectangle  of size : {:?}", event.len());
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_RECTANGLE))?;
    Ok(shape_id)
}

pub fn update_rectangle(rect_data: &RectangleData) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        rect_data.shape_id,
        vec![
            rect_data.board_id,
            rect_data.workspace_id,
            rect_data.group_id,
        ],
        bytemuck::bytes_of(rect_data),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_RECTANGLE)?;
    Ok(())
}

pub fn tombstone_rectangle(
    shape_id: [u8; 32],
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        shape_id,
        vec![board_id, workspace_id, group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_RECTANGLE)?;
    Ok(())
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

    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_CIRCLE))?;

    tracing::info!("Added circle to board {:?}", hex::encode(&board_id));
    Ok(shape_id)
}

pub fn update_circle(circle_data: &CircleData) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        circle_data.shape_id,
        vec![
            circle_data.board_id,
            circle_data.workspace_id,
            circle_data.group_id,
        ],
        bytemuck::bytes_of(circle_data),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_CIRCLE)?;
    Ok(())
}

pub fn tombstone_circle(
    shape_id: [u8; 32],
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        shape_id,
        vec![board_id, workspace_id, group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_CIRCLE)?;
    Ok(())
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
    tracing::info!("add_arrow  of size : {:?}", event.len());
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_ARROW))?;

    tracing::info!("Added arrow to board {:?}", hex::encode(&board_id));
    Ok(arrow_id)
}

pub fn update_arrow(arrow_data: &ArrowData) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        arrow_data.arrow_id,
        vec![
            arrow_data.board_id,
            arrow_data.workspace_id,
            arrow_data.group_id,
        ],
        bytemuck::bytes_of(arrow_data),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_ARROW)?;
    Ok(())
}

pub fn tombstone_arrow(
    arrow_id: [u8; 32],
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        arrow_id,
        vec![board_id, workspace_id, group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_ARROW)?;
    Ok(())
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
    tracing::info!("add_text  of size : {:?}", event.len());
    XaeroFlux::write_event_static(&event, xaeroflux_core::event::make_pinned(EVENT_TYPE_TEXT))?;

    tracing::info!("Added text to board {:?}", hex::encode(&board_id));
    Ok(text_id)
}

pub fn update_text(text_data: &TextData) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        text_data.text_id,
        vec![
            text_data.board_id,
            text_data.workspace_id,
            text_data.group_id,
        ],
        bytemuck::bytes_of(text_data),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_UPDATE_TEXT)?;
    Ok(())
}

pub fn tombstone_text(
    text_id: [u8; 32],
    board_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
) -> Result<(), Box<dyn std::error::Error>> {
    let event = build_hierarchical_event(
        text_id,
        vec![board_id, workspace_id, group_id],
        &xaeroflux_core::date_time::emit_secs().to_le_bytes(),
    );
    XaeroFlux::write_event_static(&event, EVENT_TYPE_TOMBSTONE_TEXT)?;
    Ok(())
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

// ================================================================================================
// WHITEBOARD OBJECT OPERATIONS (Generic)
// ================================================================================================

pub trait HierarchicalEvent {
    fn get_id(&self) -> [u8; 32];
}

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
    // TODO: Implement actual user-workspace mapping
    Ok(())
}

// ================================================================================================
// FIXED PATH LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_paths_for_board(
    board_id: [u8; 32],
) -> Result<Vec<PathData<256>>, Box<dyn std::error::Error>> {
    tracing::info!("Loading paths for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    // PathData<256> is 2560 bytes, use L size (4096)
    let events = query_both_event_types::<{ rusted_ring::L_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_PATH,
        board_id,
        2560,
    )?;

    let mut paths = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= 2560 {
            let mut aligned_path = PathData::<256>::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_path as *mut PathData<256> as *mut u8,
                    2560,
                );
            }
            paths.push(aligned_path);
        }
    }

    tracing::info!("Found {} paths for board", paths.len());
    Ok(paths)
}

// ================================================================================================
// FIXED STICKY NOTE LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_sticky_notes_for_board(
    board_id: [u8; 32],
) -> Result<Vec<StickyNoteData>, Box<dyn std::error::Error>> {
    tracing::info!("Loading sticky notes for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    // StickyNoteData is 512 bytes, use M size (1024)
    let events = query_both_event_types::<{ rusted_ring::M_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_STICKY_NOTE,
        board_id,
        std::mem::size_of::<StickyNoteData>(),
    )?;

    let mut notes = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<StickyNoteData>() {
            let mut aligned_note = StickyNoteData::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_note as *mut StickyNoteData as *mut u8,
                    std::mem::size_of::<StickyNoteData>(),
                );
            }
            notes.push(aligned_note);
        }
    }

    tracing::info!("Found {} sticky notes for board", notes.len());
    Ok(notes)
}

// ================================================================================================
// FIXED RECTANGLE LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_rectangles_for_board(
    board_id: [u8; 32],
) -> Result<Vec<RectangleData>, Box<dyn std::error::Error>> {
    tracing::info!("Loading rectangles for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    let events = query_both_event_types::<{ rusted_ring::M_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_RECTANGLE,
        board_id,
        std::mem::size_of::<RectangleData>(),
    )?;

    let mut rectangles = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<RectangleData>() {
            let mut aligned_rect = RectangleData::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_rect as *mut RectangleData as *mut u8,
                    std::mem::size_of::<RectangleData>(),
                );
            }
            rectangles.push(aligned_rect);
        }
    }

    tracing::info!("Found {} rectangles for board", rectangles.len());
    Ok(rectangles)
}

// ================================================================================================
// CIRCLE LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_circles_for_board(
    board_id: [u8; 32],
) -> Result<Vec<CircleData>, Box<dyn std::error::Error>> {
    tracing::info!("Loading circles for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    let events = query_both_event_types::<{ rusted_ring::M_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_CIRCLE,
        board_id,
        std::mem::size_of::<CircleData>(),
    )?;

    let mut circles = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<CircleData>() {
            let mut aligned_circle = CircleData::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_circle as *mut CircleData as *mut u8,
                    std::mem::size_of::<CircleData>(),
                );
            }
            circles.push(aligned_circle);
        }
    }

    tracing::info!("Found {} circles for board", circles.len());
    Ok(circles)
}

// ================================================================================================
// FIXED ARROW LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_arrows_for_board(
    board_id: [u8; 32],
) -> Result<Vec<ArrowData>, Box<dyn std::error::Error>> {
    tracing::info!("Loading arrows for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    let events = query_both_event_types::<{ rusted_ring::M_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_ARROW,
        board_id,
        std::mem::size_of::<ArrowData>(),
    )?;

    let mut arrows = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<ArrowData>() {
            let mut aligned_arrow = ArrowData::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_arrow as *mut ArrowData as *mut u8,
                    std::mem::size_of::<ArrowData>(),
                );
            }
            arrows.push(aligned_arrow);
        }
    }

    tracing::info!("Found {} arrows for board", arrows.len());
    Ok(arrows)
}

// ================================================================================================
// FIXED TEXT LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_text_objects_for_board(
    board_id: [u8; 32],
) -> Result<Vec<TextData>, Box<dyn std::error::Error>> {
    tracing::info!("Loading text objects for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    let events = query_both_event_types::<{ rusted_ring::M_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_TEXT,
        board_id,
        std::mem::size_of::<TextData>(),
    )?;

    let mut text_objects = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<TextData>() {
            let mut aligned_text = TextData::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_text as *mut TextData as *mut u8,
                    std::mem::size_of::<TextData>(),
                );
            }
            text_objects.push(aligned_text);
        }
    }

    tracing::info!("Found {} text objects for board", text_objects.len());
    Ok(text_objects)
}

// ================================================================================================
// FIXED COMMENT LOADING - Query both pinned and unpinned
// ================================================================================================

pub fn list_comments_for_board(
    board_id: [u8; 32],
) -> Result<Vec<CommentStandard>, Box<dyn std::error::Error>> {
    tracing::info!("Loading comments for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;

    let events = query_both_event_types::<{ rusted_ring::M_TSHIRT_SIZE }>(
        xf,
        EVENT_TYPE_COMMENT,
        board_id,
        std::mem::size_of::<CommentStandard>(),
    )?;
    tracing::info!("Found {} comments for board", events.len());
    let mut comments = Vec::new();
    for event in events {
        let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
        if data.len() >= std::mem::size_of::<CommentStandard>() {
            let mut aligned_comment = CommentStandard::zeroed();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    &mut aligned_comment as *mut CommentStandard as *mut u8,
                    std::mem::size_of::<CommentStandard>(),
                );
            }
            comments.push(aligned_comment);
        }
    }

    tracing::info!("Found final {} comments for board", comments.len());
    Ok(comments)
}

// ================================================================================================
// AGGREGATED BOARD LOADING
// ================================================================================================

// Add this function to storage.rs to recover misplaced objects

pub fn recover_misplaced_objects(board_id: [u8; 32]) -> Result<BoardObjects, Box<dyn std::error::Error>> {
    tracing::info!("Recovering misplaced objects for board {:?}", hex::encode(&board_id));
    let xf = XaeroFlux::instance(DATA_DIR).ok_or("XaeroFlux not initialized")?;
    use xaeroflux_actors::read_api::RangeQuery;

    let mut paths = Vec::new();
    let mut sticky_notes = Vec::new();
    let mut rectangles = Vec::new();
    let mut circles = Vec::new();
    let mut arrows = Vec::new();
    let mut text_objects = Vec::new();

    // These are the suspicious event types with 456 events each
    let suspicious_types = vec![253, 509, 765, 1277, 1533, 1789, 2045, 2301, 2557, 2813, 3069, 3325, 3581, 3837];

    // Check each suspicious type in M database (where they're stored)
    for event_type in suspicious_types {
        let query = RangeQuery::<{ rusted_ring::M_TSHIRT_SIZE }> {
            xaero_id: [0u8; 32],
            event_type,
        };

        if let Ok(events) = xf.range_query_with_filter::<{ rusted_ring::M_TSHIRT_SIZE }, _>(
            query,
            Box::new(move |event: &XaeroInternalEvent<{ rusted_ring::M_TSHIRT_SIZE }>| {
                let (ancestry, _data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);
                ancestry.contains(&board_id)
            }),
        ) {
            for event in events {
                let (_ancestry, data) = parse_hierarchical_event(&event.evt.data[..event.evt.len as usize]);

                // Try to determine what type of object this is by its size
                match data.len() {
                    256 => {
                        // Could be RectangleData, CircleData, or ArrowData
                        // Try to peek at the data to determine which
                        // For now, let's assume these are rectangles
                        if data.len() >= std::mem::size_of::<RectangleData>() {
                            let mut aligned_rect = RectangleData::zeroed();
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    data.as_ptr(),
                                    &mut aligned_rect as *mut RectangleData as *mut u8,
                                    std::mem::size_of::<RectangleData>(),
                                );
                            }

                            // Check if this looks like valid rectangle data
                            if aligned_rect.board_id == board_id {
                                rectangles.push(aligned_rect);
                                tracing::info!("Recovered rectangle from event type {}", event_type);
                            }
                        }
                    },
                    512 => {
                        // Could be StickyNoteData or TextData
                        // Try sticky note first
                        if data.len() >= std::mem::size_of::<StickyNoteData>() {
                            let mut aligned_sticky = StickyNoteData::zeroed();
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    data.as_ptr(),
                                    &mut aligned_sticky as *mut StickyNoteData as *mut u8,
                                    std::mem::size_of::<StickyNoteData>(),
                                );
                            }

                            if aligned_sticky.board_id == board_id {
                                sticky_notes.push(aligned_sticky);
                                tracing::info!("Recovered sticky note from event type {}", event_type);
                            }
                        }
                    },
                    _ => {
                        tracing::warn!("Unknown object size {} at event type {}", data.len(), event_type);
                    }
                }
            }
        }
    }

    // Also get the correctly stored circles
    circles = list_circles_for_board(board_id)?;

    // And the 4 paths in L database
    paths = list_paths_for_board(board_id)?;

    tracing::info!(
        "Recovery complete - Paths: {}, Sticky: {}, Rects: {}, Circles: {}, Arrows: {}, Text: {}",
        paths.len(),
        sticky_notes.len(),
        rectangles.len(),
        circles.len(),
        arrows.len(),
        text_objects.len()
    );

    Ok(BoardObjects {
        paths,
        sticky_notes,
        rectangles,
        circles,
        arrows,
        text_objects,
    })
}

// Update load_all_board_objects to use recovery
pub fn load_all_board_objects(
    board_id: [u8; 32],
) -> Result<BoardObjects, Box<dyn std::error::Error>> {
    tracing::info!("Loading all objects for board {:?}", hex::encode(&board_id));

    // First try normal loading
    let mut objects = BoardObjects {
        paths: list_paths_for_board(board_id)?,
        sticky_notes: list_sticky_notes_for_board(board_id)?,
        rectangles: list_rectangles_for_board(board_id)?,
        circles: list_circles_for_board(board_id)?,
        arrows: list_arrows_for_board(board_id)?,
        text_objects: list_text_objects_for_board(board_id)?,
    };

    // If we're missing objects (only circles loaded), try recovery
    // if objects.rectangles.is_empty() && objects.sticky_notes.is_empty() && objects.arrows.is_empty() {
    //     tracing::warn!("Normal loading failed, attempting recovery of misplaced objects");
    //     objects = recover_misplaced_objects(board_id)?;
    // }

    tracing::info!(
        "Board objects summary - Paths: {}, Sticky: {}, Rects: {}, Circles: {}, Arrows: {}, Text: {}",
        objects.paths.len(),
        objects.sticky_notes.len(),
        objects.rectangles.len(),
        objects.circles.len(),
        objects.arrows.len(),
        objects.text_objects.len()
    );

    Ok(objects)
}

pub fn create_test_data_structure(creator_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;
    let start = Instant::now();

    tracing::info!("Creating test data structure: 10 groups x 10 workspaces x 1000 boards");

    let group_colors = vec![
        [255, 99, 71, 255],   // Tomato
        [135, 206, 235, 255], // Sky Blue
        [144, 238, 144, 255], // Light Green
        [255, 182, 193, 255], // Light Pink
        [255, 218, 185, 255], // Peach
        [221, 160, 221, 255], // Plum
        [176, 224, 230, 255], // Powder Blue
        [255, 255, 224, 255], // Light Yellow
        [240, 230, 140, 255], // Khaki
        [216, 191, 216, 255], // Thistle
    ];

    let group_icons = vec![
        "folder", "briefcase", "home", "star", "heart",
        "flag", "bookmark", "globe", "camera", "music"
    ];

    for group_idx in 0..10 {
        let group_name = format!("Group {}", group_idx + 1);
        let group_icon = group_icons[group_idx % group_icons.len()];
        let group_color = group_colors[group_idx % group_colors.len()];

        // Create group
        let group_id = create_group(
            creator_id,
            &group_name,
            group_icon,
            group_color,
        )?;

        tracing::info!("Created group {}/{}: {}", group_idx + 1, 10, group_name);

        for workspace_idx in 0..10 {
            let workspace_name = format!("Workspace {}-{}", group_idx + 1, workspace_idx + 1);

            // Create workspace
            let workspace_id = create_workspace(
                creator_id,
                group_id,
                &workspace_name,
            )?;

            tracing::info!(
                "  Created workspace {}/{}: {}",
                workspace_idx + 1,
                10,
                workspace_name
            );

            // Create boards in batches to avoid overwhelming the system
            const BATCH_SIZE: usize = 100;
            for batch_start in (0..1000).step_by(BATCH_SIZE) {
                let batch_end = (batch_start + BATCH_SIZE).min(1000);

                for board_idx in batch_start..batch_end {
                    let board_name = format!(
                        "Board {}-{}-{}",
                        group_idx + 1,
                        workspace_idx + 1,
                        board_idx + 1
                    );

                    create_board(
                        creator_id,
                        workspace_id,
                        group_id,
                        &board_name,
                    )?;
                }

                if batch_end % 100 == 0 {
                    tracing::info!(
                        "    Created boards {}/{} for workspace {}",
                        batch_end,
                        1000,
                        workspace_name
                    );
                }
            }
        }
    }

    let elapsed = start.elapsed();
    tracing::info!(
        "Test data creation complete in {:.2?}: 10 groups, 100 workspaces, 100,000 boards",
        elapsed
    );

    Ok(())
}

pub fn create_small_test_data(creator_id: [u8; 32]) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Creating small test data: 3 groups x 3 workspaces x 10 boards");

    let colors = vec![
        [255, 99, 71, 255],   // Tomato
        [135, 206, 235, 255], // Sky Blue
        [144, 238, 144, 255], // Light Green
    ];

    for group_idx in 0..3 {
        let group_id = create_group(
            creator_id,
            &format!("Test Group {}", group_idx + 1),
            "folder",
            colors[group_idx],
        )?;

        for workspace_idx in 0..3 {
            let workspace_id = create_workspace(
                creator_id,
                group_id,
                &format!("Workspace {}-{}", group_idx + 1, workspace_idx + 1),
            )?;

            for board_idx in 0..10 {
                create_board(
                    creator_id,
                    workspace_id,
                    group_id,
                    &format!("Board {}-{}-{}",
                             group_idx + 1,
                             workspace_idx + 1,
                             board_idx + 1
                    ),
                )?;
            }
        }
    }

    tracing::info!("Small test data creation complete: 3 groups, 9 workspaces, 90 boards");
    Ok(())
}

