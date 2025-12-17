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
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;

use xaeroai::{
    InferenceInput, InferenceOutput, Runtime, Skill, WhiteboardPipeline,
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
// Command/Response
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum AICommand {
    Initialize { models_dir: String },
    ImageToMermaid { image_base64: String },
    AskAnalyst { question: String },
    FeedEvent { event: AIIntegrationEvent },
    SetProactive { enabled: bool },
    RegisterModel { cell_id: String, board_id: String, model_path: String, model_kind: String },
    UnloadModel { cell_id: String },
    InferModel { cell_id: String, input: serde_json::Value },
    ListModels { group_id: String },
}

#[derive(Debug, Serialize)]
struct CommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl CommandResponse {
    fn ok() -> Self { Self { success: true, error: None, data: None } }
    fn ok_with_data(data: serde_json::Value) -> Self { Self { success: true, error: None, data: Some(data) } }
    fn err(msg: impl Into<String>) -> Self { Self { success: false, error: Some(msg.into()), data: None } }
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

struct CellModel {
    cell_id: String,
    board_id: String,
    model_name: String,
    model_kind: String,
    model_path: PathBuf,
    runtime: Option<Runtime>,
}

impl AIBridge {
    pub fn new(
        db: Arc<Mutex<Connection>>,
        event_tx: tokio::sync::mpsc::UnboundedSender<SwiftEvent>,
    ) -> Self {
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

    pub async fn handle_command(&self, json: &str) -> String {
        tracing::debug!("🔍 AIBridge received command: {}", json);
        let response = match serde_json::from_str::<AICommand>(json) {
            Ok(cmd) => {
                tracing::debug!("🔍 Parsed command: {:?}", cmd);
                self.dispatch(cmd).await
            }
            Err(e) => CommandResponse::err(format!("Invalid JSON: {}", e)),
        };
        let result = serde_json::to_string(&response).unwrap_or_else(|_| {
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
            AICommand::UnloadModel { cell_id } => self.cmd_unload_model(&cell_id).await,
            AICommand::InferModel { cell_id, input } => self.cmd_infer_model(&cell_id, input).await,
            AICommand::ListModels { group_id } => self.cmd_list_models(&group_id).await,
        }
    }

    // ========================================================================
    // Initialize
    // ========================================================================

    async fn cmd_initialize(&self, models_dir: &str) -> CommandResponse {
        tracing::info!("🔍 cmd_initialize: models_dir={}", models_dir);
        let models_path = PathBuf::from(models_dir);

        // Directory names match HuggingFace repos and download_models.sh
        let yolo_dir = models_path.join("cyan-sketch");    // blockxaero/cyan-sketch
        let ocr_dir = models_path.join("paddleocr");       // PaddleOCR recognition
        let phi_dir = models_path.join("cyan-lens");       // blockxaero/cyan-lens

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

        // Initialize analyst runtime - NOW FATAL ON FAILURE
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

        // Check for SKILL.md
        let skill_path = phi_dir.join("SKILL.md");
        tracing::info!("🔍 SKILL.md path: {:?} exists={}", skill_path, skill_path.exists());

        if !skill_path.exists() {
            return Err(anyhow::anyhow!("SKILL.md not found at {:?}", skill_path));
        }

        // Read SKILL.md content for debugging
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
        tracing::info!("🔍 Skill model_file={:?}", skill.model_file);

        let name = skill.name.clone();

        // Check if model file exists
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
        tracing::debug!("🔍 Prompt length: {} chars", prompt.len());

        let mut runtime = self.analyst_runtime.write().await;
        let name = self.analyst_model_name.read().await;

        tracing::info!("🔍 Runtime present: {}, Model name: {:?}",
            runtime.is_some(), name.as_ref());

        let (rt, model_name) = match (runtime.as_mut(), name.as_ref()) {
            (Some(r), Some(n)) => (r, n.clone()),
            (None, _) => {
                tracing::error!("❌ Analyst runtime is None");
                return CommandResponse::err("Analyst not available (runtime is None)");
            }
            (_, None) => {
                tracing::error!("❌ Analyst model name is None");
                return CommandResponse::err("Analyst not available (model name is None)");
            }
        };

        tracing::info!("🔍 Running inference with model: {}", model_name);
        let start = std::time::Instant::now();

        match rt.infer_sync(&model_name, InferenceInput::Text { prompt }) {
            Ok(InferenceOutput::Text { content }) => {
                let elapsed = start.elapsed();
                tracing::info!("✅ Inference complete in {:?}, output length: {} chars",
                    elapsed, content.len());
                let citations: Vec<String> = buffer.iter().take(5).map(|e| e.anchor_id.clone()).collect();
                CommandResponse::ok_with_data(serde_json::to_value(AnalysisResult {
                    success: true, response: Some(content), citations: Some(citations), error: None,
                }).unwrap())
            }
            Ok(other) => {
                tracing::error!("❌ Unexpected output type: {:?}", other);
                CommandResponse::err("Unexpected output")
            }
            Err(e) => {
                tracing::error!("❌ Inference error: {}", e);
                CommandResponse::ok_with_data(serde_json::to_value(AnalysisResult {
                    success: false, response: None, citations: None, error: Some(e.to_string()),
                }).unwrap())
            }
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

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64).unwrap_or(0);

        events.iter().take(20).map(|e| {
            let days = (now - e.last_mention) / 86400;
            format!("- {} ({}): {} mentions, {}d ago", e.anchor_ref, e.anchor_kind, e.mention_count, days)
        }).collect::<Vec<_>>().join("\n")
    }

    async fn generate_insight(&self) -> Option<ProactiveInsight> {
        let buffer = self.event_buffer.read().await;
        if buffer.is_empty() { return None; }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
    // Model Cell Registry
    // ========================================================================

    async fn cmd_register_model(&self, cell_id: &str, board_id: &str, model_path: &str, model_kind: &str) -> CommandResponse {
        let path = PathBuf::from(model_path);
        if !path.exists() {
            return CommandResponse::err(format!("Model not found: {}", model_path));
        }

        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

        let cell_model = CellModel {
            cell_id: cell_id.to_string(),
            board_id: board_id.to_string(),
            model_name: name.clone(),
            model_kind: model_kind.to_string(),
            model_path: path,
            runtime: None, // Lazy load on first inference
        };

        self.cell_models.write().await.insert(cell_id.to_string(), cell_model);

        tracing::info!("✅ Model registered: {} ({})", name, model_kind);
        CommandResponse::ok()
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
        tracing::info!("🔍 load_cell_runtime: model_path={:?}", cell_model.model_path);

        let mut runtime = Runtime::new()?;

        // Try to find SKILL.md in same directory
        let skill_path = cell_model.model_path.parent().map(|p| p.join("SKILL.md"));
        tracing::info!("🔍 Looking for SKILL.md at {:?}", skill_path);

        if let Some(sp) = skill_path.filter(|p| p.exists()) {
            tracing::info!("🔍 Found SKILL.md, loading skill...");
            let skill = Skill::load(sp.parent().unwrap())?;
            tracing::info!("🔍 Loading model from skill: {}", skill.name);
            runtime.load_from_skill(&skill, sp.parent().unwrap())?;
            tracing::info!("✅ Cell model loaded");
        } else {
            tracing::error!("❌ SKILL.md not found");
            return Err(anyhow::anyhow!("SKILL.md required for model loading"));
        }

        Ok(runtime)
    }

    async fn cmd_list_models(&self, _group_id: &str) -> CommandResponse {
        let models = self.cell_models.read().await;
        let summaries: Vec<ModelSummary> = models.values().map(|m| ModelSummary {
            id: m.cell_id.clone(),
            name: m.model_name.clone(),
            kind: m.model_kind.clone(),
            capabilities: vec![],
            board_id: m.board_id.clone(),
            cell_id: m.cell_id.clone(),
        }).collect();

        CommandResponse::ok_with_data(serde_json::to_value(summaries).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parse() {
        let json = r#"{"cmd":"set_proactive","enabled":true}"#;
        let cmd: AICommand = serde_json::from_str(json).unwrap();
        assert!(matches!(cmd, AICommand::SetProactive { enabled: true }));
    }
}