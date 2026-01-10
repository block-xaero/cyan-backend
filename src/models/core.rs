// src/models/core.rs
//
// Core entity types - base types with no internal dependencies

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════════
// BASIC ENTITY TYPES
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub group_id: String,
    pub name: String,
    pub created_at: i64,
}