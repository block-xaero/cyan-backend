// cyan-backend/src/pipeline_executor.rs
//
// Pipeline step executor that routes through Cyan Lens.
// 
// Flow:
//   1. Pipeline sends step to Lens: POST /api/v1/execute
//   2. Lens runs ReAct loop, returns either:
//      a) Final result (cloud step — Lens ran everything)
//      b) Pending tool calls (local step — needs client to run tools)
//   3. For local steps, backend runs tools and sends results back
//   4. Loop until Lens returns final result
//   5. Save findings as timecoded notes
//   6. Publish pipeline events to Iggy for enrichment
//
// This replaces the direct skill execution in pipeline.rs

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::models::commands::CommandMsg;
use crate::models::events::SwiftEvent;

// ============================================================================
// Lens API Types
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct LensExecuteRequest {
    pub step_id: String,
    pub board_id: String,
    pub cell_content: String,
    pub executor_type: String,              // "local" or "cloud"
    pub metadata: Option<serde_json::Value>,
    pub previous_outputs: Vec<serde_json::Value>,
    pub human_input: Option<String>,
    pub tools_markdown: Option<String>,     // client-defined tools
    pub skills_markdown: Option<String>,    // client-defined skills
}

#[derive(Debug, Clone, Deserialize)]
pub struct LensExecuteResponse {
    pub success: bool,
    pub run_id: String,
    pub status: String,                     // "complete", "failed", "needs_tool_execution", "needs_human"
    #[serde(default)]
    pub result: Option<StepResult>,
    #[serde(default)]
    pub pending_tool_calls: Vec<ToolCall>,   // tools for client to execute locally
    #[serde(default)]
    pub status_markers: Vec<StatusMarker>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub reasoning_trace: Vec<serde_json::Value>,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub timecode_seconds: f64,
    pub content: String,
    pub finding_type: String,
    pub severity: String,
    #[serde(default)]
    pub suggested_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_id: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub tool_id: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LensContinueRequest {
    pub run_id: String,
    pub tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusMarker {
    pub timestamp: i64,
    pub icon: String,
    pub message: String,
}

// ============================================================================
// Iggy Pipeline Event Types
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct PipelineEvent {
    pub event_type: String,         // step_started, step_completed, step_failed, finding_created, human_approved
    pub board_id: String,
    pub step_id: String,
    pub run_id: String,
    pub timestamp: i64,
    pub data: serde_json::Value,
}

// ============================================================================
// Execute Step via Lens (with local/cloud routing)
// ============================================================================

/// Execute a pipeline step through Cyan Lens.
/// For cloud steps: Lens runs everything.
/// For local steps: Lens orchestrates, client executes tools locally.
pub async fn execute_step_via_lens(
    lens_url: &str,
    board_id: &str,
    step_id: &str,
    cell_content: &str,
    executor_type: &str,
    metadata: Option<serde_json::Value>,
    previous_outputs: Vec<serde_json::Value>,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<(String, Vec<Finding>)> {
    
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    
    let run_id = format!("run_{}_{}", &step_id[..step_id.len().min(8)], chrono::Utc::now().timestamp() % 10000);
    
    // Publish: step started
    publish_pipeline_event(event_tx, PipelineEvent {
        event_type: "step_started".into(),
        board_id: board_id.into(),
        step_id: step_id.into(),
        run_id: run_id.clone(),
        timestamp: chrono::Utc::now().timestamp(),
        data: json!({ "executor_type": executor_type, "cell_content": &cell_content[..cell_content.len().min(100)] }),
    });
    
    // Step 1: Send initial execute request
    let request = LensExecuteRequest {
        step_id: step_id.into(),
        board_id: board_id.into(),
        cell_content: cell_content.into(),
        executor_type: executor_type.into(),
        metadata,
        previous_outputs,
        human_input: None,
        tools_markdown: None,
        skills_markdown: None,
    };
    
    eprintln!("📺 PIPELINE: Step {} → Lens API ({} executor)", step_id, executor_type);
    
    let _ = event_tx.send(SwiftEvent::StatusUpdate {
        message: format!("🔄 Step '{}' → Cyan Lens", step_id),
    });
    
    let mut response = call_lens_execute(&client, lens_url, &request).await?;
    
    // Step 2: Handle back-and-forth for local tool execution
    let mut iteration = 0;
    let max_iterations = 20; // safety limit
    
    while response.status == "needs_tool_execution" && iteration < max_iterations {
        iteration += 1;
        
        eprintln!("📺 PIPELINE: Step {} needs {} local tool calls (iteration {})", 
            step_id, response.pending_tool_calls.len(), iteration);
        
        // Send status markers to UI
        for marker in &response.status_markers {
            let _ = event_tx.send(SwiftEvent::StatusUpdate {
                message: format!("{} {}", marker.icon, marker.message),
            });
        }
        
        // Execute tools locally
        let mut tool_results = Vec::new();
        for tool_call in &response.pending_tool_calls {
            let _ = event_tx.send(SwiftEvent::StatusUpdate {
                message: format!("🔧 Running {} locally...", tool_call.tool_id),
            });
            
            let result = execute_tool_locally(tool_call).await;
            
            eprintln!("📺 PIPELINE: Local {} → exit={}", 
                tool_call.tool_id, result.exit_code);
            
            tool_results.push(result);
        }
        
        // Send results back to Lens
        let continue_req = LensContinueRequest {
            run_id: run_id.clone(),
            tool_results,
        };
        
        response = call_lens_continue(&client, lens_url, &continue_req).await?;
    }
    
    // Step 3: Process final response
    // Send remaining status markers
    for marker in &response.status_markers {
        let _ = event_tx.send(SwiftEvent::StatusUpdate {
            message: format!("{} {}", marker.icon, marker.message),
        });
    }
    
    if let Some(ref result) = response.result {
        // Save findings as timecoded notes
        let findings = result.findings.clone();
        for finding in &findings {
            let note = crate::timecode_notes::TimecodeNote {
                id: uuid::Uuid::new_v4().to_string(),
                board_id: board_id.to_string(),
                timecode_seconds: finding.timecode_seconds,
                content: finding.content.clone(),
                note_type: finding.finding_type.clone(),
                author: format!("AI/{}", step_id),
                created_at: chrono::Utc::now().timestamp() as f64,
                pipeline_step_id: Some(step_id.to_string()),
                pipeline_phase: Some("during".to_string()),
                ai_reviewed: true,
                human_approved: false,
                action_skill: None,
                action_status: Some("complete".to_string()),
                action_result: finding.suggested_action.clone(),
                action_model: result.tools_used.first().cloned(),
                ai_flags_nearby: vec![],
                reply_to: None,
                thread_count: 0,
            };
            let _ = crate::timecode_notes::save_note(&note, command_tx);
        }
        
        if !findings.is_empty() {
            eprintln!("📺 PIPELINE: Saved {} timecoded notes for step {}", findings.len(), step_id);
        }
        
        // Publish: step completed
        publish_pipeline_event(event_tx, PipelineEvent {
            event_type: "step_completed".into(),
            board_id: board_id.into(),
            step_id: step_id.into(),
            run_id: run_id.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            data: json!({
                "summary": result.summary,
                "findings_count": findings.len(),
                "tools_used": result.tools_used,
                "duration_ms": result.duration_ms,
            }),
        });
        
        // Publish each finding as a separate event (for graph enrichment)
        for finding in &findings {
            publish_pipeline_event(event_tx, PipelineEvent {
                event_type: "finding_created".into(),
                board_id: board_id.into(),
                step_id: step_id.into(),
                run_id: run_id.clone(),
                timestamp: chrono::Utc::now().timestamp(),
                data: json!({
                    "timecode_seconds": finding.timecode_seconds,
                    "content": finding.content,
                    "finding_type": finding.finding_type,
                    "severity": finding.severity,
                }),
            });
        }
        
        Ok((result.summary.clone(), findings))
    } else if response.status == "needs_human" {
        let question = response.error.unwrap_or_else(|| "Human input needed".into());
        
        publish_pipeline_event(event_tx, PipelineEvent {
            event_type: "step_needs_human".into(),
            board_id: board_id.into(),
            step_id: step_id.into(),
            run_id: run_id.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            data: json!({ "question": question }),
        });
        
        Err(anyhow!("needs_human: {}", question))
    } else {
        let error = response.error.unwrap_or_else(|| "Unknown error".into());
        
        publish_pipeline_event(event_tx, PipelineEvent {
            event_type: "step_failed".into(),
            board_id: board_id.into(),
            step_id: step_id.into(),
            run_id: run_id.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            data: json!({ "error": error }),
        });
        
        Err(anyhow!("Lens execution failed: {}", error))
    }
}

// ============================================================================
// Local Tool Execution
// ============================================================================

async fn execute_tool_locally(tool_call: &ToolCall) -> ToolResult {
    let timeout = if tool_call.timeout_seconds > 0 { tool_call.timeout_seconds } else { 60 };
    
    let binary = match tool_call.tool_id.as_str() {
        "ffprobe" => "ffprobe",
        "ffmpeg" => "ffmpeg",
        "whisper" => "whisper",
        other => other,
    };
    
    match tokio::time::timeout(
        std::time::Duration::from_secs(timeout),
        tokio::process::Command::new(binary)
            .args(&tool_call.args)
            .output()
    ).await {
        Ok(Ok(output)) => ToolResult {
            call_id: tool_call.call_id.clone(),
            tool_id: tool_call.tool_id.clone(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        },
        Ok(Err(e)) => ToolResult {
            call_id: tool_call.call_id.clone(),
            tool_id: tool_call.tool_id.clone(),
            stdout: String::new(),
            stderr: format!("Execution error: {}", e),
            exit_code: -1,
        },
        Err(_) => ToolResult {
            call_id: tool_call.call_id.clone(),
            tool_id: tool_call.tool_id.clone(),
            stdout: String::new(),
            stderr: format!("Timed out after {}s", timeout),
            exit_code: -1,
        },
    }
}

// ============================================================================
// Lens API Calls
// ============================================================================

async fn call_lens_execute(
    client: &reqwest::Client,
    lens_url: &str,
    request: &LensExecuteRequest,
) -> Result<LensExecuteResponse> {
    let url = format!("{}/api/v1/execute", lens_url);
    
    let response = client.post(&url)
        .json(request)
        .send()
        .await
        .map_err(|e| anyhow!("Lens API unreachable: {}", e))?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Lens API returned {}: {}", status, &body[..body.len().min(200)]));
    }
    
    response.json().await
        .map_err(|e| anyhow!("Failed to parse Lens response: {}", e))
}

async fn call_lens_continue(
    client: &reqwest::Client,
    lens_url: &str,
    request: &LensContinueRequest,
) -> Result<LensExecuteResponse> {
    let url = format!("{}/api/v1/execute/continue", lens_url);
    
    let response = client.post(&url)
        .json(request)
        .send()
        .await
        .map_err(|e| anyhow!("Lens continue API unreachable: {}", e))?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Lens continue API returned {}: {}", status, &body[..body.len().min(200)]));
    }
    
    response.json().await
        .map_err(|e| anyhow!("Failed to parse Lens continue response: {}", e))
}

// ============================================================================
// Pipeline Event Publishing (→ Iggy → Lens enricher → Graph)
// ============================================================================

fn publish_pipeline_event(
    event_tx: &UnboundedSender<SwiftEvent>,
    event: PipelineEvent,
) {
    eprintln!("📡 PIPELINE EVENT: {} [{}] step={}", event.event_type, event.board_id[..8].to_string(), event.step_id);
    
    // Send as SwiftEvent::GenericEvent which gets routed to Iggy via the network actor
    let _ = event_tx.send(SwiftEvent::StatusUpdate { message: format!("📡 pipeline.{}: step={}", event.event_type, event.step_id) });
}

// ============================================================================
// Integration with existing pipeline.rs
// ============================================================================

/// Drop-in replacement for the skill execution block in run_pipeline().
/// Call this instead of the old skill_match + execute_skill path.
pub async fn execute_pipeline_step(
    board_id: &str,
    step_id: &str,
    cell_content: &str,
    executor_type: &str,
    metadata: Option<serde_json::Value>,
    previous_outputs: Vec<serde_json::Value>,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<(String, Vec<Finding>)> {
    let lens_url = std::env::var("CYAN_LENS_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    
    // Try Lens first
    match execute_step_via_lens(
        &lens_url, board_id, step_id, cell_content, executor_type,
        metadata, previous_outputs.clone(), command_tx, event_tx,
    ).await {
        Ok(result) => Ok(result),
        Err(lens_err) => {
            eprintln!("📺 PIPELINE: Lens failed for step {}: {}. Falling back to local.", step_id, lens_err);
            
            let _ = event_tx.send(SwiftEvent::StatusUpdate {
                message: format!("⚠️ Lens unavailable, running '{}' locally", step_id),
            });
            
            // Fall back to local skill execution
            execute_step_locally(
                board_id, step_id, cell_content,
                previous_outputs, command_tx, event_tx,
            ).await
        }
    }
}

/// Local fallback — uses the existing skill system in cyan-backend
async fn execute_step_locally(
    board_id: &str,
    step_id: &str,
    cell_content: &str,
    previous_outputs: Vec<serde_json::Value>,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<(String, Vec<Finding>)> {
    let registry = crate::skills::registry();
    let skill_match = registry.resolve_intent(cell_content);
    
    if let Some(skill_def) = skill_match {
        let video_uri = find_video_uri(board_id);
        let scope_id = find_scope_id(board_id);
        
        let skill_ctx = crate::skills::SkillContext {
            board_id: board_id.to_string(),
            step_id: step_id.to_string(),
            credentials: std::collections::HashMap::new(),
            cell_content: cell_content.to_string(),
            previous_outputs: previous_outputs.iter()
                .filter_map(|v| {
                    Some(crate::skills::StepOutput {
                        step_id: v["step_id"].as_str()?.to_string(),
                        output: v["summary"].as_str()?.to_string(),
                        output_type: crate::skills::OutputType::Summary,
                        artifacts: std::collections::HashMap::new(),
                    })
                })
                .collect(),
            video_uri,
            scope_id,
        };
        
        match crate::skills::execute_skill(&skill_def.id, &skill_ctx).await {
            Ok(skill_result) => {
                let mut findings = Vec::new();
                
                // Convert skill findings to our Finding type and save as notes
                if let Some(ref sf) = skill_result.timecoded_findings {
                    for f in sf {
                        let finding = Finding {
                            timecode_seconds: f.timecode_seconds,
                            content: f.content.clone(),
                            finding_type: f.finding_type.clone(),
                            severity: f.severity.clone(),
                            suggested_action: f.suggested_action.clone(),
                        };
                        
                        let note = crate::timecode_notes::TimecodeNote {
                            id: uuid::Uuid::new_v4().to_string(),
                            board_id: board_id.to_string(),
                            timecode_seconds: f.timecode_seconds,
                            content: f.content.clone(),
                            note_type: f.finding_type.clone(),
                            author: format!("AI/{}", skill_def.id),
                            created_at: chrono::Utc::now().timestamp() as f64,
                            pipeline_step_id: Some(step_id.to_string()),
                            pipeline_phase: Some("during".to_string()),
                            ai_reviewed: true,
                            human_approved: false,
                            action_skill: None,
                            action_status: Some("complete".to_string()),
                            action_result: f.suggested_action.clone(),
                            action_model: skill_def.tools.first().cloned(),
                            ai_flags_nearby: vec![],
                            reply_to: None,
                            thread_count: 0,
                        };
                        let _ = crate::timecode_notes::save_note(&note, command_tx);
                        findings.push(finding);
                    }
                }
                
                Ok((skill_result.summary, findings))
            }
            Err(e) => Err(e),
        }
    } else {
        // No skill match — try raw vLLM call
        let prompt = format!("Execute this pipeline step:\n\n{}", cell_content);
        let response = crate::pipeline::call_vllm_public(&prompt, 800, 0.3).await?;
        Ok((response, vec![]))
    }
}

// ============================================================================
// Helpers (imported from pipeline.rs)
// ============================================================================

fn find_video_uri(board_id: &str) -> Option<String> {
    let conn = crate::storage::db().lock().ok()?;
    let mut stmt = conn.prepare(
        "SELECT content FROM notebook_cells WHERE board_id = ?1 AND cell_type = 'markdown' ORDER BY cell_order LIMIT 1"
    ).ok()?;
    
    let content: Option<String> = stmt.query_row(rusqlite::params![board_id], |row| row.get(0)).ok();
    
    content.filter(|c| c.starts_with("http") && (c.contains(".mp4") || c.contains(".mov") || c.contains(".mxf")))
}

fn find_scope_id(board_id: &str) -> Option<String> {
    let conn = crate::storage::db().lock().ok()?;
    let mut stmt = conn.prepare(
        "SELECT workspace_id FROM objects WHERE id = ?1 LIMIT 1"
    ).ok()?;
    
    stmt.query_row(rusqlite::params![board_id], |row| row.get::<_, String>(0)).ok()
}

pub fn find_asset_metadata(_board_id: &str) -> Option<serde_json::Value> {
    // For demo: return BigBuckBunny metadata
    // In production: read from first cell or MAM API
    Some(json!({
        "title": "Big Buck Bunny",
        "content_type": "animated_film",
        "genre": ["animation", "comedy", "family"],
        "source_language": "en",
        "target_languages": ["hi", "ta", "te", "kn"],
        "target_markets": ["IN", "SG", "AE"],
        "resolution": "FHD",
        "duration_seconds": 596.0,
        "rating": "U",
        "ad_tier": "premium",
        "historical_cpm": 450.0,
        "engagement_curve": [0.98, 0.95, 0.92, 0.88, 0.85, 0.82, 0.78, 0.75, 0.72, 0.71],
        "delivery_platforms": [
            {"platform": "JioStar", "format": "HEVC_1080p"},
            {"platform": "YouTube", "format": "H264_4K"},
            {"platform": "Hotstar", "format": "ABR_ladder"}
        ]
    }))
}
