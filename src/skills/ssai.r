// cyan-backend/src/skills/ssai.rs
//
// Server-Side Ad Insertion skill

use anyhow::{anyhow, Result};
use serde_json::json;
use crate::skills::{SkillExecutor, SkillDef, SkillContext, SkillResult, OutputType, Finding};

pub fn register() -> Vec<SkillDef> {
    vec![
        SkillDef {
            id: "ssai_break_detection".into(),
            name: "Ad Break Detection".into(),
            description: "Detect optimal ad insertion points using scene analysis and AI".into(),
            keywords: vec![
                "ssai".into(), "ad".into(), "insertion".into(), "break".into(),
                "advertising".into(), "mid-roll".into(), "commercial".into(),
                "monetization".into(), "ad break".into(),
            ],
            tools: vec!["ffprobe".into(), "vllm".into()],
            output_type: OutputType::TimecodedNotes,
            requires_auth: vec![],
            default_timeout: 300,
        },
        SkillDef {
            id: "ssai_compliance".into(),
            name: "Ad Compliance Check".into(),
            description: "Verify ad insertion plan meets territory broadcast regulations".into(),
            keywords: vec![
                "ad compliance".into(), "trai".into(), "ad frequency".into(),
                "advertising regulation".into(),
            ],
            tools: vec!["vllm".into()],
            output_type: OutputType::Summary,
            requires_auth: vec![],
            default_timeout: 300,
        },
    ]
}

// ============================================================================
// Ad Break Detection
// ============================================================================

pub struct AdBreakDetection;

impl SkillExecutor for AdBreakDetection {
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[0].clone())
    }

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        tracing::info!("📺 [SSAI] Detecting ad break points");

        let video_uri = ctx.video_uri.as_deref()
            .or_else(|| {
                for output in &ctx.previous_outputs {
                    for word in output.output.split_whitespace() {
                        if (word.starts_with("http") || word.starts_with("s3://"))
                            && (word.contains(".mp4") || word.contains(".mov") || word.contains(".mkv"))
                        {
                            return Some(word);
                        }
                    }
                }
                None
            })
            .ok_or_else(|| anyhow!("No video URI found for SSAI analysis"))?;

        // Run ffprobe for duration
        let probe_output = tokio::process::Command::new("ffprobe")
            .args(["-v", "quiet", "-print_format", "json", "-show_format", video_uri])
            .output()
            .await
            .map_err(|e| anyhow!("ffprobe failed: {}", e))?;

        let probe_json = String::from_utf8_lossy(&probe_output.stdout);
        let duration = extract_duration(&probe_json);
        tracing::info!("📺 [SSAI] Video duration: {:.1}s", duration);

        // Generate candidate break points (interval-based for reliability)
        let scene_changes = generate_interval_points(duration);
        tracing::info!("📺 [SSAI] {} candidate break points", scene_changes.len());

        // Get transcript context from previous steps
        let transcript_context = ctx.previous_outputs.iter()
            .find(|o| o.step_id.contains("transcript"))
            .map(|o| o.output.clone())
            .unwrap_or_else(|| "No transcript available".to_string());

        let scene_list = scene_changes.iter()
            .map(|t| format!("  - {}", format_tc(*t)))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            r#"You are an ad insertion specialist for broadcast television.

VIDEO DURATION: {:.0} seconds ({})
CANDIDATE BREAK POINTS:
{}

TRANSCRIPT CONTEXT:
{}

REGULATIONS:
- India TRAI: Max 12 min ads/hour, min 15 min between breaks
- No mid-sentence breaks, no breaks during dramatic moments

Select 3-5 optimal ad insertion points. Respond ONLY as a JSON array:
[
  {{"timecode_seconds": 0.0, "type": "pre-roll", "duration": "15s", "reason": "Before content", "content_match": "general"}}
]"#,
            duration, format_tc(duration), scene_list,
            &transcript_context[..transcript_context.len().min(400)],
        );

        let ai_response = crate::pipeline::call_vllm_public(&prompt, 600, 0.3).await?;
        let findings = parse_ssai_response(&ai_response, duration);

        let summary = format!(
            "SSAI: {} ad break points in {} video. TRAI: {}",
            findings.len(), format_tc(duration), check_trai(&findings, duration),
        );

        Ok(SkillResult {
            skill_id: "ssai_break_detection".into(),
            summary,
            output_type: OutputType::TimecodedNotes,
            data: json!({"break_points": findings.len(), "duration": duration}),
            timecoded_findings: Some(findings),
            action_taken: None,
            artifacts: vec![],
        })
    }
}

