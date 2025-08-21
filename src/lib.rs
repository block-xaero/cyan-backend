mod objects;

use std::{error::Error, sync::OnceLock};

use bytemuck::{bytes_of, Pod, Zeroable};
use rkyv::{Archive, Deserialize, Serialize};
use rusted_ring::{RingBuffer, *};
use xaeroflux::{date_time::emit_secs, hash::blake_hash, pool::XaeroInternalEvent};
use xaeroflux_actors::{
    read_api::{RangeQuery, ReadApi},
    XaeroFlux,
};
use xaeroid::XaeroID;

pub const GROUP_EVENT: u32 = 1;
pub const WORKSPACE_EVENT: u32 = 2;
pub const WHITEBOARD_EVENT: u32 = 3;
pub const STICKY_NOTE_EVENT: u32 = 4;
pub const SKETCH_EVENT: u32 = 5;

pub const TOMBSTONE_OFFSET: u32 = 1000;
pub const POST_OFFSET: u32 = 2000;

pub const GROUP_TOMBSTONE: u32 = GROUP_EVENT + TOMBSTONE_OFFSET; // 1001
pub const WORKSPACE_TOMBSTONE: u32 = WORKSPACE_EVENT + TOMBSTONE_OFFSET; // 1002
pub const WHITEBOARD_TOMBSTONE: u32 = WHITEBOARD_EVENT + TOMBSTONE_OFFSET; // 1003
pub const STICKY_NOTE_TOMBSTONE: u32 = STICKY_NOTE_EVENT + TOMBSTONE_OFFSET; // 1004

pub static OBJECT_EVENT_BUFFER: OnceLock<RingBuffer<M_TSHIRT_SIZE, M_CAPACITY>> = OnceLock::new();
pub static WORKSPACE_EVENT_BUFFER: OnceLock<RingBuffer<S_TSHIRT_SIZE, S_CAPACITY>> =
    OnceLock::new();

pub static GROUP_EVENT_BUFFER: OnceLock<RingBuffer<XS_TSHIRT_SIZE, XS_CAPACITY>> = OnceLock::new();

pub static WORKSPACE_EVENT_BUFFER_READER: OnceLock<Reader<S_TSHIRT_SIZE, S_CAPACITY>> =
    OnceLock::new();
pub static WORKSPACE_EVENT_BUFFER_WRITER: OnceLock<Writer<S_TSHIRT_SIZE, S_CAPACITY>> =
    OnceLock::new();

pub static OBJECT_EVENT_BUFFER_READER: OnceLock<Reader<M_TSHIRT_SIZE, M_CAPACITY>> =
    OnceLock::new();
pub static OBJECT_EVENT_BUFFER_WRITER: OnceLock<Writer<M_TSHIRT_SIZE, M_CAPACITY>> =
    OnceLock::new();

#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Object<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize> {
    pub object_id: [u8; 32],
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub object_type: u8,
    pub parent_id: [u8; 32],
    pub child_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub children: [[u8; 32]; MAX_CHILDREN],
    pub payload: [u8; PAYLOAD_SIZE], // Raw bytes for payload
    pub _padding: [u8; 7],           // Adjusted for u8 object_type
}

unsafe impl<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize> Pod
    for Object<MAX_CHILDREN, PAYLOAD_SIZE>
{
}
unsafe impl<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize> Zeroable
    for Object<MAX_CHILDREN, PAYLOAD_SIZE>
{
}

impl<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize> Object<MAX_CHILDREN, PAYLOAD_SIZE> {
    pub fn new(
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
        parent_id: [u8; 32],
        object_type: u8,
        payload: [u8; PAYLOAD_SIZE],
    ) -> Self {
        Object {
            object_id,
            workspace_id,
            group_id,
            object_type,
            parent_id,
            child_count: 0,
            version: 1,
            created_at: emit_secs(),
            updated_at: emit_secs(),
            children: [[0u8; 32]; MAX_CHILDREN],
            payload,
            _padding: [0u8; 7],
        }
    }
}

#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Workspace<const MAX_OBJECTS: usize> {
    pub workspace_id: [u8; 32],
    pub group_id: [u8; 32],
    pub object_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub objects: [[u8; 32]; MAX_OBJECTS],
    pub _padding: [u8; 8],
}

