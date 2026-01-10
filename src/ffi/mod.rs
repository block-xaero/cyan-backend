// src/ffi/mod.rs
//
// All FFI functions for Swift/C interop live here.
// This is the single source of truth for FFI - no duplicates in lib.rs.

mod scaffold;
mod core;

// Re-export scaffold utilities (used internally by FFI functions)
pub use scaffold::{compute_or_load_node_id, cstr_arg, save_node_id_to_disk, to_c_string};

// Re-export all FFI functions from core
// These are the #[no_mangle] extern "C" functions called from Swift
pub use core::*;