// ============================================================================
// Ad Compliance Check
// ============================================================================

pub struct AdComplianceCheck;

impl SkillExecutor for AdComplianceCheck {
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[1].clone())
    }

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        tracing::info!("📺 [SSAI] Running ad compliance check");

        let ssai_output = ctx.previous_outputs.iter()
            .find(|o| o.step_id.contains("ssai") || o.step_id.contains("ad_break"))
            .map(|o| o.output.clone())
            .unwrap_or_else(|| ctx.cell_content.clone());

        let prompt = format!(
            r#"Review this ad insertion plan against broadcast regulations.

AD PLAN:
{}

REGULATIONS:
India TRAI: Max 12 min ads/hour, min 15 min gap. No tobacco/alcohol during children's content.
Singapore IMDA: Max 14 min/hour free-to-air. No gambling/tobacco ads.
UAE NMC: Max 15 min/hour. No ads during prayer times. No alcohol/pork ads.

For each territory provide: COMPLIANT/WARNING/VIOLATION, issue, and fix.
Format as a compliance matrix."#,
            &ssai_output[..ssai_output.len().min(800)],
        );

        let ai_response = crate::pipeline::call_vllm_public(&prompt, 500, 0.2).await?;

        Ok(SkillResult {
            skill_id: "ssai_compliance".into(),
            summary: ai_response,
            output_type: OutputType::Summary,
            data: json!({"territories": ["India", "Singapore", "UAE"]}),
            timecoded_findings: None,
            action_taken: None,
            artifacts: vec![],
        })
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn extract_duration(probe_json: &str) -> f64 {
    serde_json::from_str::<serde_json::Value>(probe_json).ok()
        .and_then(|v| v["format"]["duration"].as_str()?.parse::<f64>().ok())
        .unwrap_or(600.0)
}

fn generate_interval_points(duration: f64) -> Vec<f64> {
    let interval = if duration > 3600.0 { 900.0 }
        else if duration > 1800.0 { 600.0 }
        else if duration > 600.0 { 300.0 }
        else { 180.0 };
    let mut points = vec![0.0]; // pre-roll
    let mut t = interval;
    while t < duration - 30.0 { points.push(t); t += interval; }
    points
}

fn format_tc(s: f64) -> String {
    let t = s as u64;
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn parse_ssai_response(response: &str, duration: f64) -> Vec<Finding> {
    let json_str = response.find('[')
        .and_then(|s| response.rfind(']').map(|e| &response[s..=e]));
    
    if let Some(js) = json_str {
        if let Ok(breaks) = serde_json::from_str::<Vec<serde_json::Value>>(js) {
            return breaks.iter().map(|b| {
                let tc = b["timecode_seconds"].as_f64().unwrap_or(0.0);
                let ad_type = b["type"].as_str().unwrap_or("mid-roll");
                let dur = b["duration"].as_str().unwrap_or("30s");
                let reason = b["reason"].as_str().unwrap_or("Scene transition");
                let content = b["content_match"].as_str().unwrap_or("general");
                Finding {
                    timecode_seconds: tc,
                    content: format!("📺 {} — {} ({}) ", ad_type, dur, reason),
                    finding_type: "ad_break".to_string(),
                    severity: "info".to_string(),
                    source: "ssai".to_string(),
                    suggested_action: Some(format!("Insert {} {}. Category: {}", dur, ad_type, content)),
                }
            }).collect();
        }
    }
    // Fallback
    generate_interval_points(duration).iter().enumerate().map(|(i, t)| {
        Finding {
            timecode_seconds: *t,
            content: format!("📺 mid-roll — 30s (interval #{})", i + 1),
            finding_type: "ad_break".to_string(),
            severity: "info".to_string(),
            source: "ssai".to_string(),
            suggested_action: Some("Insert 30s mid-roll".to_string()),
        }
    }).collect()
}

fn check_trai(findings: &[Finding], duration: f64) -> &'static str {
    let total_ad = findings.len() as f64 * 30.0;
    let max = (duration / 3600.0) * 720.0;
    if total_ad > max { "⚠️ exceeds limit" } else { "✅ compliant" }
}
