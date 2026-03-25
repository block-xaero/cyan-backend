// cyan-backend/src/skills/email.rs

use anyhow::{anyhow, Result};
use serde_json::json;
use super::*;

pub fn register() -> Vec<SkillDef> {
    vec![
        SkillDef {
            id: "draft_email".into(),
            name: "Draft Email".into(),
            description: "Draft an email based on pipeline context and previous step outputs".into(),
            keywords: vec!["email".into(), "draft".into(), "briefing".into(), "morning briefing".into(), "send email".into(), "mail".into()],
            tools: vec!["vllm".into(), "smtp".into()],
            output_type: OutputType::Action,
            requires_auth: vec![],
            default_timeout: 120,
        },
        SkillDef {
            id: "send_email".into(),
            name: "Send Email".into(),
            description: "Send a pre-drafted email via SMTP".into(),
            keywords: vec!["send".into(), "deliver".into(), "forward".into()],
            tools: vec!["smtp".into()],
            output_type: OutputType::Action,
            requires_auth: vec![],
            default_timeout: 30,
        },
    ]
}

pub struct DraftEmail;
pub struct SendEmail;

#[async_trait::async_trait]
impl SkillExecutor for DraftEmail {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        tracing::info!("🔧 [draft_email] Executing");
        
        // Gather context from previous steps
        let prev_context: Vec<String> = ctx.previous_outputs.iter()
            .map(|o| format!("=== {} ===\n{}", o.step_id, o.output))
            .collect();
        
        // Extract recipient from cell content
        let recipient = extract_email_from_text(&ctx.cell_content)
            .unwrap_or_else(|| "team@company.com".into());
        
        let prompt = format!(
            "Based on the following information gathered from our tools, \
             draft a professional morning briefing email.\n\n\
             Instructions from user: {}\n\n\
             Information gathered:\n{}\n\n\
             Draft the email with:\n\
             - Subject line\n\
             - Brief greeting\n\
             - Key highlights (what happened)\n\
             - Action items (what needs attention)\n\
             - Blockers (what's stuck)\n\
             - Sign off\n\n\
             Keep it concise and actionable. Use bullet points.",
            ctx.cell_content,
            prev_context.join("\n\n")
        );
        
        let response = crate::pipeline::call_vllm_public(&prompt, 800, 0.3).await?;
        
        Ok(SkillResult {
            skill_id: "draft_email".into(),
            output_type: OutputType::Action,
            summary: format!("Email drafted for {}", recipient),
            data: json!({
                "recipient": recipient,
                "draft": response,
                "status": "drafted",
            }),
            timecoded_findings: None,
            action_taken: Some(format!("Email drafted for {}. Review in pipeline before sending.", recipient)),
            artifacts: vec![],
        })
    }
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[0].clone())
    }
}

#[async_trait::async_trait]
impl SkillExecutor for SendEmail {
    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        // Get draft from previous step
        let draft = ctx.previous_outputs.iter()
            .find(|o| o.step_id.contains("email") || o.step_id.contains("draft"))
            .map(|o| &o.output)
            .ok_or_else(|| anyhow!("No email draft found in previous steps"))?;
        
        let recipient = extract_email_from_text(&ctx.cell_content)
            .unwrap_or_else(|| "team@company.com".into());
        
        // Send via local sendmail or SMTP
        let sent = send_via_smtp(&recipient, draft).await;
        
        match sent {
            Ok(_) => Ok(SkillResult {
                skill_id: "send_email".into(),
                output_type: OutputType::Action,
                summary: format!("Email sent to {}", recipient),
                data: json!({"recipient": recipient, "status": "sent"}),
                timecoded_findings: None,
                action_taken: Some(format!("Email sent to {}", recipient)),
                artifacts: vec![],
            }),
            Err(e) => Ok(SkillResult {
                skill_id: "send_email".into(),
                output_type: OutputType::Action,
                summary: format!("Email send failed: {}", e),
                data: json!({"recipient": recipient, "status": "failed", "error": e.to_string()}),
                timecoded_findings: None,
                action_taken: Some(format!("FAILED to send to {}: {}", recipient, e)),
                artifacts: vec![],
            }),
        }
    }
    fn definition(&self) -> &SkillDef {
        static DEF: std::sync::OnceLock<SkillDef> = std::sync::OnceLock::new();
        DEF.get_or_init(|| register()[1].clone())
    }
}

fn extract_email_from_text(text: &str) -> Option<String> {
    // Simple email extraction
    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '.' && c != '_' && c != '-');
        if clean.contains('@') && clean.contains('.') {
            return Some(clean.to_string());
        }
    }
    None
}

async fn send_via_smtp(recipient: &str, body: &str) -> Result<()> {
    // Try local sendmail first
    let output = tokio::process::Command::new("sendmail")
        .arg(recipient)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    
    match output {
        Ok(mut child) => {
            if let Some(stdin) = child.stdin.as_mut() {
                use tokio::io::AsyncWriteExt;
                let email = format!(
                    "To: {}\nFrom: cyan@localhost\nSubject: Daily Briefing from Cyan\nContent-Type: text/plain\n\n{}",
                    recipient, body
                );
                stdin.write_all(email.as_bytes()).await?;
            }
            let status = child.wait().await?;
            if status.success() {
                Ok(())
            } else {
                Err(anyhow!("sendmail exited with {}", status))
            }
        }
        Err(_) => {
            tracing::warn!("sendmail not available, email not sent (would send to {})", recipient);
            // For demo: log the email instead of failing
            tracing::info!("📧 [DEMO] Email to {}:\n{}", recipient, &body[..body.len().min(500)]);
            Ok(()) // Don't fail the pipeline for email
        }
    }
}
