// src/ai_bridge.rs
//
// Bridges xaeroai crate into Cyan's FFI layer.
//
// Features:
// 1. WhiteboardPipeline for image→mermaid
// 2. Runtime for cyan-lens Phi inference
// 3. Event buffer for analyst context
// 4. Proactive insights generation
// 5. Model registry for notebook cells
// 6. CyanLens search with SQL generation
// 7. Playbook for learned patterns

use crate::SwiftEvent;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

use xaeroai::{
    lens, playbook, ActionPlan, CyanLens, ExecutionResult, Executor,
    FeedbackTag, InferenceInput, InferenceOutput, ModelRecord, ParsedOutput,
    Runtime, Section, Skill, WhiteboardPipeline,
};

const EVENT_BUFFER_SIZE: usize = 500;
const INSIGHT_INTERVAL_SECS: u64 = 60;

// ============================================================================
// Public Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIIntegrationEvent {
    pub anchor_id: String,
    pub anchor_kind: String,
    pub anchor_ref: String,
    pub mention_count: u32,
    pub last_mention: i64,
    pub context: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveInsight {
    pub id: String,
    pub message: String,
    pub severity: String,
    pub anchor_ids: Vec<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MermaidResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mermaid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shapes: Option<Vec<DetectedShape>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<PipelineTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedShape {
    pub id: usize,
    pub shape_type: String,
    pub confidence: f32,
    pub text: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTiming {
    pub detection_ms: u64,
    pub ocr_ms: u64,
    pub layout_ms: u64,
    pub generation_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citations: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub capabilities: Vec<String>,
    pub board_id: String,
    pub cell_id: String,
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BulletFeedbackInput {
    bullet_id: String,
    tag: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LensCorrectionInput {
    wrong_sql: Option<String>,
    correct_sql: Option<String>,
    explanation: String,
}

// ============================================================================
// Command/Response
// ============================================================================

#[derive(Debug, Deserialize)]
struct AICommandWrapper {
    #[serde(default)]
    cmd_id: Option<String>,
    #[serde(flatten)]
    command: AICommand,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum AICommand {
    Initialize { models_dir: String },
    ImageToMermaid { image_base64: String },
    AskAnalyst { question: String },
    FeedEvent { event: AIIntegrationEvent },
    SetProactive { enabled: bool },
    RegisterModel { cell_id: String, board_id: String, model_path: String, model_kind: String },
    RegisterModelV2 { cell_id: String, board_id: String, file_id: String, skill_md: String },
    ImportModel { cell_id: String, board_id: String, file_path: String, model_kind: String },
    UnloadModel { cell_id: String },
    InferModel { cell_id: String, input: serde_json::Value },
    ListModels { group_id: String },
    GetCellModel { cell_id: String, board_id: String },
    LensSearch { query: String },
    LensSearchWithContext {
        query: String,
        current_board_id: Option<String>,
        current_workspace_id: Option<String>,
    },
    AgentConfirm {
        request_id: String,
        confirmed: bool,
    },
    LensFeedback {
        request_id: String,
        was_helpful: bool,
        #[serde(default)]
        bullet_feedback: Vec<BulletFeedbackInput>,
        correction: Option<LensCorrectionInput>,
    },
    PlaybookAdd { scope: String, section: String, content: String },
    PlaybookFeedback { bullet_id: String, tag: String },
    PlaybookList { scope: String },
    PlaybookStats { scope: String },
    PlaybookDelete { bullet_id: String },
    PlaybookSeed { scope: String },
}

#[derive(Debug, Serialize)]
struct CommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cmd_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl CommandResponse {
    fn ok() -> Self { Self { success: true, cmd_id: None, error: None, data: None } }
    fn ok_with_data(data: serde_json::Value) -> Self { Self { success: true, cmd_id: None, error: None, data: Some(data) } }
    fn err(msg: impl Into<String>) -> Self { Self { success: false, cmd_id: None, error: Some(msg.into()), data: None } }
    fn with_cmd_id(mut self, cmd_id: Option<String>) -> Self { self.cmd_id = cmd_id; self }
}

// ============================================================================
// AI Bridge
// ============================================================================

#[derive(Debug, Clone)]
struct PendingPlan {
    plan: ActionPlan,
    current_board_id: Option<String>,
    current_workspace_id: Option<String>,
}

pub struct AIBridge {
    db: Arc<Mutex<Connection>>,
    event_tx: tokio::sync::mpsc::UnboundedSender<SwiftEvent>,
    pipeline: RwLock<Option<WhiteboardPipeline>>,
    analyst_runtime: RwLock<Option<Runtime>>,
    sql_runtime: RwLock<Option<Runtime>>,
    analyst_model_name: RwLock<Option<String>>,
    sql_model_name: RwLock<Option<String>>,
    event_buffer: RwLock<VecDeque<AIIntegrationEvent>>,
    insight_queue: RwLock<VecDeque<ProactiveInsight>>,
    proactive_enabled: RwLock<bool>,
    cell_models: RwLock<HashMap<String, CellModel>>,
    initialized: RwLock<bool>,
    models_dir: RwLock<Option<PathBuf>>,
    lens: RwLock<Option<CyanLens>>,
    cyan_db_path: RwLock<Option<PathBuf>>,
    pending_plans: RwLock<HashMap<String, PendingPlan>>,
}

struct CellModel {
    cell_id: String,
    board_id: String,
    model_id: String,
    model_name: String,
    model_kind: String,
    file_id: Option<String>,
    capabilities: Vec<String>,
    skill_md: Option<String>,
    model_path: PathBuf,
    runtime: Option<Runtime>,
}

impl AIBridge {
    pub fn new(db: Arc<Mutex<Connection>>, event_tx: tokio::sync::mpsc::UnboundedSender<SwiftEvent>) -> Self {
        {
            let conn = db.lock().unwrap();
            let _ = xaeroai::registry::init_table(&conn);
            let _ = playbook::init_tables(&conn);
        }
        Self {
            db, event_tx,
            pipeline: RwLock::new(None),
            analyst_runtime: RwLock::new(None),
            sql_runtime: RwLock::new(None),
            analyst_model_name: RwLock::new(None),
            sql_model_name: RwLock::new(None),
            event_buffer: RwLock::new(VecDeque::with_capacity(EVENT_BUFFER_SIZE)),
            insight_queue: RwLock::new(VecDeque::new()),
            proactive_enabled: RwLock::new(false),
            cell_models: RwLock::new(HashMap::new()),
            initialized: RwLock::new(false),
            models_dir: RwLock::new(None),
            lens: RwLock::new(None),
            cyan_db_path: RwLock::new(None),
            pending_plans: RwLock::new(HashMap::new()),
        }
    }

    /// Set the path to cyan.db for lens search
    /// Call this with: data_dir.join("cyan.db") where data_dir is the PathBuf
    pub async fn set_cyan_db_path(&self, path: PathBuf) {
        tracing::info!("🔍 Setting cyan.db path: {:?}", path);
        *self.cyan_db_path.write().await = Some(path);
        *self.lens.write().await = Some(CyanLens::new("cyan-lens"));
    }

    pub fn start_insight_generator(self: &Arc<Self>) {
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(INSIGHT_INTERVAL_SECS));
            loop {
                interval.tick().await;
                if !*bridge.proactive_enabled.read().await || !*bridge.initialized.read().await { continue; }
                if let Some(insight) = bridge.generate_insight().await {
                    bridge.insight_queue.write().await.push_back(insight.clone());
                    let _ = bridge.event_tx.send(SwiftEvent::AIInsight {
                        insight_json: serde_json::to_string(&insight).unwrap_or_default(),
                    });
                }
            }
        });
    }

    pub async fn handle_command(&self, json: &str) -> String {
        tracing::debug!("🔍 AIBridge command: {}", &json[..json.len().min(200)]);
        let (cmd_id, response) = match serde_json::from_str::<AICommandWrapper>(json) {
            Ok(wrapper) => (wrapper.cmd_id, self.dispatch(wrapper.command).await),
            Err(e) => (None, CommandResponse::err(format!("Invalid JSON: {}", e))),
        };
        serde_json::to_string(&response.with_cmd_id(cmd_id)).unwrap_or_else(|_| r#"{"success":false}"#.to_string())
    }

    pub async fn poll_insights(&self) -> Option<ProactiveInsight> {
        self.insight_queue.write().await.pop_front()
    }

    async fn dispatch(&self, cmd: AICommand) -> CommandResponse {
        match cmd {
            AICommand::Initialize { models_dir } => self.cmd_initialize(&models_dir).await,
            AICommand::ImageToMermaid { image_base64 } => self.cmd_image_to_mermaid(&image_base64).await,
            AICommand::AskAnalyst { question } => self.cmd_ask_analyst(&question).await,
            AICommand::FeedEvent { event } => self.cmd_feed_event(event).await,
            AICommand::SetProactive { enabled } => self.cmd_set_proactive(enabled).await,
            AICommand::RegisterModel { cell_id, board_id, model_path, model_kind } =>
                self.cmd_register_model(&cell_id, &board_id, &model_path, &model_kind).await,
            AICommand::RegisterModelV2 { cell_id, board_id, file_id, skill_md } =>
                self.cmd_register_model_v2(&cell_id, &board_id, &file_id, &skill_md).await,
            AICommand::ImportModel { cell_id, board_id, file_path, model_kind } =>
                self.cmd_import_model(&cell_id, &board_id, &file_path, &model_kind).await,
            AICommand::UnloadModel { cell_id } => self.cmd_unload_model(&cell_id).await,
            AICommand::InferModel { cell_id, input } => self.cmd_infer_model(&cell_id, input).await,
            AICommand::ListModels { group_id } => self.cmd_list_models(&group_id).await,
            AICommand::GetCellModel { cell_id, board_id } => self.cmd_get_cell_model(&cell_id, &board_id).await,
            AICommand::LensSearch { query } => self.cmd_lens_search(&query, None, None).await,
            AICommand::LensSearchWithContext { query, current_board_id, current_workspace_id } =>
                self.cmd_lens_search(&query, current_board_id, current_workspace_id).await,
            AICommand::AgentConfirm { request_id, confirmed } =>
                self.cmd_agent_confirm(&request_id, confirmed).await,
            AICommand::LensFeedback { request_id, was_helpful, bullet_feedback, correction } =>
                self.cmd_lens_feedback(&request_id, was_helpful, bullet_feedback, correction).await,
            AICommand::PlaybookAdd { scope, section, content } => self.cmd_playbook_add(&scope, &section, &content).await,
            AICommand::PlaybookFeedback { bullet_id, tag } => self.cmd_playbook_feedback(&bullet_id, &tag).await,
            AICommand::PlaybookList { scope } => self.cmd_playbook_list(&scope).await,
            AICommand::PlaybookStats { scope } => self.cmd_playbook_stats(&scope).await,
            AICommand::PlaybookDelete { bullet_id } => self.cmd_playbook_delete(&bullet_id).await,
            AICommand::PlaybookSeed { scope } => self.cmd_playbook_seed(&scope).await,
        }
    }

    // ========================================================================
    // Initialize
    // ========================================================================

    async fn cmd_initialize(&self, models_dir: &str) -> CommandResponse {
        tracing::info!("🔍 cmd_initialize: {}", models_dir);
        let models_path = PathBuf::from(models_dir);
        let yolo_dir = models_path.join("cyan-sketch");
        let ocr_dir = models_path.join("paddleocr");
        let phi_dir = models_path.join("cyan-lens");
        let sql_dir = models_path.join("cyan-sql");

        for (name, dir) in [("YOLO (cyan-sketch)", &yolo_dir), ("OCR (paddleocr)", &ocr_dir), ("Phi (cyan-lens)", &phi_dir)] {
            if !dir.exists() {
                return CommandResponse::err(format!("{} not found: {:?}", name, dir));
            }
        }

        match WhiteboardPipeline::new(&yolo_dir, &ocr_dir, &phi_dir) {
            Ok(pipeline) => *self.pipeline.write().await = Some(pipeline),
            Err(e) => return CommandResponse::err(format!("Pipeline failed: {}", e)),
        }

        if let Err(e) = self.init_analyst(&phi_dir).await {
            return CommandResponse::err(format!("Analyst init failed: {}", e));
        }

        // Load cyan-sql for lens search (optional - falls back to cyan-lens if missing)
        if sql_dir.exists() {
            match self.init_cyan_sql(&sql_dir).await {
                Ok(_) => tracing::info!("✅ cyan-sql loaded for lens search"),
                Err(e) => tracing::warn!("⚠️ cyan-sql init failed (using cyan-lens): {}", e),
            }
        }

        *self.models_dir.write().await = Some(models_path);
        *self.initialized.write().await = true;
        *self.lens.write().await = Some(CyanLens::new("cyan-lens"));
        tracing::info!("✅ AI Bridge initialized");
        CommandResponse::ok()
    }

    async fn init_analyst(&self, phi_dir: &Path) -> anyhow::Result<()> {
        let skill = Skill::load(phi_dir)?;
        let mut runtime = Runtime::new()?;
        let name = skill.name.clone();
        runtime.load_from_skill(&skill, phi_dir)?;
        *self.analyst_runtime.write().await = Some(runtime);
        *self.analyst_model_name.write().await = Some(name.clone());
        tracing::info!("✅ Analyst initialized: {}", name);
        Ok(())
    }

    async fn init_cyan_sql(&self, sql_dir: &Path) -> anyhow::Result<()> {
        let skill = Skill::load(sql_dir)?;
        let mut runtime = Runtime::new()?;
        let name = skill.name.clone();
        runtime.load_from_skill(&skill, sql_dir)?;
        *self.sql_runtime.write().await = Some(runtime);
        *self.sql_model_name.write().await = Some(name.clone());
        tracing::info!("✅ Cyan SQL initialized: {}", name);
        Ok(())
    }

    // ========================================================================
    // Image → Mermaid
    // ========================================================================

    async fn cmd_image_to_mermaid(&self, image_base64: &str) -> CommandResponse {
        if !*self.initialized.read().await { return CommandResponse::err("Not initialized"); }

        use base64::Engine;
        let image_data = match base64::engine::general_purpose::STANDARD.decode(image_base64) {
            Ok(d) => d,
            Err(e) => return CommandResponse::err(format!("Invalid base64: {}", e)),
        };

        let mut pipeline = self.pipeline.write().await;
        let p = match pipeline.as_mut() {
            Some(p) => p,
            None => return CommandResponse::err("Pipeline not loaded"),
        };

        match p.process(&image_data) {
            Ok(result) => CommandResponse::ok_with_data(serde_json::to_value(MermaidResult {
                success: true,
                mermaid: Some(result.mermaid),
                shapes: Some(result.shapes.iter().map(|s| DetectedShape {
                    id: s.id, shape_type: s.shape_type.clone(), confidence: s.confidence,
                    text: s.text.clone(), x: s.bbox.x, y: s.bbox.y,
                    width: s.bbox.width, height: s.bbox.height,
                }).collect()),
                timing: Some(PipelineTiming {
                    detection_ms: result.timing.detection_ms,
                    ocr_ms: result.timing.ocr_ms,
                    layout_ms: result.timing.layout_ms,
                    generation_ms: result.timing.generation_ms,
                    total_ms: result.timing.total_ms,
                }),
                error: None,
            }).unwrap()),
            Err(e) => CommandResponse::ok_with_data(serde_json::to_value(MermaidResult {
                success: false, mermaid: None, shapes: None, timing: None, error: Some(e.to_string()),
            }).unwrap()),
        }
    }

    // ========================================================================
    // Ask Analyst
    // ========================================================================

    async fn cmd_ask_analyst(&self, question: &str) -> CommandResponse {
        if !*self.initialized.read().await { return CommandResponse::err("Not initialized"); }

        let buffer = self.event_buffer.read().await;
        let context = self.build_context(&buffer);
        let prompt = format!(
            "<|user|>\nYou are a design analyst. Analyze project activity and answer concisely.\n\n\
            ## Activity\n{}\n\n## Question\n{}\n<|end|>\n<|assistant|>\n",
            context, question
        );

        let mut runtime = self.analyst_runtime.write().await;
        let name = self.analyst_model_name.read().await;

        let (rt, model_name) = match (runtime.as_mut(), name.as_ref()) {
            (Some(r), Some(n)) => (r, n.clone()),
            _ => return CommandResponse::err("Analyst not available"),
        };

        let start = std::time::Instant::now();
        match rt.infer_sync(&model_name, InferenceInput::Text { prompt }) {
            Ok(InferenceOutput::Text { content }) => {
                tracing::info!("✅ Inference in {:?}", start.elapsed());
                let citations: Vec<String> = buffer.iter().take(5).map(|e| e.anchor_id.clone()).collect();
                CommandResponse::ok_with_data(serde_json::to_value(AnalysisResult {
                    success: true, response: Some(content), citations: Some(citations), error: None,
                }).unwrap())
            }
            Ok(_) => CommandResponse::err("Unexpected output"),
            Err(e) => CommandResponse::ok_with_data(serde_json::to_value(AnalysisResult {
                success: false, response: None, citations: None, error: Some(e.to_string()),
            }).unwrap()),
        }
    }

    // ========================================================================
    // Events & Insights
    // ========================================================================

    async fn cmd_feed_event(&self, event: AIIntegrationEvent) -> CommandResponse {
        let mut buffer = self.event_buffer.write().await;
        if buffer.len() >= EVENT_BUFFER_SIZE { buffer.pop_front(); }
        buffer.push_back(event);
        CommandResponse::ok()
    }

    async fn cmd_set_proactive(&self, enabled: bool) -> CommandResponse {
        *self.proactive_enabled.write().await = enabled;
        CommandResponse::ok()
    }

    fn build_context(&self, events: &VecDeque<AIIntegrationEvent>) -> String {
        if events.is_empty() { return "(No activity)".to_string(); }
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        events.iter().take(20).map(|e| {
            let days = (now - e.last_mention) / 86400;
            format!("- {} ({}): {} mentions, {}d ago", e.anchor_ref, e.anchor_kind, e.mention_count, days)
        }).collect::<Vec<_>>().join("\n")
    }

    async fn generate_insight(&self) -> Option<ProactiveInsight> {
        let buffer = self.event_buffer.read().await;
        if buffer.is_empty() { return None; }
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let stale: Vec<_> = buffer.iter().filter(|e| (now - e.last_mention) > 14 * 86400 && e.mention_count > 50).collect();
        let worst = stale.iter().max_by_key(|e| e.mention_count)?;
        let days = (now - worst.last_mention) / 86400;
        Some(ProactiveInsight {
            id: uuid::Uuid::new_v4().to_string(),
            message: format!("⚠️ {} has {} mentions but no activity in {} days", worst.anchor_ref, worst.mention_count, days),
            severity: if days > 30 { "critical" } else { "warning" }.to_string(),
            anchor_ids: vec![worst.anchor_id.clone()],
            timestamp: now,
        })
    }

    // ========================================================================
    // CyanLens Search
    // ========================================================================

    async fn cmd_lens_search(
        &self,
        query: &str,
        current_board_id: Option<String>,
        current_workspace_id: Option<String>,
    ) -> CommandResponse {
        let request_id = uuid::Uuid::new_v4().to_string();
        tracing::info!("🔍 lens_search: query={}, id={}", query, &request_id[..8]);

        // Determine which model/scope to use
        let (use_sql_runtime, model_name, scope) = {
            let sql_name = self.sql_model_name.read().await;
            if let Some(n) = sql_name.as_ref() {
                (true, n.clone(), "cyan-sql")
            } else {
                let analyst_name = self.analyst_model_name.read().await;
                match analyst_name.as_ref() {
                    Some(n) => (false, n.clone(), "cyan-lens"),
                    None => return CommandResponse::err("No model loaded for lens search"),
                }
            }
        };
        tracing::info!("🔍 Using {} for lens search (scope: {})", if use_sql_runtime { "cyan-sql" } else { "cyan-lens" }, scope);

        let cyan_db_path_guard = self.cyan_db_path.read().await;
        let cyan_db_path = match cyan_db_path_guard.as_ref() {
            Some(p) => p.clone(),
            None => return CommandResponse::err("Cyan DB path not set"),
        };
        drop(cyan_db_path_guard);

        let cyan_db = match Connection::open(&cyan_db_path) {
            Ok(db) => db,
            Err(e) => return CommandResponse::err(format!("Failed to open cyan.db: {}", e)),
        };

        // Get playbook bullets from the correct scope
        let playbook_bullets = {
            let playbook_db = self.db.lock().unwrap();
            playbook::list_active(&playbook_db, scope).unwrap_or_default()
        };

        // Build prompt
        let prompt = self.build_sql_prompt(query, &playbook_bullets);

        // Run inference
        let start = std::time::Instant::now();
        let generated_text = if use_sql_runtime {
            let mut runtime_guard = self.sql_runtime.write().await;
            let runtime = match runtime_guard.as_mut() {
                Some(r) => r,
                None => return CommandResponse::err("SQL runtime not available"),
            };
            match runtime.infer_sync(&model_name, InferenceInput::Text { prompt }) {
                Ok(InferenceOutput::Text { content }) => content,
                Ok(_) => return CommandResponse::err("Unexpected output type"),
                Err(e) => return CommandResponse::err(format!("Inference failed: {}", e)),
            }
        } else {
            let mut runtime_guard = self.analyst_runtime.write().await;
            let runtime = match runtime_guard.as_mut() {
                Some(r) => r,
                None => return CommandResponse::err("Analyst runtime not available"),
            };
            match runtime.infer_sync(&model_name, InferenceInput::Text { prompt }) {
                Ok(InferenceOutput::Text { content }) => content,
                Ok(_) => return CommandResponse::err("Unexpected output type"),
                Err(e) => return CommandResponse::err(format!("Inference failed: {}", e)),
            }
        };

        // Parse output using executor
        let parsed = match Executor::parse_output(&generated_text) {
            Ok(p) => p,
            Err(e) => return CommandResponse::err(format!("Parse failed: {}", e)),
        };

        match parsed {
            ParsedOutput::Sql(sql) => {
                // Pure SELECT query - execute immediately
                let lens_guard = self.lens.read().await;
                let lens = match lens_guard.as_ref() {
                    Some(l) => l,
                    None => return CommandResponse::err("Lens not initialized"),
                };

                let results = lens.execute_search(&cyan_db, &sql).unwrap_or_default();
                let latency_ms = start.elapsed().as_millis() as u64;

                let results_json: Vec<serde_json::Value> = results.iter().map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "result_type": r.result_type,
                        "snippet": r.snippet,
                        "deep_link": r.deep_link,
                    })
                }).collect();

                CommandResponse::ok_with_data(serde_json::json!({
                    "request_id": request_id,
                    "query": query,
                    "generated_sql": sql,
                    "results": results_json,
                    "playbook_bullets_used": playbook_bullets.iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
                    "latency_ms": latency_ms,
                    "specialist": scope,
                }))
            }

            ParsedOutput::Plan(plan) => {
                // Action plan - check if confirmation needed
                if plan.requires_confirmation {
                    let actions_preview: Vec<String> = plan.actions
                        .iter()
                        .map(|a| format!("{:?}", a))
                        .collect();

                    // Store pending plan
                    self.pending_plans.write().await.insert(
                        request_id.clone(),
                        PendingPlan {
                            plan: plan.clone(),
                            current_board_id,
                            current_workspace_id,
                        },
                    );

                    // Return confirmation request
                    CommandResponse::ok_with_data(serde_json::json!({
                        "type": "confirmation_required",
                        "request_id": request_id,
                        "intent": plan.intent,
                        "confirmation_message": plan.confirmation.unwrap_or_else(|| "Execute this action?".to_string()),
                        "actions_preview": actions_preview,
                        "specialist": scope,
                    }))
                } else {
                    // Execute immediately
                    self.execute_plan(&request_id, plan, current_board_id, current_workspace_id, &cyan_db)
                }
            }
        }
    }

    async fn cmd_agent_confirm(&self, request_id: &str, confirmed: bool) -> CommandResponse {
        let pending = self.pending_plans.write().await.remove(request_id);

        if let Some(PendingPlan { plan, current_board_id, current_workspace_id }) = pending {
            if confirmed {
                let cyan_db_path_guard = self.cyan_db_path.read().await;
                let cyan_db_path = match cyan_db_path_guard.as_ref() {
                    Some(p) => p.clone(),
                    None => return CommandResponse::err("Cyan DB path not set"),
                };
                drop(cyan_db_path_guard);

                let cyan_db = match Connection::open(&cyan_db_path) {
                    Ok(db) => db,
                    Err(e) => return CommandResponse::err(format!("Failed to open cyan.db: {}", e)),
                };

                self.execute_plan(request_id, plan, current_board_id, current_workspace_id, &cyan_db)
            } else {
                CommandResponse::ok_with_data(serde_json::json!({
                    "type": "cancelled",
                    "request_id": request_id,
                    "message": "User cancelled action",
                }))
            }
        } else {
            CommandResponse::err("No pending action found")
        }
    }

    fn execute_plan(
        &self,
        request_id: &str,
        plan: ActionPlan,
        current_board_id: Option<String>,
        current_workspace_id: Option<String>,
        cyan_db: &Connection,
    ) -> CommandResponse {
        let mut executor = Executor::new().with_context(current_board_id, current_workspace_id);

        match executor.execute_plan(cyan_db, &plan) {
            Ok(exec_result) => {
                CommandResponse::ok_with_data(serde_json::json!({
                    "type": "executed",
                    "request_id": request_id,
                    "intent": exec_result.intent,
                    "affected_rows": exec_result.affected_rows,
                    "message": exec_result.message,
                }))
            }
            Err(e) => {
                CommandResponse::err(format!("Execution failed: {}", e))
            }
        }
    }

    fn build_sql_prompt(&self, query: &str, bullets: &[xaeroai::Bullet]) -> String {
        let mut prompt = String::new();

        prompt.push_str("<|system|>\n");
        prompt.push_str("You are CyanLens, an AI assistant for Cyan workspace management.\n\n");

        prompt.push_str("## Schema\n");
        prompt.push_str("- groups(id, name, icon, color, created_at)\n");
        prompt.push_str("- workspaces(id, group_id, name, description, created_at)\n");
        prompt.push_str("- objects(id, workspace_id, type, name, board_mode, archived, created_at, updated_at)\n");
        prompt.push_str("- notebook_cells(id, board_id, cell_type, content, cell_order)\n");
        prompt.push_str("- board_labels(id, board_id, label)\n");
        prompt.push_str("- board_metadata(board_id, starred, template, rating)\n\n");

        prompt.push_str("## Relationships\n");
        prompt.push_str("- objects.workspace_id → workspaces.id\n");
        prompt.push_str("- workspaces.group_id → groups.id\n");
        prompt.push_str("- notebook_cells.board_id → objects.id\n");
        prompt.push_str("- To find boards in a group: JOIN objects → workspaces → groups\n");
        prompt.push_str("- To search notebook content: JOIN notebook_cells ON notebook_cells.board_id = objects.id\n\n");

        prompt.push_str("## Output Format\n");
        prompt.push_str("- For QUERIES: Output SQL in ```sql``` block\n");
        prompt.push_str("- For MUTATIONS (create/update/delete): Output JSON action plan\n");
        prompt.push_str("<|end|>\n");

        prompt.push_str("<|user|>\n");

        if !bullets.is_empty() {
            prompt.push_str("## Learned patterns (IMPORTANT - follow these):\n");
            for bullet in bullets {
                prompt.push_str(&format!("- {}\n", bullet.content));
            }
            prompt.push('\n');
        }

        prompt.push_str(query);
        prompt.push_str("\n<|end|>\n");
        prompt.push_str("<|assistant|>\n");

        prompt
    }

    // ========================================================================
    // Lens Feedback - FIXED: saves to correct scope
    // ========================================================================

    async fn cmd_lens_feedback(
        &self,
        request_id: &str,
        was_helpful: bool,
        bullet_feedback: Vec<BulletFeedbackInput>,
        correction: Option<LensCorrectionInput>,
    ) -> CommandResponse {
        tracing::info!("📝 lens_feedback: id={}, helpful={}", &request_id[..8.min(request_id.len())], was_helpful);

        // Determine scope: use cyan-sql if that model is loaded, otherwise cyan-lens
        let scope = {
            let sql_name = self.sql_model_name.read().await;
            if sql_name.is_some() { "cyan-sql" } else { "cyan-lens" }
        };
        tracing::info!("📝 Saving feedback to scope: {}", scope);

        let db = self.db.lock().unwrap();

        // Record bullet feedback
        for bf in &bullet_feedback {
            let tag = FeedbackTag::from_str(&bf.tag);
            if let Err(e) = playbook::record_feedback(&db, &bf.bullet_id, tag) {
                tracing::warn!("Failed to record feedback for {}: {}", bf.bullet_id, e);
            }
        }

        // Create correction bullet if provided
        let new_bullet_id = if let Some(ref corr) = correction {
            let content = format!(
                "When user asks '{}', the correct approach is: {}",
                corr.wrong_sql.as_deref().unwrap_or(""),
                corr.explanation
            );
            let section = if was_helpful { Section::Strategies } else { Section::Mistakes };
            match playbook::add_with_source(&db, scope, section, &content, "lens_feedback", request_id) {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!("Failed to add correction bullet: {}", e);
                    None
                }
            }
        } else {
            None
        };

        CommandResponse::ok_with_data(serde_json::json!({
            "request_id": request_id,
            "new_bullet_id": new_bullet_id,
            "scope": scope,
        }))
    }

    // ========================================================================
    // Playbook Management
    // ========================================================================

    async fn cmd_playbook_add(&self, scope: &str, section: &str, content: &str) -> CommandResponse {
        let db = self.db.lock().unwrap();
        let section_enum = Section::from_str(section);
        match playbook::add(&db, scope, section_enum, content) {
            Ok(bullet_id) => CommandResponse::ok_with_data(serde_json::json!({
                "bullet_id": bullet_id, "scope": scope, "section": section,
            })),
            Err(e) => CommandResponse::err(format!("Failed to add bullet: {}", e)),
        }
    }

    async fn cmd_playbook_feedback(&self, bullet_id: &str, tag: &str) -> CommandResponse {
        let db = self.db.lock().unwrap();
        let tag_enum = FeedbackTag::from_str(tag);
        match playbook::record_feedback(&db, bullet_id, tag_enum) {
            Ok(()) => CommandResponse::ok_with_data(serde_json::json!({ "bullet_id": bullet_id, "tag": tag })),
            Err(e) => CommandResponse::err(format!("Failed to record feedback: {}", e)),
        }
    }

    async fn cmd_playbook_list(&self, scope: &str) -> CommandResponse {
        let db = self.db.lock().unwrap();
        match playbook::list_all(&db, scope) {
            Ok(bullets) => {
                let items: Vec<serde_json::Value> = bullets.iter().map(|b| serde_json::json!({
                    "id": b.id, "section": b.section.as_str(), "content": b.content,
                    "helpful_count": b.helpful_count, "harmful_count": b.harmful_count, "score": b.score,
                })).collect();
                CommandResponse::ok_with_data(serde_json::json!({ "scope": scope, "bullets": items }))
            }
            Err(e) => CommandResponse::err(format!("Failed to list bullets: {}", e)),
        }
    }

    async fn cmd_playbook_stats(&self, scope: &str) -> CommandResponse {
        let db = self.db.lock().unwrap();
        match playbook::stats(&db, scope) {
            Ok(stats) => CommandResponse::ok_with_data(serde_json::json!({
                "scope": scope, "total_bullets": stats.total_bullets,
                "by_section": stats.by_section, "avg_score": stats.avg_score,
            })),
            Err(e) => CommandResponse::err(format!("Failed to get stats: {}", e)),
        }
    }

    async fn cmd_playbook_delete(&self, bullet_id: &str) -> CommandResponse {
        let db = self.db.lock().unwrap();
        match playbook::delete(&db, bullet_id) {
            Ok(()) => CommandResponse::ok_with_data(serde_json::json!({ "bullet_id": bullet_id, "deleted": true })),
            Err(e) => CommandResponse::err(format!("Failed to delete: {}", e)),
        }
    }

    async fn cmd_playbook_seed(&self, scope: &str) -> CommandResponse {
        let db = self.db.lock().unwrap();

        if let Ok(stats) = playbook::stats(&db, scope) {
            if stats.total_bullets > 0 {
                return CommandResponse::ok_with_data(serde_json::json!({
                    "scope": scope, "seeded": false, "existing_count": stats.total_bullets,
                }));
            }
        }

        let seeds = vec![
            (Section::Strategies, "When user says 'X boards', first check if X matches a group name"),
            (Section::Strategies, "Always use JOINs through workspaces when connecting objects to groups"),
            (Section::Strategies, "For 'recent' queries, ORDER BY last_accessed DESC"),
            (Section::Strategies, "Use DISTINCT when joining with notebook_cells to avoid duplicates"),
            (Section::Strategies, "Limit results to 100 unless user specifies a count"),
            (Section::Strategies, "To find content inside notebooks, JOIN notebook_cells.board_id = objects.id and search notebook_cells.content"),
            (Section::Apis, "Table 'objects' contains boards - there is no 'boards' table"),
            (Section::Apis, "notebook_cells.cell_type can be: 'markdown', 'mermaid', 'code', 'image'"),
            (Section::Apis, "groups table has: id, name, icon, color, created_at"),
            (Section::Apis, "workspaces links groups to objects via group_id"),
            (Section::Apis, "Deep link format: cyan://group/{gid}/workspace/{wid}/board/{bid}"),
            (Section::Mistakes, "Never use 'boards' as table name - use 'objects'"),
            (Section::Mistakes, "Don't search objects.name when user says 'X boards' - X is often a group name"),
            (Section::Mistakes, "Don't forget to join through workspaces - objects don't have direct group_id"),
            (Section::Mistakes, "CRITICAL: When joining notebook_cells to objects, always use notebook_cells.board_id = objects.id (NOT objects.workspace_id)"),
            (Section::Verification, "Always verify SQL is SELECT-only, no INSERT/UPDATE/DELETE/DROP"),
            (Section::Verification, "Check that all table names exist in schema"),
        ];

        let mut added = 0;
        for (section, content) in seeds {
            if playbook::add(&db, scope, section, content).is_ok() { added += 1; }
        }

        tracing::info!("✅ Playbook seeded: {} bullets", added);
        CommandResponse::ok_with_data(serde_json::json!({ "scope": scope, "seeded": true, "added": added }))
    }

    // ========================================================================
    // Model Registry - Legacy
    // ========================================================================

    async fn cmd_register_model(&self, cell_id: &str, board_id: &str, model_path: &str, model_kind: &str) -> CommandResponse {
        let path = PathBuf::from(model_path);
        if !path.exists() { return CommandResponse::err(format!("Model not found: {}", model_path)); }
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let model_id = uuid::Uuid::new_v4().to_string();
        let cell_model = CellModel {
            cell_id: cell_id.to_string(), board_id: board_id.to_string(), model_id: model_id.clone(),
            model_name: name.clone(), model_kind: model_kind.to_string(), file_id: None,
            capabilities: vec![], skill_md: None, model_path: path, runtime: None,
        };
        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);
        CommandResponse::ok_with_data(serde_json::json!({ "model_id": model_id, "name": name, "capabilities": [] }))
    }

    // ========================================================================
    // Model Registry - V3 Import
    // ========================================================================

    async fn cmd_import_model(&self, cell_id: &str, board_id: &str, file_path: &str, model_kind: &str) -> CommandResponse {
        tracing::info!("📦 import_model: cell={}, path={}", &cell_id[..8.min(cell_id.len())], file_path);

        let path = PathBuf::from(file_path);
        if !path.exists() { return CommandResponse::err(format!("File not found: {}", file_path)); }
        let file_name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".to_string());

        let file_bytes = match std::fs::read(&path) {
            Ok(b) => b, Err(e) => return CommandResponse::err(format!("Failed to read: {}", e)),
        };
        let file_hash = blake3::hash(&file_bytes).to_hex().to_string();
        let file_size = file_bytes.len() as u64;

        let files_dir = crate::DATA_DIR.get().cloned().unwrap_or_else(|| PathBuf::from(".")).join("files");
        let _ = std::fs::create_dir_all(&files_dir);
        let local_path = files_dir.join(&file_hash);
        if let Err(e) = std::fs::write(&local_path, &file_bytes) {
            return CommandResponse::err(format!("Failed to store: {}", e));
        }

        let (group_id, workspace_id) = {
            let db = self.db.lock().unwrap();
            let ids: Option<(String, String)> = db.query_row(
                "SELECT w.group_id, o.workspace_id FROM objects o JOIN workspaces w ON o.workspace_id = w.id WHERE o.id = ?1",
                params![board_id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).ok();
            match ids { Some((g, w)) => (g, Some(w)), None => return CommandResponse::err("Board not found") }
        };

        let now = chrono::Utc::now().timestamp();
        let file_id = blake3::hash(format!("file:{}:{}:{}", &group_id, &file_name, now).as_bytes()).to_hex().to_string();

        {
            let db = self.db.lock().unwrap();
            let _ = db.execute(
                "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, board_id, type, name, hash, size, source_peer, local_path, created_at) VALUES (?1, ?2, ?3, ?4, 'file', ?5, ?6, ?7, ?8, ?9, ?10)",
                params![&file_id, &group_id, &workspace_id, board_id, &file_name, &file_hash, file_size as i64, "", local_path.to_string_lossy().to_string(), now],
            );
        }

        let skill_md = Self::generate_skill_md(&file_name, model_kind, Some(&file_name));
        let skill = match Skill::parse(&skill_md) {
            Ok(s) => s, Err(e) => return CommandResponse::err(format!("Skill parse failed: {}", e)),
        };

        let model_id = uuid::Uuid::new_v4().to_string();
        let capabilities: Vec<String> = skill.capabilities.iter().map(|c| format!("{:?}", c).to_lowercase()).collect();

        // FIXED: Handle Option fields properly
        let record = ModelRecord {
            id: model_id.clone(),
            board_id: board_id.to_string(),
            name: skill.name.clone(),
            version: skill.version.clone().unwrap_or_else(|| "0.1.0".to_string()),
            kind: format!("{:?}", skill.kind).to_lowercase(),
            capabilities: capabilities.clone(),
            tags: vec![], // Skill doesn't have tags field
            skill_md: skill_md.clone(),
            model_hash: file_hash,
            file_id: Some(file_id.clone()),
            author: skill.author.clone().unwrap_or_else(|| "local".to_string()),
            created_at: now,
            updated_at: now,
        };

        { let db = self.db.lock().unwrap(); let _ = xaeroai::registry::insert(&db, &record); }

        {
            let db = self.db.lock().unwrap();
            let _ = db.execute("INSERT INTO board_metadata (board_id, contains_model) VALUES (?1, ?2) ON CONFLICT(board_id) DO UPDATE SET contains_model = ?2", params![board_id, &file_name]);
        }

        let cell_model = CellModel {
            cell_id: cell_id.to_string(), board_id: board_id.to_string(), model_id: model_id.clone(),
            model_name: file_name.clone(), model_kind: model_kind.to_string(), file_id: Some(file_id.clone()),
            capabilities: capabilities.clone(), skill_md: Some(skill_md), model_path: local_path, runtime: None,
        };
        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);

        CommandResponse::ok_with_data(serde_json::json!({
            "success": true, "model_id": model_id, "file_id": file_id, "name": file_name,
            "kind": model_kind, "capabilities": capabilities,
        }))
    }

    fn generate_skill_md(name: &str, kind: &str, model_file: Option<&str>) -> String {
        let (capabilities, input_type, output_type) = match kind.to_lowercase().as_str() {
            "gguf" => (vec!["text_generation"], "text", "text"),
            "onnx" => (vec!["inference"], "tensor", "tensor"),
            _ => (vec![], "unknown", "unknown"),
        };
        let caps_json = serde_json::to_string(&capabilities).unwrap_or_else(|_| "[]".to_string());
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let model_line = model_file.map(|f| format!("model: {}\n", f)).unwrap_or_default();
        format!(r#"---
name: {name}
version: 0.1.0
kind: {kind}
{model_line}capabilities: {caps_json}
input:
  type: {input_type}
output:
  type: {output_type}
author: local
created: {now}
---

# {name}

Auto-generated skill.
"#)
    }

    // ========================================================================
    // Model Registry - V2
    // ========================================================================

    async fn cmd_register_model_v2(&self, cell_id: &str, board_id: &str, file_id: &str, skill_md: &str) -> CommandResponse {
        let skill = match Skill::parse(skill_md) {
            Ok(s) => s, Err(e) => return CommandResponse::err(format!("Invalid SKILL.md: {}", e)),
        };
        let file_path = match self.get_file_local_path(file_id) {
            Ok(p) => p, Err(e) => return CommandResponse::err(format!("File not found: {}", e)),
        };
        let model_hash = match std::fs::read(&file_path) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(e) => return CommandResponse::err(format!("Read failed: {}", e)),
        };

        let model_id = uuid::Uuid::new_v4().to_string();
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let capabilities: Vec<String> = skill.capabilities.iter().map(|c| format!("{:?}", c).to_lowercase()).collect();

        // FIXED: Handle Option fields properly
        let record = ModelRecord {
            id: model_id.clone(),
            board_id: board_id.to_string(),
            name: skill.name.clone(),
            version: skill.version.clone().unwrap_or_else(|| "0.1.0".to_string()),
            kind: format!("{:?}", skill.kind).to_lowercase(),
            capabilities: capabilities.clone(),
            tags: vec![], // Skill doesn't have tags field
            skill_md: skill_md.to_string(),
            model_hash,
            file_id: Some(file_id.to_string()),
            author: skill.author.clone().unwrap_or_else(|| "local".to_string()),
            created_at: now,
            updated_at: now,
        };

        { let db = self.db.lock().unwrap(); let _ = xaeroai::registry::insert(&db, &record); }

        let cell_model = CellModel {
            cell_id: cell_id.to_string(), board_id: board_id.to_string(), model_id: model_id.clone(),
            model_name: skill.name.clone(), model_kind: format!("{:?}", skill.kind).to_lowercase(),
            file_id: Some(file_id.to_string()), capabilities: capabilities.clone(),
            skill_md: Some(skill_md.to_string()), model_path: file_path, runtime: None,
        };
        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);

        CommandResponse::ok_with_data(serde_json::json!({
            "model_id": model_id, "name": skill.name, "version": skill.version,
            "kind": format!("{:?}", skill.kind).to_lowercase(), "capabilities": capabilities, "file_id": file_id,
        }))
    }

    fn get_file_local_path(&self, file_id: &str) -> anyhow::Result<PathBuf> {
        let db = self.db.lock().unwrap();
        let local_path: Option<String> = db.query_row(
            "SELECT local_path FROM objects WHERE id = ?1 AND type = 'file'",
            params![file_id], |row| row.get(0),
        ).optional()?;
        match local_path {
            Some(p) => Ok(PathBuf::from(p)),
            None => Err(anyhow::anyhow!("File not found: {}", file_id)),
        }
    }

    async fn cmd_unload_model(&self, cell_id: &str) -> CommandResponse {
        self.cell_models.write().await.remove(cell_id);
        CommandResponse::ok()
    }

    async fn cmd_infer_model(&self, cell_id: &str, input: serde_json::Value) -> CommandResponse {
        let mut models = self.cell_models.write().await;
        let cell_model = match models.get_mut(cell_id) {
            Some(m) => m, None => return CommandResponse::err("Model not registered"),
        };
        if cell_model.runtime.is_none() {
            match self.load_cell_runtime(cell_model).await {
                Ok(rt) => cell_model.runtime = Some(rt),
                Err(e) => return CommandResponse::err(format!("Load failed: {}", e)),
            }
        }
        let runtime = cell_model.runtime.as_mut().unwrap();
        let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        let start = std::time::Instant::now();
        match runtime.infer_sync(&cell_model.model_name, InferenceInput::Text { prompt: prompt.to_string() }) {
            Ok(InferenceOutput::Text { content }) => CommandResponse::ok_with_data(serde_json::to_value(InferenceResult {
                success: true, output: Some(serde_json::Value::String(content)),
                timing_ms: Some(start.elapsed().as_millis() as u64), error: None,
            }).unwrap()),
            Ok(_) => CommandResponse::err("Unexpected output type"),
            Err(e) => CommandResponse::ok_with_data(serde_json::to_value(InferenceResult {
                success: false, output: None, timing_ms: None, error: Some(e.to_string()),
            }).unwrap()),
        }
    }

    async fn load_cell_runtime(&self, cell_model: &CellModel) -> anyhow::Result<Runtime> {
        let mut runtime = Runtime::new()?;
        let model_file = cell_model.model_path.file_name().and_then(|f| f.to_str());
        let skill_md = cell_model.skill_md.clone().unwrap_or_else(|| {
            Self::generate_skill_md(&cell_model.model_name, &cell_model.model_kind, model_file)
        });
        let skill = Skill::parse(&skill_md)?;
        let model_dir = cell_model.model_path.parent().unwrap_or_else(|| Path::new("."));
        runtime.load_from_skill(&skill, model_dir)?;
        Ok(runtime)
    }

    async fn cmd_list_models(&self, _group_id: &str) -> CommandResponse {
        let models = self.cell_models.read().await;
        let summaries: Vec<ModelSummary> = models.values().map(|m| ModelSummary {
            id: m.model_id.clone(), name: m.model_name.clone(), kind: m.model_kind.clone(),
            capabilities: m.capabilities.clone(), board_id: m.board_id.clone(),
            cell_id: m.cell_id.clone(), file_id: m.file_id.clone(),
        }).collect();
        CommandResponse::ok_with_data(serde_json::to_value(summaries).unwrap())
    }

    async fn cmd_get_cell_model(&self, cell_id: &str, board_id: &str) -> CommandResponse {
        {
            let models = self.cell_models.read().await;
            if let Some(m) = models.get(cell_id) {
                return CommandResponse::ok_with_data(serde_json::json!({
                    "model_id": m.model_id, "name": m.model_name, "kind": m.model_kind,
                    "capabilities": m.capabilities, "file_id": m.file_id,
                    "skill_md": m.skill_md, "loaded": m.runtime.is_some(),
                }));
            }
        }

        let models_dir = self.models_dir.read().await.clone().unwrap_or_default();
        let found: Option<ModelRecord> = {
            let db = self.db.lock().unwrap();
            xaeroai::registry::list_by_board(&db, board_id).ok().and_then(|r| r.into_iter().next())
        };

        if let Some(record) = found {
            let model_path = if let Some(ref fid) = record.file_id {
                let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".".to_string());
                let file_path = PathBuf::from(&data_dir).join("files").join(fid);
                if file_path.exists() { file_path } else { PathBuf::from(&models_dir).join(&record.name) }
            } else {
                PathBuf::from(&models_dir).join(&record.name)
            };

            let cell_model = CellModel {
                cell_id: cell_id.to_string(), board_id: board_id.to_string(),
                model_id: record.id.clone(), model_name: record.name.clone(),
                model_kind: record.kind.clone(), file_id: record.file_id.clone(),
                capabilities: record.capabilities.clone(), skill_md: Some(record.skill_md.clone()),
                model_path, runtime: None,
            };

            let response = serde_json::json!({
                "model_id": record.id, "name": record.name, "kind": record.kind,
                "capabilities": record.capabilities, "file_id": record.file_id,
                "skill_md": record.skill_md, "loaded": false,
            });
            self.cell_models.write().await.insert(cell_id.to_string(), cell_model);
            return CommandResponse::ok_with_data(response);
        }

        CommandResponse::ok_with_data(serde_json::json!({
            "model_id": null, "name": null, "kind": null, "capabilities": [],
            "file_id": null, "skill_md": null, "loaded": false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lens_search_parse() {
        let json = r#"{"cmd":"lens_search","query":"find design boards"}"#;
        let wrapper: AICommandWrapper = serde_json::from_str(json).unwrap();
        assert!(matches!(wrapper.command, AICommand::LensSearch { .. }));
    }

    #[test]
    fn test_playbook_add_parse() {
        let json = r#"{"cmd":"playbook_add","scope":"cyan-lens","section":"strategies","content":"test"}"#;
        let wrapper: AICommandWrapper = serde_json::from_str(json).unwrap();
        assert!(matches!(wrapper.command, AICommand::PlaybookAdd { .. }));
    }
}