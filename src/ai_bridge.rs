// src/ai_bridge.rs
//
// Bridges xaeroai crate into Cyan's FFI layer.
//
// Features:
// 1. WhiteboardPipeline for image→mermaid (CyanLensPanel, MermaidCell)
// 2. Runtime for cyan-lens Phi inference (askAnalyst)
// 3. Event buffer for analyst context (fed from ConsoleView)
// 4. Proactive insights generation (shown in ConsoleView)
// 5. Model registry for notebook cells (drag/drop GGUF/ONNX)
//
// Models (HuggingFace):
// - blockxaero/cyan-sketch (YOLO ONNX, 6MB)
// - PaddleOCR rec (ONNX, 7.5MB)
// - blockxaero/cyan-lens (Phi-3 GGUF Q4, 2GB)

use crate::SwiftEvent;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

use xaeroai::{
    InferenceInput, InferenceOutput, ModelRecord, Runtime, Skill, WhiteboardPipeline,
};

// ============================================================================
// Constants
// ============================================================================

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

// ============================================================================
// Command/Response with cmd_id for correlation
// ============================================================================

/// Wrapper that extracts cmd_id before dispatching
#[derive(Debug, Deserialize)]
struct AICommandWrapper {
    /// Optional command ID for request-response correlation
    #[serde(default)]
    cmd_id: Option<String>,
    /// The actual command (flattened)
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
    // Legacy: register by path (for backwards compat)
    RegisterModel { cell_id: String, board_id: String, model_path: String, model_kind: String },
    // V2: register by file_id + skill_md
    RegisterModelV2 { cell_id: String, board_id: String, file_id: String, skill_md: String },
    // V3: Complete import flow - upload, register, update metadata
    ImportModel { cell_id: String, board_id: String, file_path: String, model_kind: String },
    UnloadModel { cell_id: String },
    InferModel { cell_id: String, input: serde_json::Value },
    ListModels { group_id: String },
    GetCellModel { cell_id: String, board_id: String },
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

    fn with_cmd_id(mut self, cmd_id: Option<String>) -> Self {
        self.cmd_id = cmd_id;
        self
    }
}

// ============================================================================
// AI Bridge
// ============================================================================

pub struct AIBridge {
    db: Arc<Mutex<Connection>>,
    event_tx: tokio::sync::mpsc::UnboundedSender<SwiftEvent>,

    // Core models
    pipeline: RwLock<Option<WhiteboardPipeline>>,
    analyst_runtime: RwLock<Option<Runtime>>,
    analyst_model_name: RwLock<Option<String>>,

    // Event buffer for analyst context
    event_buffer: RwLock<VecDeque<AIIntegrationEvent>>,

    // Proactive insights
    insight_queue: RwLock<VecDeque<ProactiveInsight>>,
    proactive_enabled: RwLock<bool>,

    // Notebook model cells
    cell_models: RwLock<HashMap<String, CellModel>>,

    // State
    initialized: RwLock<bool>,
    models_dir: RwLock<Option<PathBuf>>,
}

// Updated CellModel with more metadata for V2 flow
struct CellModel {
    cell_id: String,
    board_id: String,
    model_id: String,           // UUID for this model registration
    model_name: String,
    model_kind: String,
    file_id: Option<String>,    // Reference to file in cyan-backend (V2)
    capabilities: Vec<String>,  // From SKILL.md
    skill_md: Option<String>,   // Stored SKILL.md content for lazy loading
    model_path: PathBuf,
    runtime: Option<Runtime>,
}

