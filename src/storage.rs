use std::sync::{Arc, Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};
use xaeroflux_actors::{
    aof::storage::lmdb::{get_children_entitities_by_entity_id, get_current_state_by_entity_id},
    XaeroFlux,
};
use xaeroflux_core::date_time::emit_secs;

use crate::{objects::*, Group, Workspace, Board, Comment, Layer, DrawingPath, FileNode};

pub static DATA_DIR: &str = "cyan";

// Event type constants for writing
pub const EVENT_TYPE_GROUP: u32 = 100;
pub const EVENT_TYPE_WORKSPACE: u32 = 101;
pub const EVENT_TYPE_BOARD: u32 = 102;
pub const EVENT_TYPE_WHITEBOARD_OBJECT: u32 = 103;
pub const EVENT_TYPE_COMMENT: u32 = 104;
pub const EVENT_TYPE_LAYER: u32 = 105;
pub const EVENT_TYPE_VOTE: u32 = 106;

// Object type identifiers (store as first byte in event data)
pub const OBJECT_TYPE_WHITEBOARD: u8 = 0;
pub const OBJECT_TYPE_PATH: u8 = 1;
pub const OBJECT_TYPE_STICKY: u8 = 2;
pub const OBJECT_TYPE_RECTANGLE: u8 = 3;
pub const OBJECT_TYPE_CIRCLE: u8 = 4;
pub const OBJECT_TYPE_ARROW: u8 = 5;
pub const OBJECT_TYPE_TEXT: u8 = 6;
pub const OBJECT_TYPE_COMMENT: u8 = 7;
pub const OBJECT_TYPE_FILE: u8 = 8;

// Container structures
#[derive(Debug, Default)]
pub struct WhiteboardObjects {
    pub paths: Vec<PathData<512>>,
    pub sticky_notes: Vec<StickyNoteData>,
    pub rectangles: Vec<RectangleData>,
    pub circles: Vec<CircleData>,
    pub arrows: Vec<ArrowData>,
    pub text_boxes: Vec<TextBoxData>,
    pub comments: Vec<CommentData>,
    pub files: Vec<FileAttachmentData>,
}

#[derive(Debug)]
pub struct PopulatedWhiteboard {
    pub metadata: WhiteboardData,
    pub objects: WhiteboardObjects,
}

#[derive(Debug)]
pub struct PopulatedWorkspace {
    pub workspace_id: [u8; 32],
    pub whiteboards: Vec<PopulatedWhiteboard>,
}

// ================================================================================================
// READ OPERATIONS
// ================================================================================================


pub fn get_group_by_group_id<const SIZE: usize>(
    group_id: [u8; 32],
) -> Result<Group<SIZE>, Box<dyn std::error::Error>> where [(); { std::mem::size_of::<Group<SIZE>>() }]: {
    let rh = XaeroFlux::read_handle().expect("read handle not ready yet!");
    match get_current_state_by_entity_id::<{ std::mem::size_of::<Group<SIZE>>() }>(&rh, group_id)? {
        Some(current_state) => {
            Ok(*bytemuck::from_bytes::<Group<SIZE>>(
                &current_state.evt.data[..std::mem::size_of::<Group<SIZE>>()],
            ))
        }
        None => Err("Group not found".into()),
    }
}

pub fn get_workspaces_for_group<const SIZE: usize, const MAX_WORKSPACES: usize>(
    group_id: [u8; 32],
) -> Result<([Workspace<SIZE>; MAX_WORKSPACES], usize), Box<dyn std::error::Error>> where [(); { std::mem::size_of::<Workspace<SIZE>>() }]: {
    let mut workspaces = [Workspace::<SIZE>::zeroed(); MAX_WORKSPACES];
    let mut count = 0;

    let rh = XaeroFlux::read_handle().expect("read handle not ready yet!");
    let workspace_ids = get_children_entitities_by_entity_id(&rh, group_id)?;

    for chunk in workspace_ids.chunks_exact(32) {
        if count >= MAX_WORKSPACES {
            tracing::warn!(
                "Group has more than {} workspaces, truncating",
                MAX_WORKSPACES
            );
            break;
        }

        let mut buffer = [0u8; 32];
        buffer.copy_from_slice(chunk);

        match get_current_state_by_entity_id::<{ std::mem::size_of::<Workspace<SIZE>>() }>(&rh, buffer)? {
            Some(current_state_workspace) => {
                let w = bytemuck::from_bytes::<Workspace<SIZE>>(&current_state_workspace.evt.data);
                workspaces[count] = *w;
                count += 1;
            }
            None => {
                tracing::warn!("no workspace found yet for {:?}", hex::encode(buffer));
            }
        }
    }
    Ok((workspaces, count))
}

