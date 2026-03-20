// cyan-backend/src/import_orchestrator.rs
//
// Import orchestrator for CyanLens /import command.
// Fetches from Jira, Confluence, Google Docs and creates boards with markdown content.
// Uses command_tx for board/cell creation → ensures gossip sync to peers.
//
// Flow:
//   1. Read integration config from SQLite (domain, email, projects/spaces)
//   2. Read token from config
//   3. Fetch data from external API
//   4. Convert to markdown
//   5. Create boards via CommandMsg (syncs to peers)
//   6. Report progress via SwiftEvent

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use cyan_backend_integrations::clients::jira::JiraClient;
use cyan_backend_integrations::clients::confluence::ConfluenceClient;
use cyan_backend_integrations::clients::googledocs::GoogleDocsClient;

use crate::models::commands::CommandMsg;
use crate::models::events::SwiftEvent;
use crate::storage;

// ============================================================================
// Import Result (returned to UI)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub source: String,
    pub boards_created: usize,
    pub items_imported: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportableProject {
    pub key: String,
    pub name: String,
    pub item_count: Option<usize>,
}

// ============================================================================
// Config Loading
// ============================================================================

#[derive(Debug, Clone)]
struct IntegrationConfig {
    domain: String,
    email: String,
    token: String,
    projects: Vec<String>,   // Jira projects or Confluence spaces
    document_ids: Vec<String>, // Google Docs IDs
}

fn load_integration_config(scope_id: &str, integration_type: &str) -> Result<IntegrationConfig> {
    let conn = storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
    
    let row: (String, String) = conn.query_row(
        "SELECT config_json, integration_type FROM integration_bindings WHERE scope_id = ?1 AND integration_type = ?2 LIMIT 1",
        rusqlite::params![scope_id, integration_type],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|_| anyhow!("No {} integration configured. Right-click a workspace to add one.", integration_type))?;
    
    let config: serde_json::Value = serde_json::from_str(&row.0)
        .map_err(|_| anyhow!("Invalid integration config"))?;
    
    Ok(IntegrationConfig {
        domain: config["domain"].as_str().unwrap_or("").to_string(),
        email: config["email"].as_str().unwrap_or("").to_string(),
        token: config["token"].as_str()
            .or_else(|| config["api_token"].as_str())
            .or_else(|| config["access_token"].as_str())
            .unwrap_or("").to_string(),
        projects: parse_string_or_array(&config["projects"])
            .or_else(|| parse_string_or_array(&config["spaces"]))
            .unwrap_or_default(),
        document_ids: parse_string_or_array(&config["document_ids"])
            .unwrap_or_default(),
    })
}

/// Parse a JSON value that is either a proper array or a string-encoded array like "[\"AD\"]"
fn parse_string_or_array(val: &serde_json::Value) -> Option<Vec<String>> {
    // Try as a proper JSON array first
    if let Some(arr) = val.as_array() {
        return Some(arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());
    }
    // Try as a string-encoded JSON array
    if let Some(s) = val.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Vec<String>>(s) {
            return Some(parsed);
        }
        // Single value, not an array
        if !s.is_empty() {
            return Some(vec![s.to_string()]);
        }
    }
    None
}

/// Find the first workspace in a group that has this integration configured
fn find_integration_scope(integration_type: &str) -> Result<(String, String)> {
    let conn = storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
    
    let (scope_id, scope_type): (String, String) = conn.query_row(
        "SELECT scope_id, scope_type FROM integration_bindings WHERE integration_type = ?1 LIMIT 1",
        rusqlite::params![integration_type],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|_| anyhow!("No {} integration configured. Right-click a workspace to add one.", integration_type))?;
    
    Ok((scope_id, scope_type))
}

// ============================================================================
// Jira Import
// ============================================================================

