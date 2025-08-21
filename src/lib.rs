use std::{error::Error, sync::OnceLock};

use bytemuck::{Pod, Zeroable, bytes_of};
use rkyv::{Archive, Deserialize, Serialize};
use rusted_ring::{RingBuffer, *};
use xaeroflux::{date_time::emit_secs, hash::blake_hash, pool::XaeroInternalEvent};
use xaeroflux_actors::{
    XaeroFlux,
    read_api::{RangeQuery, ReadApi},
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

#[repr(C, align(64))]
#[derive(Archive, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Object<const MAX_CHILDREN: usize> {
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
    pub _padding: [u8; 8],
}

unsafe impl<const MAX_CHILDREN: usize> Pod for Object<MAX_CHILDREN> {}
unsafe impl<const MAX_CHILDREN: usize> Zeroable for Object<MAX_CHILDREN> {}

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
            version: 0,
            created_at: emit_secs(),
            updated_at: emit_secs(),
            workspaces: [[0u8; 32]; MAX_WORKSPACES],
            _padding: [0u8; 31],
        }
    }
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
    fn list_workspaces(&self, group_id: [u8; 32]) -> Result<bool, Box<dyn std::error::Error>>;
}

pub trait ObjectOps<const MAX_CHILDREN: usize> {
    fn create_object(
        &mut self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
        workspace_id: [u8; 32],
        object_id: [u8; 32],
        name: String,
    ) -> Result<Object<MAX_CHILDREN>, Box<dyn std::error::Error>>;
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
    ) -> Result<Vec<Object<MAX_CHILDREN>>, Box<dyn std::error::Error>>;
}

pub struct CyanApp {
    pub xaero_flux: XaeroFlux,
}
impl CyanApp {
    pub fn init(xaero_id: XaeroID) -> Result<CyanApp, Box<dyn std::error::Error>> {
        WORKSPACE_EVENT_BUFFER_WRITER
            .get_or_init(|| RingFactory::get_writer(&WORKSPACE_EVENT_BUFFER));
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
        (self.xaero_flux.event_bus.write_optimal(bytes, 1))?;
        Ok(g)
    }

    fn delete_group(
        &mut self,
        _xaero_id: [u8; 32],
        group_id: [u8; 32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        // look in ring quickly
        self.xaero_flux
            .event_bus
            .write_optimal(bytes_of(&group_id), GROUP_TOMBSTONE)
            .map_err(|xfe| Box::new(xfe) as Box<dyn std::error::Error>)
    }

    fn list_workspaces<const MAX_OBJECTS: usize>(
        &self,
        xaero_id: [u8; 32],
        group_id: [u8; 32],
    ) -> Result<Vec<Workspace<MAX_OBJECTS>>, Box<dyn std::error::Error>> {
        // look in ring
        let mut workspaces_found = Vec::new();
        let mut reader = WORKSPACE_EVENT_BUFFER_READER
            .get()
            .expect("READER not ready yet!")
            .clone(); // Clone the reader to get ownership
        while let Some(event) = reader.by_ref().next() {
            let workspace = bytemuck::from_bytes::<Workspace<MAX_OBJECTS>>(&event.data);
            if workspace.group_id == group_id {
                workspaces_found.push(*workspace);
            }
        }
        // look in lmdb and refresh ring
        let query_results = self.xaero_flux.range_query_with_filter(
            RangeQuery {
                xaero_id,
                event_type: WORKSPACE_EVENT,
            },
            Box::new(|event: &XaeroInternalEvent<MAX_OBJECTS>| {
                let workspace_candidate =
                    bytemuck::from_bytes::<Workspace<MAX_OBJECTS>>(&event.evt.data);
                workspace_candidate.group_id == group_id
            }),
        )?;
        let lmdb_workspaces: Vec<Workspace<MAX_OBJECTS>> = query_results
            .into_iter()
            .map(|event| *bytemuck::from_bytes::<Workspace<MAX_OBJECTS>>(&event.evt.data))
            .collect();

        // Extend with owned values
        workspaces_found.extend(lmdb_workspaces);
        Ok(workspaces_found)
    }
}