unsafe impl<const MAX_OBJECTS: usize> Pod for Workspace<MAX_OBJECTS> {}
unsafe impl<const MAX_OBJECTS: usize> Zeroable for Workspace<MAX_OBJECTS> {}

impl<const MAX_OBJECTS: usize> Workspace<MAX_OBJECTS> {
    pub fn new(group_id: [u8; 32], name: String) -> Self {
        Workspace {
            workspace_id: blake_hash(name.as_str()),
            group_id,
            object_count: 0,
            version: 1,
            created_at: emit_secs(),
            updated_at: emit_secs(),
            objects: [[0u8; 32]; MAX_OBJECTS],
            _padding: [0u8; 8],
        }
    }
}

#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Group<const MAX_WORKSPACES: usize> {
    pub group_id: [u8; 32],
    pub parent_id: [u8; 32],
    pub workspace_count: u32,
    pub version: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub workspaces: [[u8; 32]; MAX_WORKSPACES],
    pub _padding: [u8; 31],
}

unsafe impl<const MAX_WORKSPACES: usize> Pod for Group<MAX_WORKSPACES> {}
unsafe impl<const MAX_WORKSPACES: usize> Zeroable for Group<MAX_WORKSPACES> {}

impl<const MAX_WORKSPACES: usize> Group<MAX_WORKSPACES> {
    pub fn new(name: String) -> Self {
        Group {
            group_id: blake_hash(name.as_str()),
            parent_id: [0u8; 32],
            workspace_count: 0,
            version: 1,
            created_at: emit_secs(),
            updated_at: emit_secs(),
            workspaces: [[0u8; 32]; MAX_WORKSPACES],
            _padding: [0u8; 31],
        }
    }
}

// Helper function to check if event is tombstoned
pub fn is_tombstone_event(event_type: u32) -> bool {
    event_type >= TOMBSTONE_OFFSET
}

pub trait GroupOps<const MAX_WORKSPACES: usize> {
    fn create_group(
        &mut self,
        xaero_id: [u8; 32],
        name: String,
    ) -> Result<Group<MAX_WORKSPACES>, Box<dyn std::error::Error>>;
    fn delete_group(
        &mut self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>>;
    fn list_workspaces<const MAX_OBJECTS: usize>(
        &self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
    ) -> Result<Vec<Workspace<MAX_OBJECTS>>, Box<dyn std::error::Error>>;
}

pub trait WorkspaceOps<const MAX_OBJECTS: usize> {
    fn create_workspace(
        &mut self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
        name: String,
    ) -> Result<Workspace<MAX_OBJECTS>, Box<dyn std::error::Error>>;
    fn delete_workspace(
        &mut self,
        xaero_id: [u8; 32],
        workspace_id: [u8; 32],
    ) -> Result<bool, Box<dyn std::error::Error>>;
    fn list_objects<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize>(
        &self,
        xaero_id: [u8; 32],
        workspace_id: [u8; 32],
    ) -> Result<Vec<Object<MAX_CHILDREN, PAYLOAD_SIZE>>, Box<dyn std::error::Error>>;
}

pub trait ObjectOps<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize> {
    fn create_object(
        &mut self,
        name: String,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
        parent_id: [u8; 32],
        payload: [u8; PAYLOAD_SIZE],
    ) -> Result<Object<MAX_CHILDREN, PAYLOAD_SIZE>, Box<dyn std::error::Error>>;
    fn delete_object(
        &mut self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
    ) -> Result<bool, Box<dyn std::error::Error>>;
    fn list_children(
        &self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
    ) -> Result<Vec<Object<MAX_CHILDREN, PAYLOAD_SIZE>>, Box<dyn std::error::Error>>;
}

pub struct CyanApp {
    pub xaero_flux: XaeroFlux,
}

impl CyanApp {
    pub fn init(xaero_id: XaeroID) -> Result<CyanApp, Box<dyn std::error::Error>> {
        WORKSPACE_EVENT_BUFFER_WRITER
            .get_or_init(|| RingFactory::get_writer(&WORKSPACE_EVENT_BUFFER));
        OBJECT_EVENT_BUFFER_WRITER.get_or_init(|| RingFactory::get_writer(&OBJECT_EVENT_BUFFER));
        let mut xaero_flux = XaeroFlux::new();
        xaero_flux.start_aof()?;
        xaero_flux.start_p2p(xaero_id)?;
        Ok(CyanApp { xaero_flux })
    }
}

impl<const MAX_WORKSPACES: usize> GroupOps<MAX_WORKSPACES> for CyanApp {
    fn create_group(
        &mut self,
        _xaero_id: [u8; 32],
        name: String,
    ) -> Result<Group<MAX_WORKSPACES>, Box<dyn Error>> {
        let g: Group<MAX_WORKSPACES> = Group::new(name);
        let bytes = bytemuck::bytes_of(&g);
        (self.xaero_flux.event_bus.write_optimal(bytes, GROUP_EVENT))?;
        Ok(g)
    }

