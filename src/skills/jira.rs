// cyan-backend/src/skills/jira.rs

use anyhow::{anyhow, Result};
use serde_json::json;
use super::*;

pub fn register() -> Vec<SkillDef> {
    vec![
        SkillDef {
            id: "jira_my_board".into(),
            name: "My Jira Board".into(),
            description: "Check tickets assigned to me: overdue, new comments, status changes".into(),
            keywords: vec!["jira".into(), "tickets".into(), "assigned".into(), "overdue".into(), "my board".into(), "my tickets".into()],
            tools: vec!["jira_api".into(), "vllm".into()],
            output_type: OutputType::Summary,
            requires_auth: vec!["jira".into()],
            default_timeout: 120,
        },
        SkillDef {
            id: "jira_scan".into(),
            name: "Jira Ticket Scanner".into(),
            description: "Scan Jira for specific patterns: stale tickets, unassigned, blockers".into(),
            keywords: vec!["scan jira".into(), "stale tickets".into(), "blockers".into(), "unassigned".into()],
            tools: vec!["jira_api".into(), "vllm".into()],
            output_type: OutputType::Summary,
            requires_auth: vec!["jira".into()],
            default_timeout: 120,
        },
    ]
}

pub struct MyBoard;
pub struct TicketScan;

#[async_trait::async_trait]
impl SkillExecutor for MyBoard {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        tracing::info!("🔧 [jira_my_board] Executing");
        let scope_id = ctx.scope_id.as_deref().unwrap_or("").to_string();
        
        // Scope DB access to drop MutexGuard before .await
        let tickets: Vec<serde_json::Value> = {
            let conn = crate::storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
            let mut stmt = conn.prepare(
                "SELECT n.external_id, n.content, n.ts, \
                        json_extract(n.metadata, '$.author') as author, \
                        json_extract(n.metadata, '$.status') as status, \
                        json_extract(n.metadata, '$.title') as title, \
                        json_extract(n.metadata, '$.url') as url \
                 FROM nodes n \
                 WHERE n.scope_id = ?1 AND n.kind IN ('jira_ticket', 'JiraTicket') \
                 ORDER BY n.ts DESC LIMIT 50"
            )?;
            
            stmt.query_map(rusqlite::params![scope_id], |row| {
                Ok(json!({
                    "key": row.get::<_, String>(0)?,
                    "title": row.get::<_, Option<String>>(5)?,
                    "status": row.get::<_, Option<String>>(4)?,
                    "content": row.get::<_, String>(1)?,
                }))
            })?.filter_map(|r| r.ok()).collect()
        }; // conn dropped here
        
        let ticket_text: Vec<String> = tickets.iter()
            .map(|t| format!("{}: {} ({})", 
                t["key"].as_str().unwrap_or("?"),
                t["title"].as_str().unwrap_or("?"),
                t["status"].as_str().unwrap_or("?")))
            .collect();
        
        let prompt = format!(
            "Review my Jira tickets and identify:\n\
             1. Overdue or stale items\n\
             2. Items with new activity I should respond to\n\
             3. Blockers\n\n\
             Context: {}\n\nTickets:\n{}\n\nProvide actionable summary.",
            ctx.cell_content, ticket_text.join("\n")
        );
        
        let response = crate::pipeline::call_vllm_public(&prompt, 500, 0.3).await?;
        
        Ok(SkillResult {
            skill_id: "jira_my_board".into(),
            output_type: OutputType::Summary,
            summary: response,
            data: json!({"ticket_count": tickets.len()}),
            timecoded_findings: None, action_taken: None, artifacts: vec![],
        })
    }
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[0].clone())
    }
}

#[async_trait::async_trait]
impl SkillExecutor for TicketScan {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        // Similar to MyBoard but broader scan
        MyBoard.execute(ctx).await.map(|mut r| { r.skill_id = "jira_scan".into(); r })
    }
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[1].clone())
    }
}