pub fn get_workspace_with_all_objects(
    workspace_id: [u8; 32],
) -> Result<PopulatedWorkspace, Box<dyn std::error::Error>> {
    let env = XaeroFlux::read_handle().expect("read_api not ready!");

    // Get all whiteboard IDs for this workspace
    let whiteboard_ids_raw = get_children_entitities_by_entity_id(&env, workspace_id)?;

    let mut populated_workspace = PopulatedWorkspace {
        workspace_id,
        whiteboards: Vec::new(),
    };

    // Process each whiteboard
    for board_chunk in whiteboard_ids_raw.chunks_exact(32) {
        let mut board_id = [0u8; 32];
        board_id.copy_from_slice(board_chunk);

        // Get whiteboard metadata
        let whiteboard_metadata = get_whiteboard_metadata(&env, board_id)?;

        // Get all objects for this whiteboard
        let objects = get_all_whiteboard_objects(board_id)?;

        populated_workspace.whiteboards.push(PopulatedWhiteboard {
            metadata: whiteboard_metadata,
            objects,
        });
    }

    Ok(populated_workspace)
}

// ================================================================================================
// WRITE OPERATIONS
// ================================================================================================

pub fn create_group<const MAX_WORKSPACES: usize>(
    creator_id: [u8; 32],
    name: &str,
    icon: &str,
    color: [u8; 4],
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let group_id = blake3::hash(&[
        creator_id.as_slice(),
        name.as_bytes(),
        &emit_secs().to_le_bytes(),
    ].concat()).into();

    let mut group = Group::<MAX_WORKSPACES>::zeroed();
    group.group_id = group_id;
    group.parent_id = [0; 32];
    group.created_by = creator_id;
    group.created_at = emit_secs();
    group.updated_at = group.created_at;
    group.color = color;

    // Copy name and icon
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    group.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    let icon_bytes = icon.as_bytes();
    let copy_len = icon_bytes.len().min(32);
    group.icon[..copy_len].copy_from_slice(&icon_bytes[..copy_len]);

    // Write to XaeroFlux
    let data = bytemuck::bytes_of(&group);
    XaeroFlux::write_event_static(data, EVENT_TYPE_GROUP)?;

    Ok(group_id)
}

pub fn create_workspace<const MAX_BOARDS: usize>(
    creator_id: [u8; 32],
    group_id: [u8; 32],
    name: &str,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let workspace_id = blake3::hash(&[
        group_id.as_slice(),
        name.as_bytes(),
        &emit_secs().to_le_bytes(),
    ].concat()).into();

    let mut workspace = Workspace::<MAX_BOARDS>::zeroed();
    workspace.workspace_id = workspace_id;
    workspace.group_id = group_id;
    workspace.created_by = creator_id;
    workspace.created_at = emit_secs();
    workspace.updated_at = workspace.created_at;

    // Copy name
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    workspace.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    // Write to XaeroFlux
    let data = bytemuck::bytes_of(&workspace);
    XaeroFlux::write_event_static(data, EVENT_TYPE_WORKSPACE)?;

    Ok(workspace_id)
}

pub fn create_board<const MAX_FILES: usize>(
    creator_id: [u8; 32],
    workspace_id: [u8; 32],
    group_id: [u8; 32],
    name: &str,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let board_id = blake3::hash(&[
        workspace_id.as_slice(),
        name.as_bytes(),
        &emit_secs().to_le_bytes(),
    ].concat()).into();

    let mut board = Board::<MAX_FILES>::zeroed();
    board.board_id = board_id;
    board.workspace_id = workspace_id;
    board.group_id = group_id;
    board.created_by = creator_id;
    board.created_at = emit_secs();
    board.updated_at = board.created_at;
    board.last_modified_by = creator_id;

    // Copy name
    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    board.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    // Write to XaeroFlux
    let data = bytemuck::bytes_of(&board);
    XaeroFlux::write_event_static(data, EVENT_TYPE_BOARD)?;

    // Also create default WhiteboardData
    let mut whiteboard_data = WhiteboardData::zeroed();
    whiteboard_data.board_id = board_id;
    whiteboard_data.created_at = emit_secs();
    whiteboard_data.updated_at = whiteboard_data.created_at;
    whiteboard_data.created_by = creator_id;
    whiteboard_data.canvas_width = 1920.0;
    whiteboard_data.canvas_height = 1080.0;
    whiteboard_data.background_color = [255, 255, 255, 255];
    whiteboard_data.zoom_level = 1.0;

    // Write whiteboard metadata with type byte
    let mut wb_data = Vec::with_capacity(std::mem::size_of::<WhiteboardData>() + 1);
    wb_data.push(OBJECT_TYPE_WHITEBOARD);
    wb_data.extend_from_slice(bytemuck::bytes_of(&whiteboard_data));
    XaeroFlux::write_event_static(&wb_data, EVENT_TYPE_WHITEBOARD_OBJECT)?;

    Ok(board_id)
}

