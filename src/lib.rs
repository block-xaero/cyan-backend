use bytemuck::{Pod, Zeroable};
use rkyv::{Archive, Deserialize, Serialize};

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
    pub _padding: [u8; 8],
}

unsafe impl<const MAX_WORKSPACES: usize> Pod for Group<MAX_WORKSPACES> {}
unsafe impl<const MAX_WORKSPACES: usize> Zeroable for Group<MAX_WORKSPACES> {}