    fn delete_group(
        &mut self,
        _xaero_id: [u8; 32],
        group_id: [u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.xaero_flux
            .event_bus
            .write_optimal(bytes_of(&group_id), GROUP_TOMBSTONE)?;
        Ok(())
    }

    fn list_workspaces<const MAX_OBJECTS: usize>(
        &self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
    ) -> Result<Vec<Workspace<MAX_OBJECTS>>, Box<dyn std::error::Error>> {
        let mut workspaces_found = Vec::new();

        // Check ring buffer first
        if let Some(reader) = WORKSPACE_EVENT_BUFFER_READER.get() {
            let mut reader = reader.clone();
            while let Some(event) = reader.next() {
                let workspace = bytemuck::from_bytes::<Workspace<MAX_OBJECTS>>(&event.data);
                if workspace.group_id == group_id {
                    workspaces_found.push(*workspace);
                }
            }
        }

        // Query LMDB for additional workspaces
        let query_results = self.xaero_flux.range_query_with_filter(
            RangeQuery {
                xaero_id,
                event_type: WORKSPACE_EVENT,
            },
            Box::new(move |event: &XaeroInternalEvent<MAX_OBJECTS>| {
                if is_tombstone_event(event.evt.event_type) {
                    return false;
                }
                let workspace_candidate =
                    bytemuck::from_bytes::<Workspace<MAX_OBJECTS>>(&event.evt.data);
                workspace_candidate.group_id == group_id
            }),
        )?;

        let lmdb_workspaces: Vec<Workspace<MAX_OBJECTS>> = query_results
            .into_iter()
            .map(|event| *bytemuck::from_bytes::<Workspace<MAX_OBJECTS>>(&event.evt.data))
            .collect();

        workspaces_found.extend(lmdb_workspaces);
        Ok(workspaces_found)
    }
}

impl<const MAX_OBJECTS: usize> WorkspaceOps<MAX_OBJECTS> for CyanApp {
    fn create_workspace(
        &mut self,
        _xaero_id: [u8; 32],
        group_id: [u8; 32],
        name: String,
    ) -> Result<Workspace<MAX_OBJECTS>, Box<dyn std::error::Error>> {
        let workspace: Workspace<MAX_OBJECTS> = Workspace::new(group_id, name);
        let bytes = bytemuck::bytes_of(&workspace);
        self.xaero_flux
            .event_bus
            .write_optimal(bytes, WORKSPACE_EVENT)?;
        Ok(workspace)
    }

    fn delete_workspace(
        &mut self,
        _xaero_id: [u8; 32],
        workspace_id: [u8; 32],
    ) -> Result<bool, Box<dyn std::error::Error>> {
        self.xaero_flux
            .event_bus
            .write_optimal(bytes_of(&workspace_id), WORKSPACE_TOMBSTONE)?;
        Ok(true)
    }