pub async fn list_jira_projects(
    token: &str,
    _event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<Vec<ImportableProject>> {
    let (scope_id, _) = find_integration_scope("jira")?;
    let config = load_integration_config(&scope_id, "jira")?;
    let client = JiraClient::new(config.domain, config.email, token.to_string());
    
    // Use configured projects from config_json (API get_projects may lack permissions)
    // The projects field can be either a JSON array or a string-encoded array
    let project_keys: Vec<String> = if !config.projects.is_empty() {
        config.projects.clone()
    } else {
        // Fallback: try API
        match client.get_projects().await {
            Ok(projects) => projects.iter()
                .filter_map(|p| p["key"].as_str().map(String::from))
                .collect(),
            Err(_) => vec![],
        }
    };
    
    // For each configured project, get issue count via search
    let mut result = Vec::new();
    for key in &project_keys {
        let jql = format!("project = {} ORDER BY created DESC", key);
        let count = match client.search_issues(&jql).await {
            Ok(issues) => Some(issues.len()),
            Err(_) => None,
        };
        result.push(ImportableProject {
            key: key.clone(),
            name: key.clone(),
            item_count: count,
        });
    }
    
    Ok(result)
}

pub async fn import_jira_project(
    project_key: &str,
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    _event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let (scope_id, _) = find_integration_scope("jira")?;
    let config = load_integration_config(&scope_id, "jira")?;
    let client = JiraClient::new(config.domain, config.email, token.to_string());
    
    // Fetch all issues for the project
    let jql = format!("project = {} ORDER BY status ASC, priority DESC, created DESC", project_key);
    let issues = client.search_issues(&jql).await
        .map_err(|e| anyhow!("Jira search failed: {}", e))?;
    
    if issues.is_empty() {
        return Ok(ImportResult {
            source: "jira".to_string(),
            boards_created: 0,
            items_imported: 0,
            errors: vec![format!("No issues found for project {}", project_key)],
        });
    }
    
    // Group issues by status for Kanban sections
    let mut status_groups: std::collections::BTreeMap<String, Vec<&cyan_backend_integrations::clients::jira::JiraIssue>> = std::collections::BTreeMap::new();
    
    for issue in &issues {
        let status = issue.fields.status.name.clone();
        status_groups.entry(status).or_default().push(issue);
    }
    
    // Count done vs total
    let done_count = issues.iter()
        .filter(|i| {
            let s = i.fields.status.name.to_lowercase();
            s.contains("done") || s.contains("closed") || s.contains("resolved")
        })
        .count();
    let total = issues.len();
    let progress_pct = if total > 0 { (done_count * 100) / total } else { 0 };
    
    // Build single Kanban-style markdown
    let markdown = build_kanban_markdown(project_key, &status_groups, done_count, total, progress_pct);
    
    // Create ONE board
    let board_name = format!("📋 {} — Kanban", project_key);
    
    let _ = command_tx.send(CommandMsg::CreateBoard {
        workspace_id: workspace_id.to_string(),
        name: board_name.clone(),
    });
    
    let board_id = blake3::hash(format!("board:{}-{}", workspace_id, board_name).as_bytes()).to_hex().to_string();
    
    // Wait for board creation to process through command channel
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    let _ = command_tx.send(CommandMsg::AddNotebookCell {
        board_id: board_id.clone(),
        cell_type: "markdown".to_string(),
        cell_order: 0,
        content: Some(markdown),
    });
    
    // Wait for cell to be created, then set board mode to notebook
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    // Set board mode to notebook so it opens in rendered preview
    {
        let conn = storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
        let rows = conn.execute(
            "UPDATE objects SET board_mode = 'notebook' WHERE id = ?1",
            rusqlite::params![board_id],
        ).unwrap_or(0);
        tracing::info!("Set board mode to notebook for {}: {} rows affected", board_id, rows);
    }
    
    Ok(ImportResult {
        source: "jira".to_string(),
        boards_created: 1,
        items_imported: total,
        errors: vec![],
    })
}

fn build_kanban_markdown(
    project_key: &str,
    status_groups: &std::collections::BTreeMap<String, Vec<&cyan_backend_integrations::clients::jira::JiraIssue>>,
    done_count: usize,
    total: usize,
    progress_pct: usize,
) -> String {
    let mut md = format!("# {} — Project Board\n\n", project_key);
    
    // Progress bar
    let filled = progress_pct / 5; // 20 chars wide
    let empty = 20 - filled;
    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    md.push_str(&format!("**Progress:** {} {}/{} ({}%)\n\n", bar, done_count, total, progress_pct));
    md.push_str("---\n\n");
    
    // Define status ordering: To Do → In Progress → Done
    let status_order = ["To Do", "In Progress", "In Review", "Done", "Closed", "Resolved"];
    
    // Render known statuses first in order
    let mut rendered: std::collections::HashSet<&str> = std::collections::HashSet::new();
    
    for &status_name in &status_order {
        if let Some(issues) = status_groups.get(status_name) {
            render_status_section(&mut md, status_name, issues);
            rendered.insert(status_name);
        }
    }
    
    // Render any remaining statuses not in the predefined order
    for (status_name, issues) in status_groups {
        if !rendered.contains(status_name.as_str()) {
            render_status_section(&mut md, status_name, issues);
        }
    }
    
    md
}

fn render_status_section(
    md: &mut String,
    status: &str,
    issues: &[&cyan_backend_integrations::clients::jira::JiraIssue],
) {
    let status_lower = status.to_lowercase();
    let emoji = if status_lower.contains("done") || status_lower.contains("closed") || status_lower.contains("resolved") {
        "✅"
    } else if status_lower.contains("progress") || status_lower.contains("review") {
        "🔄"
    } else {
        "📋"
    };
    
    let is_done = status_lower.contains("done") || status_lower.contains("closed") || status_lower.contains("resolved");
    
    md.push_str(&format!("## {} {} ({})\n\n", emoji, status, issues.len()));
    
    for issue in issues {
        let check = if is_done { "x" } else { " " };
        let assignee = issue.fields.assignee.as_ref()
            .map(|a| format!(" → {}", a.display_name))
            .unwrap_or_default();
        let priority = issue.fields.priority.as_ref()
            .map(|p| format!(" `{}`", p.name))
            .unwrap_or_default();
        
        md.push_str(&format!("- [{}] **{}**: {}{}{}\n", 
            check, issue.key, issue.fields.summary, assignee, priority));
        
        if let Some(desc) = issue.fields.description_text() {
            let first_line = desc.lines().next().unwrap_or("");
            if !first_line.is_empty() && first_line.len() < 120 {
                md.push_str(&format!("  > {}\n", first_line));
            }
        }
    }
    
    md.push('\n');
}

// ============================================================================
// Confluence Import
// ============================================================================

pub async fn list_confluence_spaces(
    token: &str,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<Vec<ImportableProject>> {
    let (scope_id, _) = find_integration_scope("confluence")?;
    let config = load_integration_config(&scope_id, "confluence")?;
    
    // Return configured spaces
    Ok(config.projects.iter().map(|s| ImportableProject {
        key: s.clone(),
        name: s.clone(),
        item_count: None,
    }).collect())
}

pub async fn import_confluence_space(
    space_key: &str,
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let (scope_id, _) = find_integration_scope("confluence")?;
    let config = load_integration_config(&scope_id, "confluence")?;
    let client = ConfluenceClient::new(config.domain, config.email, token.to_string());
    
    let pages = client.get_space_pages(space_key).await
        .map_err(|e| anyhow!("Confluence API error: {}", e))?;
    
    let mut boards_created = 0;
    
    for page in &pages {
        let board_name = format!("📄 {}", page.title);
        
        // Convert HTML body to markdown (simple strip for now)
        let markdown = confluence_page_to_markdown(page);
        
        let _ = command_tx.send(CommandMsg::CreateBoard {
            workspace_id: workspace_id.to_string(),
            name: board_name.clone(),
        });
        
        let board_id = blake3::hash(format!("board:{}-{}", workspace_id, board_name).as_bytes()).to_hex().to_string();
        
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        let _ = command_tx.send(CommandMsg::AddNotebookCell {
            board_id,
            cell_type: "markdown".to_string(),
            cell_order: 0,
            content: Some(markdown),
        });
        
        boards_created += 1;
    }
    
    Ok(ImportResult {
        source: "confluence".to_string(),
        boards_created,
        items_imported: pages.len(),
        errors: vec![],
    })
}

fn confluence_page_to_markdown(page: &cyan_backend_integrations::clients::confluence::ConfluencePage) -> String {
    let mut md = format!("# {}\n\n", page.title);
    
    // Simple HTML to markdown conversion
    let body_text = page.body.as_ref().map(|b| b.storage.value.clone()).unwrap_or_default();
    let stripped = body_text
        .replace("<p>", "\n")
        .replace("</p>", "\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<strong>", "**")
        .replace("</strong>", "**")
        .replace("<em>", "_")
        .replace("</em>", "_")
        .replace("<h1>", "# ")
        .replace("</h1>", "\n")
        .replace("<h2>", "## ")
        .replace("</h2>", "\n")
        .replace("<h3>", "### ")
        .replace("</h3>", "\n")
        .replace("<li>", "- ")
        .replace("</li>", "\n")
        .replace("<ul>", "")
        .replace("</ul>", "\n")
        .replace("<ol>", "")
        .replace("</ol>", "\n")
        .replace("<code>", "`")
        .replace("</code>", "`");
    
    // Strip remaining HTML tags
    let mut result = String::new();
    let mut in_tag = false;
    for ch in stripped.chars() {
        if ch == '<' { in_tag = true; continue; }
        if ch == '>' { in_tag = false; continue; }
        if !in_tag { result.push(ch); }
    }
    
    md.push_str(result.trim());
    md.push('\n');
    md
}

// ============================================================================
// Google Docs Import
// ============================================================================

pub async fn list_google_docs(
    token: &str,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<Vec<ImportableProject>> {
    let (scope_id, _) = find_integration_scope("googledocs")?;
    let config = load_integration_config(&scope_id, "googledocs")?;
    let client = GoogleDocsClient::new(token.to_string());
    
    let files = client.list_files(None).await
        .map_err(|e| anyhow!("Google API error: {}", e))?;
    
    Ok(files.iter().filter_map(|f| {
        Some(ImportableProject {
            key: f["id"].as_str()?.to_string(),
            name: f["name"].as_str()?.to_string(),
            item_count: None,
        })
    }).collect())
}

pub async fn import_google_doc(
    doc_id: &str,
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let (scope_id, _) = find_integration_scope("googledocs")?;
    let config = load_integration_config(&scope_id, "googledocs")?;
    let client = GoogleDocsClient::new(token.to_string());
    
    let doc = client.get_document(doc_id).await
        .map_err(|e| anyhow!("Google Docs API error: {}", e))?;
    
    let board_name = format!("📄 {}", doc.title);
    
    let _ = command_tx.send(CommandMsg::CreateBoard {
        workspace_id: workspace_id.to_string(),
        name: board_name.clone(),
    });
    
    let board_id = blake3::hash(format!("board:{}-{}", workspace_id, board_name).as_bytes()).to_hex().to_string();
    
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    let _ = command_tx.send(CommandMsg::AddNotebookCell {
        board_id,
        cell_type: "markdown".to_string(),
        cell_order: 0,
        content: Some(format!("# {}\n\n{}", doc.title, extract_gdoc_text(&doc))),
    });
    
    Ok(ImportResult {
        source: "googledocs".to_string(),
        boards_created: 1,
        items_imported: 1,
        errors: vec![],
    })
}

pub async fn import_all_google_docs(
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let docs = list_google_docs(token, event_tx).await?;
    let mut total_boards = 0;
    let mut errors = Vec::new();
    
    for doc in &docs {
        match import_google_doc(&doc.key, workspace_id, token, command_tx, event_tx).await {
            Ok(r) => total_boards += r.boards_created,
            Err(e) => errors.push(format!("{}: {}", doc.name, e)),
        }
    }
    
    Ok(ImportResult {
        source: "googledocs".to_string(),
        boards_created: total_boards,
        items_imported: docs.len(),
        errors,
    })
}

// ============================================================================
// Import All for Jira
// ============================================================================

pub async fn import_all_jira(
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let projects = list_jira_projects(token, event_tx).await?;
    let mut total_boards = 0;
    let mut total_items = 0;
    let mut errors = Vec::new();
    
    for project in &projects {
        match import_jira_project(&project.key, workspace_id, token, command_tx, event_tx).await {
            Ok(r) => {
                total_boards += r.boards_created;
                total_items += r.items_imported;
            }
            Err(e) => errors.push(format!("{}: {}", project.key, e)),
        }
    }
    
    Ok(ImportResult {
        source: "jira".to_string(),
        boards_created: total_boards,
        items_imported: total_items,
        errors,
    })
}

pub async fn import_all_confluence(
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let spaces = list_confluence_spaces(token, event_tx).await?;
    let mut total_boards = 0;
    let mut total_items = 0;
    let mut errors = Vec::new();
    
    for space in &spaces {
        match import_confluence_space(&space.key, workspace_id, token, command_tx, event_tx).await {
            Ok(r) => {
                total_boards += r.boards_created;
                total_items += r.items_imported;
            }
            Err(e) => errors.push(format!("{}: {}", space.key, e)),
        }
    }
    
    Ok(ImportResult {
        source: "confluence".to_string(),
        boards_created: total_boards,
        items_imported: total_items,
        errors,
    })
}

// ============================================================================
// Google Docs Text Extraction
// ============================================================================

fn extract_gdoc_text(doc: &cyan_backend_integrations::clients::googledocs::GoogleDoc) -> String {
    doc.body.as_ref()
        .map(|b| {
            b.content.iter()
                .filter_map(|se| {
                    se.paragraph.as_ref().map(|p| {
                        p.elements.iter()
                            .filter_map(|e| e.text_run.as_ref().map(|t| t.content.clone()))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
