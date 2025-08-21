use std::{collections::BTreeMap, sync::Arc};

use rkyv::{Archive, Deserialize, Serialize};

use crate::Workspace;

#[repr(C, align(64))]
#[derive(Serialize, Archive, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: [u8; 32],
    pub workspaces: BTreeMap<[u8; 32], Workspace>,
}

pub trait Groupable {
    fn create_new() -> Arc<Group>;
    fn delete(group: Arc<Group>) -> Result<(), String>;
    fn add_workspace(
        xaero_id: [u8; 32],
        group: Arc<Group>,
        workspace: Workspace,
    ) -> Result<Arc<Group>, String>;

    fn delete_workspace(
        xaero_id: [u8; 32],
        group: Arc<Group>,
        id: [u8; 32],
    ) -> Result<Arc<Group>, String>;
    fn search_group(
        point_query: xaeroflux_actors::read_api::PointQuery<rusted_ring::M_TSHIRT_SIZE>,
    ) -> Result<Arc<Group>, String>;
}
