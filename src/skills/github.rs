// cyan-backend/src/skills/github.rs

use anyhow::{anyhow, Result};
use serde_json::json;

use super::*;

pub fn register() -> Vec<SkillDef> {
    vec![
        SkillDef {
            id: "github_pr_review".into(),
            name: "GitHub PR Review Status".into(),
            description: "Scan open PRs, flag stale reviews, identify blockers".into(),
            keywords: vec!["github".into(), "pr".into(), "pull request".into(), "review".into(), "blocked".into(), "stale".into(), "open prs".into()],
            tools: vec!["github_api".into(), "vllm".into()],
            output_type: OutputType::Summary,
            requires_auth: vec!["github".into()],
            default_timeout: 120,
        },
        SkillDef {
            id: "github_commit_summary".into(),
            name: "GitHub Commit Summary".into(),
            description: "Summarize recent commits and code changes".into(),
            keywords: vec!["commits".into(), "code changes".into(), "what was pushed".into(), "merged".into()],
            tools: vec!["github_api".into(), "vllm".into()],
            output_type: OutputType::Summary,
            requires_auth: vec!["github".into()],
            default_timeout: 120,
        },
    ]
}

pub struct PrReview;
pub struct CommitSummary;

#[async_trait::async_trait]
impl SkillExecutor for PrReview {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        tracing::info!("🔧 [github_pr_review] Executing");
        
        // Load PR data from local nodes DB
        let scope_id = ctx.scope_id.as_deref().unwrap_or("");
        let prs = load_github_prs(scope_id)?;
        
        if prs.is_empty() {
            return Ok(SkillResult {
                skill_id: "github_pr_review".into(),
                output_type: OutputType::Summary,
                summary: "No open PRs found.".into(),
                data: json!({"pr_count": 0}),
                timecoded_findings: None,
                action_taken: None,
                artifacts: vec![],
            });
        }
        
        // Build LLM prompt
        let pr_text: Vec<String> = prs.iter()
            .map(|pr| format!(
                "PR #{}: {} (by {}, opened {}, reviewers: {}, status: {})",
                pr.number, pr.title, pr.author, pr.created_at, pr.reviewers, pr.status
            ))
            .collect();
        
        let prompt = format!(
            "Analyze these GitHub PRs and identify:\n\
             1. PRs waiting for review for more than 2 days\n\
             2. PRs that appear blocked (review requested but not started)\n\
             3. PRs that are ready to merge\n\
             4. Any patterns (one reviewer overloaded, etc.)\n\n\
             Context: {}\n\n\
             PRs:\n{}\n\n\
             Provide actionable summary. Flag urgency.",
            ctx.cell_content,
            pr_text.join("\n")
        );
        
        let response = crate::pipeline::call_vllm_public(&prompt, 600, 0.3).await?;
        
        let blocked: Vec<_> = prs.iter().filter(|pr| pr.status == "review_pending" || pr.days_open > 2).collect();
        
        Ok(SkillResult {
            skill_id: "github_pr_review".into(),
            output_type: OutputType::Summary,
            summary: response,
            data: json!({
                "total_prs": prs.len(),
                "blocked_count": blocked.len(),
                "blocked_prs": blocked.iter().map(|pr| json!({
                    "number": pr.number,
                    "title": pr.title,
                    "author": pr.author,
                    "days_open": pr.days_open,
                    "reviewers": pr.reviewers,
                })).collect::<Vec<_>>(),
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
impl SkillExecutor for CommitSummary {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        let scope_id = ctx.scope_id.as_deref().unwrap_or("");
        let commits = load_github_commits(scope_id, 24)?;
        
        let commit_text: Vec<String> = commits.iter()
            .map(|c| format!("{}: {} (by {})", &c.sha[..7], c.message, c.author))
            .collect();
        
        let prompt = format!(
            "Summarize these recent commits:\n{}\n\nHighlight: major changes, patterns, areas of activity.",
            commit_text.join("\n")
        );
        
        let response = crate::pipeline::call_vllm_public(&prompt, 400, 0.3).await?;
        
        Ok(SkillResult {
            skill_id: "github_commit_summary".into(),
            output_type: OutputType::Summary,
            summary: response,
            data: json!({"commit_count": commits.len()}),
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
// GitHub Data Loaders
// ============================================================================

#[derive(Debug, Clone, serde::Serialize)]
struct GithubPr {
    number: u32,
    title: String,
    author: String,
    created_at: String,
    reviewers: String,
    status: String,
    days_open: u32,
    url: String,
}

#[derive(Debug, Clone)]
struct GithubCommit {
    sha: String,
    message: String,
    author: String,
    ts: u64,
}

fn load_github_prs(scope_id: &str) -> Result<Vec<GithubPr>> {
    let conn = crate::storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
    
    let mut stmt = conn.prepare(
        "SELECT n.external_id, n.content, n.ts, \
                json_extract(n.metadata, '$.author') as author, \
                json_extract(n.metadata, '$.url') as url, \
                json_extract(n.metadata, '$.title') as title, \
                json_extract(n.metadata, '$.status') as status \
         FROM nodes n \
         WHERE n.scope_id = ?1 AND n.kind IN ('github_pr', 'GitHubPR') \
         ORDER BY n.ts DESC LIMIT 50"
    )?;
    
    let now = chrono::Utc::now().timestamp() as u64;
    
    let prs = stmt.query_map(rusqlite::params![scope_id], |row| {
        let ts: u64 = row.get(2)?;
        let days_open = ((now - ts) / 86400) as u32;
        
        Ok(GithubPr {
            number: row.get::<_, String>(0)?.parse().unwrap_or(0),
            title: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            author: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            created_at: chrono::DateTime::from_timestamp(ts as i64, 0)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            reviewers: "pending".into(), // TODO: extract from metadata
            status: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "open".into()),
            days_open,
            url: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        })
    })?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(prs)
}

fn load_github_commits(scope_id: &str, hours: u64) -> Result<Vec<GithubCommit>> {
    let conn = crate::storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
    let cutoff = chrono::Utc::now().timestamp() as u64 - (hours * 3600);
    
    let mut stmt = conn.prepare(
        "SELECT n.external_id, n.content, n.ts, \
                json_extract(n.metadata, '$.author') as author \
         FROM nodes n \
         WHERE n.scope_id = ?1 AND n.kind IN ('github_commit', 'GitHubCommit') AND n.ts > ?2 \
         ORDER BY n.ts DESC LIMIT 100"
    )?;
    
    let commits = stmt.query_map(rusqlite::params![scope_id, cutoff], |row| {
        Ok(GithubCommit {
            sha: row.get(0)?,
            message: row.get(1)?,
            ts: row.get(2)?,
            author: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        })
    })?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(commits)
}
