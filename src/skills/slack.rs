// cyan-backend/src/skills/slack.rs

use anyhow::{anyhow, Result};
use serde_json::json;

use super::*;

pub fn register() -> Vec<SkillDef> {
    vec![
        SkillDef {
            id: "slack_digest".into(),
            name: "Slack Channel Digest".into(),
            description: "Summarize recent Slack channel activity with key discussions, decisions, and action items".into(),
            keywords: vec!["slack".into(), "channel".into(), "messages".into(), "summarize".into(), "digest".into(), "discussions".into(), "what happened".into()],
            tools: vec!["slack_api".into(), "vllm".into()],
            output_type: OutputType::Summary,
            requires_auth: vec!["slack".into()],
            default_timeout: 120,
        },
        SkillDef {
            id: "slack_search".into(),
            name: "Slack Search".into(),
            description: "Search Slack messages for specific topics or mentions".into(),
            keywords: vec!["search slack".into(), "find message".into(), "mentioned".into(), "slack thread".into()],
            tools: vec!["slack_api".into()],
            output_type: OutputType::Json,
            requires_auth: vec!["slack".into()],
            default_timeout: 60,
        },
    ]
}

// ============================================================================
// Slack Digest Executor
// ============================================================================

pub struct SlackDigest;
pub struct SlackSearch;

#[async_trait::async_trait]
impl SkillExecutor for SlackDigest {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        tracing::info!("🔧 [slack_digest] Executing for board {}", &ctx.board_id[..8.min(ctx.board_id.len())]);
        
        // 1. Load Slack nodes from local DB (already imported via integration)
        let scope_id = ctx.scope_id.as_deref()
            .ok_or_else(|| anyhow!("No scope_id for Slack digest"))?;
        
        let messages = load_recent_slack_messages(scope_id, 24)?; // last 24 hours
        
        if messages.is_empty() {
            return Ok(SkillResult {
                skill_id: "slack_digest".into(),
                output_type: OutputType::Summary,
                summary: "No new Slack messages in the last 24 hours.".into(),
                data: json!({"message_count": 0}),
                timecoded_findings: None,
                action_taken: None,
                artifacts: vec![],
            });
        }
        
        // 2. Build context for LLM
        let message_text: Vec<String> = messages.iter()
            .map(|m| format!("[{}] {}: {}", m.channel, m.author, m.content))
            .collect();
        
        let prompt = format!(
            "Analyze these Slack messages from the last 24 hours and provide:\n\
             1. Key discussions (what topics were discussed)\n\
             2. Decisions made (any commitments or agreements)\n\
             3. Action items (tasks assigned or volunteered)\n\
             4. Things that need attention (unanswered questions, blocked items)\n\n\
             Additional context from the user: {}\n\n\
             Messages:\n{}\n\n\
             Provide a structured summary. Be concise but don't miss important details.",
            ctx.cell_content,
            message_text.join("\n")
        );
        
        // 3. Call vLLM
        let response = crate::pipeline::call_vllm_public(&prompt, 800, 0.3).await?;
        
        Ok(SkillResult {
            skill_id: "slack_digest".into(),
            output_type: OutputType::Summary,
            summary: response.clone(),
            data: json!({
                "message_count": messages.len(),
                "channels": messages.iter().map(|m| m.channel.clone()).collect::<std::collections::HashSet<_>>(),
            }),
            timecoded_findings: None,
            action_taken: None,
            artifacts: vec![],
        })
    }
    
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[0].clone())
    }
}

#[async_trait::async_trait]
impl SkillExecutor for SlackSearch {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        let scope_id = ctx.scope_id.as_deref()
            .ok_or_else(|| anyhow!("No scope_id"))?;
        
        let messages = load_recent_slack_messages(scope_id, 168)?; // last week
        
        // Filter by content keywords from cell
        let keywords: Vec<&str> = ctx.cell_content.split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();
        
        let matched: Vec<_> = messages.iter()
            .filter(|m| keywords.iter().any(|kw| m.content.to_lowercase().contains(&kw.to_lowercase())))
            .take(20)
            .collect();
        
        let summary = format!("Found {} messages matching your search.", matched.len());
        
        Ok(SkillResult {
            skill_id: "slack_search".into(),
            output_type: OutputType::Json,
            summary,
            data: json!(matched),
            timecoded_findings: None,
            action_taken: None,
            artifacts: vec![],
        })
    }
    
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[1].clone())
    }
}

// ============================================================================
// Slack Data Loader (reads from local DB, not API)
// ============================================================================

#[derive(Debug, Clone, serde::Serialize)]
struct SlackMessage {
    channel: String,
    author: String,
    content: String,
    ts: u64,
    url: String,
}

fn load_recent_slack_messages(scope_id: &str, hours: u64) -> Result<Vec<SlackMessage>> {
    let conn = crate::storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
    let cutoff = chrono::Utc::now().timestamp() as u64 - (hours * 3600);
    
    // Query nodes table for Slack messages (imported via integration)
    let mut stmt = match conn.prepare(
        "SELECT n.external_id, n.workspace_id, n.content, n.ts, \
                json_extract(n.metadata, '$.author') as author, \
                json_extract(n.metadata, '$.url') as url, \
                json_extract(n.metadata, '$.title') as channel \
         FROM nodes n \
         WHERE n.scope_id = ?1 AND n.kind IN ('slack_message', 'SlackMessage') AND n.ts > ?2 \
         ORDER BY n.ts DESC \
         LIMIT 200"
    ) {
        Ok(s) => s,
        Err(_) => {
            // Fallback: try notebook_cells with slack content
            conn.prepare(
                "SELECT id, board_id, content, 0, 'unknown', '', 'slack' \
                 FROM notebook_cells WHERE content LIKE '%slack%' LIMIT 50"
            )?
        }
    };
    
    let messages = stmt.query_map(rusqlite::params![scope_id, cutoff], |row| {
        Ok(SlackMessage {
            channel: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "general".into()),
            author: row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "unknown".into()),
            content: row.get::<_, String>(2)?,
            ts: row.get::<_, u64>(3)?,
            url: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        })
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default();
    
    Ok(messages)
}
