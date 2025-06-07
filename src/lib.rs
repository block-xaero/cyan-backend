mod workspaces;
// in src/lib.rs
use std::sync::Arc;

use once_cell::sync::OnceCell;
use xaeroflux::{
    actors::{
        pipe::{BusKind, Pipe},
        XaeroEvent,
    },
    event::Event,
};

static GLOBAL_PIPE: OnceCell<Arc<Pipe>> = OnceCell::new();

#[unsafe(no_mangle)]
pub extern "C" fn xaero_init(bus_kind: u8) {
    let kind = if bus_kind == 0 {
        BusKind::Control
    } else {
        BusKind::Data
    };
    let pipe = Pipe::new(kind, None);
    GLOBAL_PIPE.set(pipe).ok();
}

/// Call this from Dart whenever you have a new raw payload to inject.
/// `data_ptr` points at a native heap buffer you’ve filled in Dart,
/// `len` is its length, and `etype` is the XAeroEventType byte.
#[unsafe(no_mangle)]
pub extern "C" fn xaero_publish(data_ptr: *const u8, len: usize, etype: u8) {
    let slice = unsafe { std::slice::from_raw_parts(data_ptr, len) };
    let evt = Event::new(slice.to_vec(), etype);
    let xe = XaeroEvent {
        evt,
        merkle_proof: None,
    };
    if let Some(pipe) = GLOBAL_PIPE.get() {
        let _ = pipe.sink.tx.send(xe);
    }
}