pub fn add_whiteboard_object<T: Pod>(
    board_id: [u8; 32],
    object_type: u8,
    object: &T,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    // Generate object ID
    let object_id = blake3::hash(&[
        board_id.as_slice(),
        &[object_type],
        &emit_secs().to_le_bytes(),
    ].concat()).into();

    // Prepare event with type byte + object data
    let mut event_data = Vec::with_capacity(std::mem::size_of::<T>() + 1);
    event_data.push(object_type);
    event_data.extend_from_slice(bytemuck::bytes_of(object));

    // Write to XaeroFlux
    XaeroFlux::write_event_static(&event_data, EVENT_TYPE_WHITEBOARD_OBJECT)?;

    Ok(object_id)
}

pub fn add_comment<const MAX_TEXT: usize>(
    board_id: [u8; 32],
    author_id: [u8; 32],
    author_name: &str,
    content: &str,
    parent_id: Option<[u8; 32]>,
) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let comment_id = blake3::hash(&[
        board_id.as_slice(),
        author_id.as_slice(),
        content.as_bytes(),
        &emit_secs().to_le_bytes(),
    ].concat()).into();

    let mut comment = Comment::<MAX_TEXT>::zeroed();
    comment.comment_id = comment_id;
    comment.board_id = board_id;
    comment.parent_id = parent_id.unwrap_or([0; 32]);
    comment.author_id = author_id;
    comment.created_at = emit_secs();
    comment.updated_at = comment.created_at;
    comment.depth = if parent_id.is_some() { 1 } else { 0 };

    // Copy author name
    let name_bytes = author_name.as_bytes();
    let copy_len = name_bytes.len().min(64);
    comment.author_name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    // Copy content
    let content_bytes = content.as_bytes();
    let copy_len = content_bytes.len().min(MAX_TEXT);
    comment.content[..copy_len].copy_from_slice(&content_bytes[..copy_len]);

    // Write to XaeroFlux
    let data = bytemuck::bytes_of(&comment);
    XaeroFlux::write_event_static(data, EVENT_TYPE_COMMENT)?;

    Ok(comment_id)
}

// ================================================================================================
// HELPER FUNCTIONS (keep existing implementations)
// ================================================================================================

fn get_whiteboard_metadata(
    env: &Arc<Mutex<xaeroflux_actors::aof::storage::lmdb::LmdbEnv>>,
    board_id: [u8; 32],
) -> Result<WhiteboardData, Box<dyn std::error::Error>> {
    // Try to get whiteboard data (assuming it's stored as M size)
    if let Some(event) = get_current_state_by_entity_id::<1024>(env, board_id)? {
        let data = &event.evt.data[..event.evt.len as usize];

        // Skip type byte if present
        let start_offset = if data[0] == OBJECT_TYPE_WHITEBOARD {
            1
        } else {
            0
        };

        let whiteboard: &WhiteboardData = bytemuck::from_bytes(&data[start_offset..]);
        Ok(*whiteboard)
    } else {
        // Return default if not found
        Ok(WhiteboardData {
            board_id,
            title: [0; 128],
            canvas_width: 1920.0,
            canvas_height: 1080.0,
            background_color: [255, 255, 255, 255],
            zoom_level: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            grid_enabled: false,
            grid_size: 20.0,
            current_layer: 0,
            total_layers: 1,
            total_objects: 0,
            created_at: 0,
            updated_at: 0,
            created_by: [0; 32],
            _padding: [0; 39],
        })
    }
}