impl AIBridge {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        event_tx: tokio::sync::mpsc::UnboundedSender<SwiftEvent>,
    ) -> Self {
        {
            let conn = db.lock().unwrap();
            if let Err(e) = xaeroai::registry::init_table(&conn) {
                tracing::warn!("Failed to init model_registry table: {}", e);
            }
        }
        Self {
            db,
            event_tx,
            pipeline: RwLock::new(None),
            analyst_runtime: RwLock::new(None),
            analyst_model_name: RwLock::new(None),
            event_buffer: RwLock::new(VecDeque::with_capacity(EVENT_BUFFER_SIZE)),
            insight_queue: RwLock::new(VecDeque::new()),
            proactive_enabled: RwLock::new(false),
            cell_models: RwLock::new(HashMap::new()),
            initialized: RwLock::new(false),
            models_dir: RwLock::new(None),
        }
    }

    pub fn start_insight_generator(self: &Arc<Self>) {
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(INSIGHT_INTERVAL_SECS));
            loop {
                interval.tick().await;
                if !*bridge.proactive_enabled.read().await || !*bridge.initialized.read().await {
                    continue;
                }
                if let Some(insight) = bridge.generate_insight().await {
                    bridge.insight_queue.write().await.push_back(insight.clone());
                    let _ = bridge.event_tx.send(SwiftEvent::AIInsight {
                        insight_json: serde_json::to_string(&insight).unwrap_or_default(),
                    });
                }
            }
        });
    }

    /// Handle command with cmd_id correlation
    pub async fn handle_command(&self, json: &str) -> String {
        tracing::debug!("🔍 AIBridge received command: {}", json);

        // Parse wrapper to extract cmd_id
        let (cmd_id, response) = match serde_json::from_str::<AICommandWrapper>(json) {
            Ok(wrapper) => {
                tracing::debug!("🔍 Parsed command: {:?}, cmd_id: {:?}", wrapper.command, wrapper.cmd_id);
                let resp = self.dispatch(wrapper.command).await;
                (wrapper.cmd_id, resp)
            }
            Err(e) => {
                tracing::error!("❌ Failed to parse command: {}", e);
                (None, CommandResponse::err(format!("Invalid JSON: {}", e)))
            }
        };

        // Attach cmd_id to response
        let response_with_id = response.with_cmd_id(cmd_id);

        let result = serde_json::to_string(&response_with_id).unwrap_or_else(|_| {
            r#"{"success":false,"error":"Serialization failed"}"#.to_string()
        });
        tracing::debug!("🔍 AIBridge response: {}", result);
        result
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
            AICommand::RegisterModel { cell_id, board_id, model_path, model_kind } => {
                self.cmd_register_model(&cell_id, &board_id, &model_path, &model_kind).await
            }
            AICommand::RegisterModelV2 { cell_id, board_id, file_id, skill_md } => {
                self.cmd_register_model_v2(&cell_id, &board_id, &file_id, &skill_md).await
            }
            AICommand::ImportModel { cell_id, board_id, file_path, model_kind } => {
                self.cmd_import_model(&cell_id, &board_id, &file_path, &model_kind).await
            }
            AICommand::UnloadModel { cell_id } => self.cmd_unload_model(&cell_id).await,
            AICommand::InferModel { cell_id, input } => self.cmd_infer_model(&cell_id, input).await,
            AICommand::ListModels { group_id } => self.cmd_list_models(&group_id).await,
            AICommand::GetCellModel { cell_id, board_id } => {
                self.cmd_get_cell_model(&cell_id, &board_id).await
            }
        }
    }

    // ========================================================================
    // Initialize
    // ========================================================================

    async fn cmd_initialize(&self, models_dir: &str) -> CommandResponse {
        tracing::info!("🔍 cmd_initialize: models_dir={}", models_dir);
        let models_path = PathBuf::from(models_dir);

        // Directory names match HuggingFace repos and download_models.sh
        let yolo_dir = models_path.join("cyan-sketch");
        let ocr_dir = models_path.join("paddleocr");
        let phi_dir = models_path.join("cyan-lens");

        tracing::info!("🔍 Checking model directories:");
        tracing::info!("   yolo_dir: {:?} exists={}", yolo_dir, yolo_dir.exists());
        tracing::info!("   ocr_dir:  {:?} exists={}", ocr_dir, ocr_dir.exists());
        tracing::info!("   phi_dir:  {:?} exists={}", phi_dir, phi_dir.exists());

        for (name, dir) in [("YOLO (cyan-sketch)", &yolo_dir), ("OCR (paddleocr)", &ocr_dir), ("Phi (cyan-lens)", &phi_dir)] {
            if !dir.exists() {
                tracing::error!("❌ {} not found: {:?}", name, dir);
                return CommandResponse::err(format!("{} not found: {:?}. Run download_models.sh first.", name, dir));
            }
        }

        // List contents of phi_dir for debugging
        tracing::info!("🔍 Contents of phi_dir ({:?}):", phi_dir);
        if let Ok(entries) = std::fs::read_dir(&phi_dir) {
            for entry in entries.flatten() {
                tracing::info!("   - {:?}", entry.file_name());
            }
        }

        // Initialize pipeline
        tracing::info!("🔍 Initializing WhiteboardPipeline...");
        match WhiteboardPipeline::new(&yolo_dir, &ocr_dir, &phi_dir) {
            Ok(pipeline) => {
                *self.pipeline.write().await = Some(pipeline);
                tracing::info!("✅ Pipeline initialized");
            }
            Err(e) => {
                tracing::error!("❌ Pipeline failed: {}", e);
                return CommandResponse::err(format!("Pipeline failed: {}", e));
            }
        }

        // Initialize analyst runtime
        tracing::info!("🔍 Initializing analyst runtime...");
        if let Err(e) = self.init_analyst(&phi_dir).await {
            tracing::error!("❌ Analyst init failed: {}", e);
            return CommandResponse::err(format!("Analyst init failed: {}", e));
        }

        *self.models_dir.write().await = Some(models_path);
        *self.initialized.write().await = true;

        tracing::info!("✅ AI Bridge fully initialized");
        CommandResponse::ok()
    }

    async fn init_analyst(&self, phi_dir: &Path) -> anyhow::Result<()> {
        tracing::info!("🔍 init_analyst: phi_dir={:?}", phi_dir);

        let skill_path = phi_dir.join("SKILL.md");
        tracing::info!("🔍 SKILL.md path: {:?} exists={}", skill_path, skill_path.exists());

        if !skill_path.exists() {
            return Err(anyhow::anyhow!("SKILL.md not found at {:?}", skill_path));
        }

        if let Ok(content) = std::fs::read_to_string(&skill_path) {
            tracing::info!("🔍 SKILL.md content (first 500 chars): {}", &content[..content.len().min(500)]);
        }

        tracing::info!("🔍 Creating Runtime...");
        let mut runtime = Runtime::new()?;
        tracing::info!("✅ Runtime created");

        tracing::info!("🔍 Loading Skill from {:?}...", phi_dir);
        let skill = Skill::load(phi_dir)?;
        tracing::info!("✅ Skill loaded: name={}, version={}, kind={:?}",
            skill.name, skill.version, skill.kind);

        let name = skill.name.clone();

        if let Some(ref model_file) = skill.model_file {
            let model_path = phi_dir.join(model_file);
            tracing::info!("🔍 Model file path: {:?} exists={}", model_path, model_path.exists());
            if model_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&model_path) {
                    tracing::info!("🔍 Model file size: {} bytes ({:.2} MB)",
                        metadata.len(), metadata.len() as f64 / 1_048_576.0);
                }
            }
        }

        tracing::info!("🔍 Loading model from skill...");
        runtime.load_from_skill(&skill, phi_dir)?;
        tracing::info!("✅ Model loaded into runtime");

        *self.analyst_runtime.write().await = Some(runtime);
        *self.analyst_model_name.write().await = Some(name.clone());

        tracing::info!("✅ Analyst initialized with model: {}", name);
        Ok(())
    }

    // ========================================================================
    // Image → Mermaid
    // ========================================================================

    async fn cmd_image_to_mermaid(&self, image_base64: &str) -> CommandResponse {
        if !*self.initialized.read().await {
            return CommandResponse::err("Not initialized");
        }

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
            Ok(result) => {
                CommandResponse::ok_with_data(serde_json::to_value(MermaidResult {
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
                }).unwrap())
            }
            Err(e) => CommandResponse::ok_with_data(serde_json::to_value(MermaidResult {
                success: false, mermaid: None, shapes: None, timing: None,
                error: Some(e.to_string()),
            }).unwrap()),
        }
    }

    // ========================================================================
    // Ask Analyst
    // ========================================================================

    async fn cmd_ask_analyst(&self, question: &str) -> CommandResponse {
        tracing::info!("🔍 cmd_ask_analyst: question={}", question);

        if !*self.initialized.read().await {
            tracing::warn!("❌ Not initialized");
            return CommandResponse::err("Not initialized");
        }

        let buffer = self.event_buffer.read().await;
        let context = self.build_context(&buffer);
        tracing::debug!("🔍 Context built: {} chars", context.len());

        let prompt = format!(
            "<|user|>\nYou are a design analyst. Analyze project activity and answer concisely.\n\n\
            ## Activity\n{}\n\n## Question\n{}\n<|end|>\n<|assistant|>\n",
            context, question
        );

        let mut runtime = self.analyst_runtime.write().await;
        let name = self.analyst_model_name.read().await;

        let (rt, model_name) = match (runtime.as_mut(), name.as_ref()) {
            (Some(r), Some(n)) => (r, n.clone()),
            (None, _) => return CommandResponse::err("Analyst not available (runtime is None)"),
            (_, None) => return CommandResponse::err("Analyst not available (model name is None)"),
        };

        let start = std::time::Instant::now();

        match rt.infer_sync(&model_name, InferenceInput::Text { prompt }) {
            Ok(InferenceOutput::Text { content }) => {
                let elapsed = start.elapsed();
                tracing::info!("✅ Inference complete in {:?}", elapsed);
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
    // Event Buffer & Insights
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

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64).unwrap_or(0);

        events.iter().take(20).map(|e| {
            let days = (now - e.last_mention) / 86400;
            format!("- {} ({}): {} mentions, {}d ago", e.anchor_ref, e.anchor_kind, e.mention_count, days)
        }).collect::<Vec<_>>().join("\n")
    }

    async fn generate_insight(&self) -> Option<ProactiveInsight> {
        let buffer = self.event_buffer.read().await;
        if buffer.is_empty() { return None; }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64).unwrap_or(0);

        let stale: Vec<_> = buffer.iter()
            .filter(|e| (now - e.last_mention) > 14 * 86400 && e.mention_count > 50)
            .collect();

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
    // Model Cell Registry - Legacy (by path)
    // ========================================================================

    async fn cmd_register_model(&self, cell_id: &str, board_id: &str, model_path: &str, model_kind: &str) -> CommandResponse {
        let path = PathBuf::from(model_path);
        if !path.exists() {
            return CommandResponse::err(format!("Model not found: {}", model_path));
        }

        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let model_id = uuid::Uuid::new_v4().to_string();

        let cell_model = CellModel {
            cell_id: cell_id.to_string(),
            board_id: board_id.to_string(),
            model_id: model_id.clone(),
            model_name: name.clone(),
            model_kind: model_kind.to_string(),
            file_id: None,
            capabilities: vec![],
            skill_md: None,  // Legacy doesn't have skill_md
            model_path: path,
            runtime: None,
        };

        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);

        tracing::info!("✅ Model registered (legacy): {} ({})", name, model_kind);
        CommandResponse::ok_with_data(serde_json::json!({
            "model_id": model_id,
            "name": name,
            "capabilities": [],
        }))
    }

    // ========================================================================
    // Model Cell Registry - V3 (complete import flow)
    // ========================================================================

    /// Complete model import: upload file → generate skill → register → update metadata
    async fn cmd_import_model(
        &self,
        cell_id: &str,
        board_id: &str,
        file_path: &str,
        model_kind: &str,
    ) -> CommandResponse {
        tracing::info!("📦 import_model: cell={}, board={}, path={}",
            &cell_id[..8.min(cell_id.len())],
            &board_id[..8.min(board_id.len())],
            file_path);

        let path = PathBuf::from(file_path);
        if !path.exists() {
            return CommandResponse::err(format!("File not found: {}", file_path));
        }

        let file_name = path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "model".to_string());

        // Step 1: Read file and compute hash
        let file_bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return CommandResponse::err(format!("Failed to read file: {}", e)),
        };
        let file_hash = blake3::hash(&file_bytes).to_hex().to_string();
        let file_size = file_bytes.len() as u64;

        // Step 2: Store file locally
        let files_dir = crate::DATA_DIR
            .get()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("files");

        if let Err(e) = std::fs::create_dir_all(&files_dir) {
            return CommandResponse::err(format!("Failed to create files dir: {}", e));
        }

        let local_path = files_dir.join(&file_hash);
        if let Err(e) = std::fs::write(&local_path, &file_bytes) {
            return CommandResponse::err(format!("Failed to store file: {}", e));
        }

        // Step 3: Get group_id from board
        let (group_id, workspace_id) = {
            let db = self.db.lock().unwrap();
            let ids: Option<(String, String)> = db.query_row(
                "SELECT w.group_id, o.workspace_id FROM objects o
                 JOIN workspaces w ON o.workspace_id = w.id
                 WHERE o.id = ?1",
                rusqlite::params![board_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).ok();
            match ids {
                Some((g, w)) => (g, Some(w)),
                None => return CommandResponse::err("Could not find board"),
            }
        };

        // Step 4: Generate file_id and insert into objects table
        let now = chrono::Utc::now().timestamp();
        let file_id = blake3::hash(format!("file:{}:{}:{}", &group_id, &file_name, now).as_bytes())
            .to_hex()
            .to_string();

        {
            let db = self.db.lock().unwrap();
            if let Err(e) = db.execute(
                "INSERT OR REPLACE INTO objects (id, group_id, workspace_id, board_id, type, name, hash, size, source_peer, local_path, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'file', ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    &file_id,
                    &group_id,
                    &workspace_id,
                    board_id,
                    &file_name,
                    &file_hash,
                    file_size as i64,
                    "", // source_peer - local upload
                    local_path.to_string_lossy().to_string(),
                    now
                ],
            ) {
                return CommandResponse::err(format!("Failed to insert file record: {}", e));
            }
        }
        tracing::info!("✅ File uploaded: {}", &file_id[..8]);

        // Step 5: Generate SKILL.md
        let skill_md = Self::generate_skill_md(&file_name, model_kind, Some(&file_name));

        // Step 6: Parse skill and create model record
        let skill = match Skill::parse(&skill_md) {
            Ok(s) => s,
            Err(e) => return CommandResponse::err(format!("Failed to parse skill: {}", e)),
        };

        let model_id = uuid::Uuid::new_v4().to_string();
        let capabilities: Vec<String> = skill.capabilities.iter()
            .map(|c| format!("{:?}", c).to_lowercase())
            .collect();

        let record = ModelRecord {
            id: model_id.clone(),
            board_id: board_id.to_string(),
            name: skill.name.clone(),
            version: skill.version.clone(),
            kind: format!("{:?}", skill.kind).to_lowercase(),
            capabilities: capabilities.clone(),
            tags: skill.tags.clone(),
            skill_md: skill_md.clone(),
            model_hash: file_hash.clone(),
            file_id: Some(file_id.clone()),
            author: skill.author.clone(),
            created_at: now,
            updated_at: now,
        };

        // Step 7: Insert into model_registry
        {
            let db = self.db.lock().unwrap();
            if let Err(e) = xaeroai::registry::insert(&db, &record) {
                tracing::error!("❌ Failed to insert model record: {}", e);
                return CommandResponse::err(format!("Registry insert failed: {}", e));
            }
        }
        tracing::info!("✅ Model registered: {}", &model_id[..8]);

        // Step 8: Update board_metadata
        {
            let db = self.db.lock().unwrap();

            // Set contains_model
            let _ = db.execute(
                "INSERT INTO board_metadata (board_id, contains_model) VALUES (?1, ?2)
                 ON CONFLICT(board_id) DO UPDATE SET contains_model = ?2",
                rusqlite::params![board_id, &file_name],
            );

            // Add labels
            let existing_labels: Option<String> = db.query_row(
                "SELECT labels FROM board_metadata WHERE board_id = ?1",
                rusqlite::params![board_id],
                |row| row.get(0),
            ).ok().flatten();

            let mut labels: Vec<String> = existing_labels
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

            // Add "model" and kind labels
            for label in ["model", model_kind] {
                if !labels.contains(&label.to_string()) {
                    labels.push(label.to_string());
                }
            }

            let labels_json = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".to_string());
            let _ = db.execute(
                "INSERT INTO board_metadata (board_id, labels) VALUES (?1, ?2)
                 ON CONFLICT(board_id) DO UPDATE SET labels = ?2",
                rusqlite::params![board_id, &labels_json],
            );
        }
        tracing::info!("✅ Board metadata updated");

        // Step 9: Store in memory for lazy loading
        let cell_model = CellModel {
            cell_id: cell_id.to_string(),
            board_id: board_id.to_string(),
            model_id: model_id.clone(),
            model_name: file_name.clone(),
            model_kind: model_kind.to_string(),
            file_id: Some(file_id.clone()),
            capabilities: capabilities.clone(),
            skill_md: Some(skill_md.clone()),  // Store for lazy loading
            model_path: local_path,
            runtime: None,
        };

        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);

        // Step 10: Broadcast FileAvailable event
        // (This would go through network_tx if we had access to it)
        // For now, the file is stored locally and can be synced via board sync

        tracing::info!("✅ Import complete: {} -> {}", file_name, &model_id[..8]);

        CommandResponse::ok_with_data(serde_json::json!({
            "success": true,
            "model_id": model_id,
            "file_id": file_id,
            "name": file_name,
            "kind": model_kind,
            "capabilities": capabilities,
        }))
    }

    /// Generate minimal SKILL.md for a model file
    fn generate_skill_md(name: &str, kind: &str, model_file: Option<&str>) -> String {
        let (capabilities, input_type, output_type) = match kind.to_lowercase().as_str() {
            "gguf" => (vec!["text_generation"], "text", "text"),
            "onnx" => (vec!["inference"], "tensor", "tensor"),
            _ => (vec![], "unknown", "unknown"),
        };

        let caps_json = serde_json::to_string(&capabilities).unwrap_or_else(|_| "[]".to_string());
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let model_line = model_file
            .map(|f| format!("model: {}\n", f))
            .unwrap_or_default();

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

Auto-generated skill for imported model file.
"#)
    }

    // ========================================================================
    // Model Cell Registry - V2 (by file_id + skill_md)
    // ========================================================================

    async fn cmd_register_model_v2(
        &self,
        cell_id: &str,
        board_id: &str,
        file_id: &str,
        skill_md: &str,
    ) -> CommandResponse {
        tracing::info!("📦 register_model_v2: cell={}, board={}, file={}",
            &cell_id[..8.min(cell_id.len())],
            &board_id[..8.min(board_id.len())],
            &file_id[..8.min(file_id.len())]);

        // 1. Parse SKILL.md
        let skill = match Skill::parse(skill_md) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("❌ Invalid SKILL.md: {}", e);
                return CommandResponse::err(format!("Invalid SKILL.md: {}", e));
            }
        };
        tracing::info!("✅ Parsed skill: name={}, kind={:?}", skill.name, skill.kind);

        // 2. Get local file path from cyan-backend
        let file_path = match self.get_file_local_path(file_id) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("❌ Could not get file path: {}", e);
                return CommandResponse::err(format!("File not found: {}", e));
            }
        };
        tracing::info!("📁 File path: {:?}", file_path);

        // 3. Compute content hash
        let model_hash = match std::fs::read(&file_path) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(e) => {
                tracing::error!("❌ Could not read file: {}", e);
                return CommandResponse::err(format!("Could not read file: {}", e));
            }
        };

        // 4. Generate model ID
        let model_id = uuid::Uuid::new_v4().to_string();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // 5. Create model record for xaeroai registry
        let capabilities: Vec<String> = skill.capabilities.iter()
            .map(|c| format!("{:?}", c).to_lowercase())
            .collect();

        let record = ModelRecord {
            id: model_id.clone(),
            board_id: board_id.to_string(),
            name: skill.name.clone(),
            version: skill.version.clone(),
            kind: format!("{:?}", skill.kind).to_lowercase(),
            capabilities: capabilities.clone(),
            tags: skill.tags.clone(),
            skill_md: skill_md.to_string(),
            model_hash,
            file_id: Some(file_id.to_string()),
            author: skill.author.clone(),
            created_at: now,
            updated_at: now,
        };

        // 6. Insert into xaeroai model_registry
        {
            let db = self.db.lock().unwrap();
            if let Err(e) = xaeroai::registry::insert(&db, &record) {
                tracing::error!("❌ Failed to insert model record: {}", e);
                return CommandResponse::err(format!("Registry insert failed: {}", e));
            }
        }
        tracing::info!("✅ Model record inserted: {}", model_id);

        // 7. Store in memory for lazy loading
        let cell_model = CellModel {
            cell_id: cell_id.to_string(),
            board_id: board_id.to_string(),
            model_id: model_id.clone(),
            model_name: skill.name.clone(),
            model_kind: format!("{:?}", skill.kind).to_lowercase(),
            file_id: Some(file_id.to_string()),
            capabilities: capabilities.clone(),
            skill_md: Some(skill_md.to_string()),  // Store for lazy loading
            model_path: file_path,
            runtime: None,
        };

        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);
        tracing::info!("✅ Cell model stored in memory");

        // 8. Return success with model info
        CommandResponse::ok_with_data(serde_json::json!({
            "model_id": model_id,
            "name": skill.name,
            "version": skill.version,
            "kind": format!("{:?}", skill.kind).to_lowercase(),
            "capabilities": capabilities,
            "file_id": file_id,
        }))
    }

    /// Get local file path from cyan-backend by file_id
    fn get_file_local_path(&self, file_id: &str) -> anyhow::Result<PathBuf> {
        let db = self.db.lock().unwrap();

        let local_path: Option<String> = db.query_row(
            "SELECT local_path FROM objects WHERE id = ?1 AND type = 'file'",
            rusqlite::params![file_id],
            |row| row.get(0),
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
            Some(m) => m,
            None => return CommandResponse::err("Model not registered"),
        };

        // Lazy load runtime
        if cell_model.runtime.is_none() {
            match self.load_cell_runtime(cell_model).await {
                Ok(rt) => cell_model.runtime = Some(rt),
                Err(e) => return CommandResponse::err(format!("Failed to load: {}", e)),
            }
        }

        let runtime = cell_model.runtime.as_mut().unwrap();
        let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");

        let start = std::time::Instant::now();

        match runtime.infer_sync(&cell_model.model_name, InferenceInput::Text { prompt: prompt.to_string() }) {
            Ok(InferenceOutput::Text { content }) => {
                CommandResponse::ok_with_data(serde_json::to_value(InferenceResult {
                    success: true,
                    output: Some(serde_json::Value::String(content)),
                    timing_ms: Some(start.elapsed().as_millis() as u64),
                    error: None,
                }).unwrap())
            }
            Ok(_) => CommandResponse::err("Unexpected output type"),
            Err(e) => CommandResponse::ok_with_data(serde_json::to_value(InferenceResult {
                success: false, output: None, timing_ms: None, error: Some(e.to_string()),
            }).unwrap()),
        }
    }

    async fn load_cell_runtime(&self, cell_model: &CellModel) -> anyhow::Result<Runtime> {
        tracing::info!("🔍 load_cell_runtime: model={}, path={:?}",
        cell_model.model_name, cell_model.model_path);

        let mut runtime = Runtime::new()?;

        // Always include model file in skill_md
        let model_file = cell_model.model_path.file_name()
            .and_then(|f| f.to_str());

        // Use stored skill_md if available, but ensure model file is set
        let skill_md = if let Some(stored_md) = &cell_model.skill_md {
            // Check if stored skill_md already has model: line
            if stored_md.contains("model:") {
                stored_md.clone()
            } else {
                // Regenerate with model file
                tracing::info!("🔍 Stored SKILL.md missing model file, regenerating...");
                Self::generate_skill_md(&cell_model.model_name, &cell_model.model_kind, model_file)
            }
        } else {
            tracing::info!("🔍 No SKILL.md available, generating one...");
            Self::generate_skill_md(&cell_model.model_name, &cell_model.model_kind, model_file)
        };

        let skill = Skill::parse(&skill_md)?;
        let model_dir = cell_model.model_path.parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        runtime.load_from_skill(&skill, model_dir)?;

        tracing::info!("✅ Cell model loaded: {}", cell_model.model_name);
        Ok(runtime)
    }

    async fn cmd_list_models(&self, _group_id: &str) -> CommandResponse {
        let models = self.cell_models.read().await;
        let summaries: Vec<ModelSummary> = models.values().map(|m| ModelSummary {
            id: m.model_id.clone(),
            name: m.model_name.clone(),
            kind: m.model_kind.clone(),
            capabilities: m.capabilities.clone(),
            board_id: m.board_id.clone(),
            cell_id: m.cell_id.clone(),
            file_id: m.file_id.clone(),
        }).collect();

        CommandResponse::ok_with_data(serde_json::to_value(summaries).unwrap())
    }

    /// Get model metadata for a cell (for UI state restoration)
    async fn cmd_get_cell_model(&self, cell_id: &str, board_id: &str) -> CommandResponse {
        // First check in-memory cache
        {
            let models = self.cell_models.read().await;
            if let Some(cell_model) = models.get(cell_id) {
                return CommandResponse::ok_with_data(serde_json::json!({
                "model_id": cell_model.model_id,
                "name": cell_model.model_name,
                "kind": cell_model.model_kind,
                "capabilities": cell_model.capabilities,
                "file_id": cell_model.file_id,
                "skill_md": cell_model.skill_md,  // Include for UI
                "loaded": cell_model.runtime.is_some(),
            }));
            }
        }

        // Get models_dir before sync block
        let models_dir = self.models_dir.read().await.clone().unwrap_or_default();

        // Not in memory - check database by board_id
        let found: Option<ModelRecord> = {
            let db = self.db.lock().unwrap();
            xaeroai::registry::list_by_board(&db, board_id)
                .ok()
                .and_then(|records| records.into_iter().next())
        };

        if let Some(record) = found {
            // Build model path from file_id or name
            let model_path = if let Some(ref fid) = record.file_id {
                let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| ".".to_string());
                let file_path = PathBuf::from(&data_dir).join("files").join(fid);
                if file_path.exists() {
                    file_path
                } else {
                    PathBuf::from(&models_dir).join(&record.name)
                }
            } else {
                PathBuf::from(&models_dir).join(&record.name)
            };

            // Register in cell_models for inference
            let cell_model = CellModel {
                cell_id: cell_id.to_string(),
                board_id: board_id.to_string(),
                model_id: record.id.clone(),
                model_name: record.name.clone(),
                model_kind: record.kind.clone(),
                file_id: record.file_id.clone(),
                capabilities: record.capabilities.clone(),
                skill_md: Some(record.skill_md.clone()),  // Store skill_md!
                model_path,
                runtime: None,
            };

            let response = serde_json::json!({
            "model_id": record.id,
            "name": record.name,
            "kind": record.kind,
            "capabilities": record.capabilities,
            "file_id": record.file_id,
            "skill_md": record.skill_md,  // Include for UI label
            "loaded": false,
        });

            self.cell_models.write().await.insert(cell_id.to_string(), cell_model);

            return CommandResponse::ok_with_data(response);
        }

        // Not found anywhere
        CommandResponse::ok_with_data(serde_json::json!({
        "model_id": null,
        "name": null,
        "kind": null,
        "capabilities": [],
        "file_id": null,
        "skill_md": null,
        "loaded": false,
    }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parse() {
        let json = r#"{"cmd":"set_proactive","enabled":true}"#;
        let wrapper: AICommandWrapper = serde_json::from_str(json).unwrap();
        assert!(matches!(wrapper.command, AICommand::SetProactive { enabled: true }));
        assert!(wrapper.cmd_id.is_none());
    }

    #[test]
    fn test_command_parse_with_cmd_id() {
        let json = r#"{"cmd_id":"abc-123","cmd":"set_proactive","enabled":true}"#;
        let wrapper: AICommandWrapper = serde_json::from_str(json).unwrap();
        assert!(matches!(wrapper.command, AICommand::SetProactive { enabled: true }));
        assert_eq!(wrapper.cmd_id, Some("abc-123".to_string()));
    }

    #[test]
    fn test_register_model_v2_parse() {
        let json = r#"{"cmd":"register_model_v2","cell_id":"cell-123","board_id":"board-456","file_id":"file-789","skill_md":"---\nname: test\n---"}"#;
        let wrapper: AICommandWrapper = serde_json::from_str(json).unwrap();
        assert!(matches!(wrapper.command, AICommand::RegisterModelV2 { .. }));
    }

    #[test]
    fn test_get_cell_model_parse() {
        let json = r#"{"cmd":"get_cell_model","cell_id":"cell-123","board_id":"board-456"}"#;
        let wrapper: AICommandWrapper = serde_json::from_str(json).unwrap();
        assert!(matches!(wrapper.command, AICommand::GetCellModel { .. }));
    }

    #[test]
    fn test_response_with_cmd_id() {
        let resp = CommandResponse::ok_with_data(serde_json::json!({"test": true}))
            .with_cmd_id(Some("xyz-789".to_string()));

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"cmd_id\":\"xyz-789\""));
        assert!(json.contains("\"success\":true"));
    }
}