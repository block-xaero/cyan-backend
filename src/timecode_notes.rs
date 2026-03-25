// cyan-backend/src/timecode_notes.rs
//
// Timecoded notes for video assets.
// Notes are stored as notebook cells with cell_type="timecode_note"
// and metadata containing timecode, pipeline context, and AI action state.
//
// This means notes gossip-sync automatically via existing notebook cell infra.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::models::commands::CommandMsg;
use crate::storage;

// ============================================================================
// Timecode Note Model
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimecodeNote {
    pub id: String,
    pub board_id: String,
    pub timecode_seconds: f64,
    pub content: String,
    pub note_type: String,          // comment, qc_issue, revision, approved, action
    pub author: String,
    pub created_at: i64,
    
    // Pipeline context
    #[serde(default)]
    pub pipeline_step_id: Option<String>,
    #[serde(default)]
    pub pipeline_phase: Option<String>,     // pre_exec, during, review, post_approval
    #[serde(default)]
    pub ai_reviewed: bool,
    #[serde(default)]
    pub human_approved: bool,
    
    // AI action
    #[serde(default)]
    pub action_skill: Option<String>,       // which skill to invoke (dub, subtitle, qc)
    #[serde(default)]
    pub action_status: Option<String>,      // pending, sent, complete, rejected
    #[serde(default)]
    pub action_result: Option<String>,      // AI's response
    #[serde(default)]
    pub action_model: Option<String>,       // which model was used
    
    // Context for dedup
    #[serde(default)]
    pub ai_flags_nearby: Vec<AiFlag>,       // what AI already found near this timecode
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiFlag {
    pub timecode_seconds: f64,
    pub description: String,
    pub severity: String,           // info, warning, critical
    pub source_step: String,        // which pipeline step flagged this
    pub resolved: bool,
}

// ============================================================================
// CRUD Operations
// ============================================================================

/// Save a timecoded note as a notebook cell
pub fn save_note(
    note: &TimecodeNote,
    command_tx: &UnboundedSender<CommandMsg>,
) -> Result<()> {
    let metadata = json!({
        "timecode_seconds": note.timecode_seconds,
        "note_type": note.note_type,
        "author": note.author,
        "pipeline_step_id": note.pipeline_step_id,
        "pipeline_phase": note.pipeline_phase,
        "ai_reviewed": note.ai_reviewed,
        "human_approved": note.human_approved,
        "action_skill": note.action_skill,
        "action_status": note.action_status,
        "action_result": note.action_result,
        "action_model": note.action_model,
        "ai_flags_nearby": note.ai_flags_nearby,
    });
    
    // Cell order: use timecode * 1000 to sort notes chronologically
    let cell_order = (note.timecode_seconds * 1000.0) as i32;
    
    let _ = command_tx.send(CommandMsg::UpdateNotebookCell {
        id: note.id.clone(),
        board_id: note.board_id.clone(),
        cell_type: "timecode_note".to_string(),
        cell_order,
        content: Some(note.content.clone()),
        output: None,
        collapsed: false,
        height: None,
        metadata_json: Some(metadata.to_string()),
    });
    
    tracing::info!(
        "📝 Saved timecoded note at {:.1}s: {} (step: {:?})",
        note.timecode_seconds,
        &note.content[..note.content.len().min(50)],
        note.pipeline_step_id
    );
    
    Ok(())
}

/// Load all timecoded notes for a board
pub fn load_notes(board_id: &str) -> Result<Vec<TimecodeNote>> {
    let conn = storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
    
    let mut stmt = conn.prepare(
        "SELECT id, board_id, cell_order, content, metadata_json \
         FROM notebook_cells \
         WHERE board_id = ?1 AND cell_type = 'timecode_note' \
         ORDER BY cell_order"
    )?;
    
    let notes = stmt.query_map(rusqlite::params![board_id], |row| {
        let id: String = row.get(0)?;
        let board_id: String = row.get(1)?;
        let content: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
        let metadata_json: Option<String> = row.get(4)?;
        
        let meta: serde_json::Value = metadata_json.as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        
        Ok(TimecodeNote {
            id,
            board_id,
            timecode_seconds: meta["timecode_seconds"].as_f64().unwrap_or(0.0),
            content,
            note_type: meta["note_type"].as_str().unwrap_or("comment").to_string(),
            author: meta["author"].as_str().unwrap_or("unknown").to_string(),
            created_at: meta["created_at"].as_i64().unwrap_or(0),
            pipeline_step_id: meta["pipeline_step_id"].as_str().map(String::from),
            pipeline_phase: meta["pipeline_phase"].as_str().map(String::from),
            ai_reviewed: meta["ai_reviewed"].as_bool().unwrap_or(false),
            human_approved: meta["human_approved"].as_bool().unwrap_or(false),
            action_skill: meta["action_skill"].as_str().map(String::from),
            action_status: meta["action_status"].as_str().map(String::from),
            action_result: meta["action_result"].as_str().map(String::from),
            action_model: meta["action_model"].as_str().map(String::from),
            ai_flags_nearby: serde_json::from_value(
                meta["ai_flags_nearby"].clone()
            ).unwrap_or_default(),
        })
    })?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(notes)
}

/// Delete a timecoded note
pub fn delete_note(note_id: &str, board_id: &str, command_tx: &UnboundedSender<CommandMsg>) -> Result<()> {
    let _ = command_tx.send(CommandMsg::DeleteNotebookCell {
        id: note_id.to_string(),
        board_id: board_id.to_string(),
    });
    Ok(())
}

// ============================================================================
// AI Action Dispatch
// ============================================================================

/// Send a timecoded note to AI for action
/// The note's content + pipeline context + video timecode get sent to vLLM
pub async fn act_on_note(
    note: &TimecodeNote,
    command_tx: &UnboundedSender<CommandMsg>,
) -> Result<String> {
    // Build context-aware prompt
    let mut prompt = String::new();
    
    prompt.push_str(&format!(
        "You are reviewing a video asset. A human reviewer left a note at timecode {:.1}s.\n\n",
        note.timecode_seconds
    ));
    
    // Add pipeline context
    if let Some(ref step_id) = note.pipeline_step_id {
        prompt.push_str(&format!("Current pipeline step: {}\n", step_id));
    }
    if let Some(ref phase) = note.pipeline_phase {
        prompt.push_str(&format!("Pipeline phase: {}\n", phase));
    }
    
    // Add AI flags context to avoid duplicate work
    if !note.ai_flags_nearby.is_empty() {
        prompt.push_str("\nAI has already flagged the following near this timecode:\n");
        for flag in &note.ai_flags_nearby {
            prompt.push_str(&format!(
                "  - [{:.1}s] {} (severity: {}, resolved: {})\n",
                flag.timecode_seconds, flag.description, flag.severity, flag.resolved
            ));
        }
        prompt.push_str("\nDo NOT duplicate these findings. Focus on what the human found that AI missed.\n");
    }
    
    prompt.push_str(&format!(
        "\nHuman's note: \"{}\"\nNote type: {}\n\n\
         Based on this context, provide:\n\
         1. Analysis of the issue\n\
         2. Recommended action (specific command or skill to run)\n\
         3. Whether this requires a re-run of any pipeline step\n\
         4. Priority: low / medium / high / critical\n\n\
         Respond in JSON format with fields: analysis, action, rerun_step (null or step_id), priority",
        note.content, note.note_type
    ));
    
    // Call vLLM
    let result = crate::pipeline::call_vllm_public(&prompt, 500, 0.3).await?;
    
    // Update note with AI's response
    let mut updated_note = note.clone();
    updated_note.ai_reviewed = true;
    updated_note.action_status = Some("complete".to_string());
    updated_note.action_result = Some(result.clone());
    
    save_note(&updated_note, command_tx)?;
    
    tracing::info!(
        "🤖 AI acted on note at {:.1}s: {}",
        note.timecode_seconds,
        &result[..result.len().min(100)]
    );
    
    Ok(result)
}

/// Get AI flags near a timecode (within a window) from pipeline step results
pub fn get_ai_flags_near_timecode(
    board_id: &str,
    timecode: f64,
    window_seconds: f64,
    step_id: Option<&str>,
) -> Vec<AiFlag> {
    // Load existing notes that are AI-generated
    let notes = load_notes(board_id).unwrap_or_default();
    
    notes.iter()
        .filter(|n| {
            n.ai_reviewed
                && (n.timecode_seconds - timecode).abs() <= window_seconds
                && step_id.map_or(true, |s| n.pipeline_step_id.as_deref() == Some(s))
        })
        .map(|n| AiFlag {
            timecode_seconds: n.timecode_seconds,
            description: n.content.clone(),
            severity: if n.note_type == "qc_issue" { "warning".to_string() } else { "info".to_string() },
            source_step: n.pipeline_step_id.clone().unwrap_or_default(),
            resolved: n.human_approved,
        })
        .collect()
}

// ============================================================================
// FFI-friendly wrappers
// ============================================================================

/// Save a note — called from FFI (blocking)
pub fn save_note_ffi(json_str: &str) -> Result<()> {
    let note: TimecodeNote = serde_json::from_str(json_str)
        .map_err(|e| anyhow!("Invalid note JSON: {}", e))?;
    
    let system = crate::SYSTEM.get().ok_or_else(|| anyhow!("System not initialized"))?;
    save_note(&note, &system.command_tx)
}

/// Load notes — called from FFI (blocking)
pub fn load_notes_ffi(board_id: &str) -> Result<String> {
    let notes = load_notes(board_id)?;
    Ok(serde_json::to_string(&notes)?)
}

/// Act on a note — called from FFI (blocking, runs async)
pub fn act_on_note_ffi(note_json: &str) -> Result<String> {
    let note: TimecodeNote = serde_json::from_str(note_json)
        .map_err(|e| anyhow!("Invalid note JSON: {}", e))?;
    
    let system = crate::SYSTEM.get().ok_or_else(|| anyhow!("System not initialized"))?;
    let rt = crate::RUNTIME.get().ok_or_else(|| anyhow!("Runtime not available"))?;
    
    rt.block_on(act_on_note(&note, &system.command_tx))
}