    fn list_objects<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize>(
        &self,
        xaero_id: [u8; 32],
        workspace_id: [u8; 32],
    ) -> Result<Vec<Object<MAX_CHILDREN, PAYLOAD_SIZE>>, Box<dyn std::error::Error>> {
        let mut objects_found = Vec::new();

        // Check ring buffer first
        if let Some(reader) = OBJECT_EVENT_BUFFER_READER.get() {
            let mut reader = reader.clone();
            while let Some(event) = reader.next() {
                let object =
                    bytemuck::from_bytes::<Object<MAX_CHILDREN, PAYLOAD_SIZE>>(&event.data);
                if object.workspace_id == workspace_id {
                    objects_found.push(*object);
                }
            }
        }

        // Query LMDB for additional objects
        let query_results = self.xaero_flux.range_query_with_filter(
            RangeQuery {
                xaero_id,
                event_type: WHITEBOARD_EVENT,
            },
            Box::new(move |event: &XaeroInternalEvent<MAX_CHILDREN>| {
                if is_tombstone_event(event.evt.event_type) {
                    return false;
                }
                let object_candidate =
                    bytemuck::from_bytes::<Object<MAX_CHILDREN,PAYLOAD_SIZE>>(&event.evt.data);
                object_candidate.workspace_id == workspace_id
            }),
        )?;

        let lmdb_objects: Vec<Object<MAX_CHILDREN, PAYLOAD_SIZE>> = query_results
            .into_iter()
            .map(|event| {
                *bytemuck::from_bytes::<Object<MAX_CHILDREN, PAYLOAD_SIZE>>(&event.evt.data)
            })
            .collect();

        objects_found.extend(lmdb_objects);
        Ok(objects_found)
    }
}

impl<const MAX_CHILDREN: usize, const PAYLOAD_SIZE: usize> ObjectOps<MAX_CHILDREN, PAYLOAD_SIZE>
    for CyanApp
{
    fn create_object(
        &mut self,
        _name: String,
        _xaero_id: [u8; 32],
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
        parent_id: [u8; 32],
        payload: [u8; PAYLOAD_SIZE],
    ) -> Result<Object<MAX_CHILDREN, PAYLOAD_SIZE>, Box<dyn std::error::Error>>{
        let object: Object<MAX_CHILDREN, PAYLOAD_SIZE> = Object::new(
            group_id,
            workspace_id,
            object_id,
            parent_id,
            3,
            payload
        );
        let bytes = bytemuck::bytes_of(&object);
        self.xaero_flux
            .event_bus
            .write_optimal(bytes, WHITEBOARD_EVENT)?;
        Ok(object)
    }

    fn delete_object(
        &mut self,
        _xaero_id: [u8; 32],
        _group_id: [u8; 32],
        _workspace_id: [u8; 32],
        object_id: [u8; 32],
    ) -> Result<bool, Box<dyn std::error::Error>> {
        self.xaero_flux
            .event_bus
            .write_optimal(bytes_of(&object_id), WHITEBOARD_TOMBSTONE)?;
        Ok(true)
    }

    fn list_children(
        &self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
    ) -> Result<Vec<Object<MAX_CHILDREN, PAYLOAD_SIZE>>, Box<dyn std::error::Error>> {
        let mut children_found = Vec::new();

        // Check ring buffer first
        if let Some(reader) = OBJECT_EVENT_BUFFER_READER.get() {
            let mut reader = reader.clone();
            while let Some(event) = reader.next() {
                let object = bytemuck::from_bytes::<Object<MAX_CHILDREN, PAYLOAD_SIZE>>(&event.data);
                if object.parent_id == object_id
                    && object.workspace_id == workspace_id
                    && object.group_id == group_id
                {
                    children_found.push(*object);
                }
            }
        }

        // Query LMDB for additional child objects
        let query_results = self.xaero_flux.range_query_with_filter(
            RangeQuery {
                xaero_id,
                event_type: STICKY_NOTE_EVENT,
            },
            Box::new(move |event: &XaeroInternalEvent<MAX_CHILDREN>| {
                if is_tombstone_event(event.evt.event_type) {
                    return false;
                }
                let object_candidate =
                    bytemuck::from_bytes::<Object<MAX_CHILDREN,PAYLOAD_SIZE>>(&event.evt.data);
                object_candidate.parent_id == object_id
                    && object_candidate.workspace_id == workspace_id
                    && object_candidate.group_id == group_id
            }),
        )?;

        let lmdb_children: Vec<Object<MAX_CHILDREN,PAYLOAD_SIZE>> = query_results
            .into_iter()
            .map(|event| *bytemuck::from_bytes::<Object<MAX_CHILDREN,PAYLOAD_SIZE>>(&event.evt.data))
            .collect();

        children_found.extend(lmdb_children);
        Ok(children_found)
    }
}