fn get_all_whiteboard_objects(
    board_id: [u8; 32],
) -> Result<WhiteboardObjects, Box<dyn std::error::Error>> {
    let mut objects = WhiteboardObjects::default();

    // Get all object IDs for this whiteboard
    let object_ids_raw = get_children_entitities_by_entity_id(
        &XaeroFlux::read_handle().expect("read_api not ready!"),
        board_id,
    )?;

    // Process each object
    for obj_chunk in object_ids_raw.chunks_exact(32) {
        let mut obj_id = [0u8; 32];
        obj_id.copy_from_slice(obj_chunk);

        // Try to retrieve and parse the object
        parse_and_add_object(obj_id, &mut objects)?;
    }

    Ok(objects)
}

fn parse_and_add_object(
    obj_id: [u8; 32],
    objects: &mut WhiteboardObjects,
) -> Result<(), Box<dyn std::error::Error>> {
    // Try different sizes, starting with most common

    // Try L size (4096) for paths with many points
    if let Some(event) = get_current_state_by_entity_id::<4096>(
        &XaeroFlux::read_handle().expect("read_api not ready!"),
        obj_id,
    )? {
        if process_event_data(&event.evt.data[..event.evt.len as usize], objects) {
            return Ok(());
        }
    }

    // Try M size (1024) for medium objects
    if let Some(event) = get_current_state_by_entity_id::<1024>(
        &XaeroFlux::read_handle().expect("read_api not ready!"),
        obj_id,
    )? {
        if process_event_data(&event.evt.data[..event.evt.len as usize], objects) {
            return Ok(());
        }
    }

    // Try S size (256) for small objects
    if let Some(event) = get_current_state_by_entity_id::<256>(
        &XaeroFlux::read_handle().expect("read_api not ready!"),
        obj_id,
    )? {
        if process_event_data(&event.evt.data[..event.evt.len as usize], objects) {
            return Ok(());
        }
    }

    Ok(())
}

fn process_event_data(data: &[u8], objects: &mut WhiteboardObjects) -> bool {
    if data.is_empty() {
        return false;
    }

    let object_type = data[0];
    let object_data = &data[1..];

    match object_type {
        OBJECT_TYPE_PATH => {
            if let Ok(path) = bytemuck::try_from_bytes::<PathData<512>>(object_data) {
                objects.paths.push(*path);
                return true;
            }
        }
        OBJECT_TYPE_STICKY => {
            if let Ok(sticky) = bytemuck::try_from_bytes::<StickyNoteData>(object_data) {
                objects.sticky_notes.push(*sticky);
                return true;
            }
        }
        OBJECT_TYPE_RECTANGLE => {
            if let Ok(rect) = bytemuck::try_from_bytes::<RectangleData>(object_data) {
                objects.rectangles.push(*rect);
                return true;
            }
        }
        OBJECT_TYPE_CIRCLE => {
            if let Ok(circle) = bytemuck::try_from_bytes::<CircleData>(object_data) {
                objects.circles.push(*circle);
                return true;
            }
        }
        OBJECT_TYPE_ARROW => {
            if let Ok(arrow) = bytemuck::try_from_bytes::<ArrowData>(object_data) {
                objects.arrows.push(*arrow);
                return true;
            }
        }
        OBJECT_TYPE_TEXT => {
            if let Ok(text) = bytemuck::try_from_bytes::<TextBoxData>(object_data) {
                objects.text_boxes.push(*text);
                return true;
            }
        }
        OBJECT_TYPE_COMMENT => {
            if let Ok(comment) = bytemuck::try_from_bytes::<CommentData>(object_data) {
                objects.comments.push(*comment);
                return true;
            }
        }
        OBJECT_TYPE_FILE => {
            if let Ok(file) = bytemuck::try_from_bytes::<FileAttachmentData>(object_data) {
                objects.files.push(*file);
                return true;
            }
        }
        _ => {}
    }

    false
}

pub fn get_multiple_workspaces_with_objects(
    workspace_ids: Vec<[u8; 32]>,
) -> Result<Vec<PopulatedWorkspace>, Box<dyn std::error::Error>> {
    let mut workspaces = Vec::new();

    for workspace_id in workspace_ids {
        match get_workspace_with_all_objects(workspace_id) {
            Ok(workspace) => workspaces.push(workspace),
            Err(e) => {
                eprintln!(
                    "Failed to load workspace {:?}: {}",
                    hex::encode(workspace_id),
                    e
                );
                continue;
            }
        }
    }

    Ok(workspaces)
}