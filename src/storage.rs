use bytemuck::Zeroable;
use std::{char::MAX, sync::OnceLock};
use xaeroflux_actors::{
    aof::storage::lmdb::{get_children_entitities_by_entity_id, get_current_state_by_entity_id},
    XaeroFlux,
};

use crate::{Group, Workspace};

pub static DATA_DIR: &str = "cyan";
pub fn get_group_by_group_id<const SIZE: usize>(
    group_id: [u8; 32],
) -> Result<Group<SIZE>, Box<dyn std::error::Error>> {
    let mut xf_handle = XaeroFlux::instance(DATA_DIR)
        .get()
        .expect("expected to initialize xaeroflux first!");
    match get_current_state_by_entity_id(
        &mut xf_handle
            .read_handle
            .clone()
            .expect("read handle not ready yet!"),
        group_id,
    ) {
        Ok(current_state_opt) =>
            if let Some(current_state) = current_state_opt {
                Ok(*bytemuck::from_bytes::<Group<SIZE>>(
                    &current_state.evt.data,
                ))
            } else {
                Err("not found".into())
            },
        Err(e) => {
            return Err(e);
        }
    }
}

pub fn get_workspaces_for_group<const SIZE: usize, const MAX_WORKSPACES: usize>(
    group_id: [u8; 32],
) -> Result<([Workspace<SIZE>; MAX_WORKSPACES], usize), Box<dyn std::error::Error>> {
    let mut workspaces = [Workspace::<SIZE>::zeroed(); MAX_WORKSPACES];
    let mut count = 0;

    let rh = XaeroFlux::read_handle().expect("read handle not ready yet!");
    let workspace_ids = get_children_entitities_by_entity_id(&rh, group_id)?;

    for chunk in workspace_ids.chunks_exact(32) {
        if count >= MAX_WORKSPACES {
            tracing::warn!("Group has more than {} workspaces, truncating", MAX_WORKSPACES);
            break;
        }

        let mut buffer = [0u8; 32];
        buffer.copy_from_slice(chunk);

        match get_current_state_by_entity_id(&rh, buffer)? {
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
