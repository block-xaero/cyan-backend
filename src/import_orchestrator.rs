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
use cyan_backend_integrations::clients::github::GitHubClient;

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
    projects: Vec<String>,     // Jira projects or Confluence spaces
    document_ids: Vec<String>, // Google Docs IDs
    repos: Vec<(String, String)>, // GitHub repos (owner, repo)
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
        repos: config["repos"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| {
                let owner = v["owner"].as_str()?;
                let repo = v["repo"].as_str()?;
                Some((owner.to_string(), repo.to_string()))
            }).collect())
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
        let markdown = confluence_page_to_markdown(page);
        
        create_notebook_board(workspace_id, &board_name, &markdown, command_tx).await?;
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
    
    let body_html = page.body.as_ref().map(|b| b.storage.value.clone()).unwrap_or_default();
    if body_html.is_empty() {
        md.push_str("_No content_\n");
        return md;
    }
    
    // Extract the base URL for images from page links
    let base_url = page.links.as_ref()
        .map(|l| {
            // Extract domain from self_link: https://domain.atlassian.net/wiki/rest/api/...
            if let Some(pos) = l.self_link.find("/wiki/") {
                l.self_link[..pos].to_string()
            } else {
                String::new()
            }
        })
        .unwrap_or_default();
    
    let result = convert_confluence_html(&body_html, &base_url, &page.id);
    md.push_str(&result);
    md.push('\n');
    md
}

fn convert_confluence_html(html: &str, base_url: &str, page_id: &str) -> String {
    let mut result = html.to_string();
    
    // 1. Handle Confluence image attachments BEFORE stripping tags
    // Pattern: <ac:image ...><ri:attachment ri:filename="name.png"/></ac:image>
    let img_re = regex::Regex::new(r#"<ac:image[^>]*>.*?<ri:attachment ri:filename="([^"]+)"[^/]*/?>.*?</ac:image>"#).ok();
    if let Some(re) = &img_re {
        result = re.replace_all(&result, |caps: &regex::Captures| {
            let filename = &caps[1];
            if base_url.is_empty() {
                format!("![{}](attachment:{})", filename, filename)
            } else {
                format!("![{}]({}/wiki/rest/api/content/{}/child/attachment?filename={})", 
                    filename, base_url, page_id, filename)
            }
        }).to_string();
    }
    
    // 2. Handle standalone img tags
    let img_tag_re = regex::Regex::new(r#"<img[^>]+src="([^"]+)"[^>]*/?"#).ok();
    if let Some(re) = &img_tag_re {
        result = re.replace_all(&result, |caps: &regex::Captures| {
            format!("![image]({})", &caps[1])
        }).to_string();
    }
    
    // 3. Handle code blocks: <ac:structured-macro ac:name="code">...<ac:plain-text-body><![CDATA[...]]></ac:plain-text-body>
    let code_re = regex::Regex::new(r#"<ac:structured-macro[^>]*ac:name="code"[^>]*>.*?<!\[CDATA\[(.*?)\]\]>.*?</ac:structured-macro>"#).ok();
    if let Some(re) = &code_re {
        result = re.replace_all(&result, |caps: &regex::Captures| {
            format!("\n```\n{}\n```\n", &caps[1])
        }).to_string();
    }
    
    // 4. Handle tables → markdown tables
    let table_re = regex::Regex::new(r#"<table[^>]*>(.*?)</table>"#).ok();
    if let Some(re) = &table_re {
        result = re.replace_all(&result, |caps: &regex::Captures| {
            convert_html_table(&caps[1])
        }).to_string();
    }
    
    // 5. Handle links: <a href="url">text</a>
    let link_re = regex::Regex::new(r#"<a[^>]+href="([^"]+)"[^>]*>([^<]*)</a>"#).ok();
    if let Some(re) = &link_re {
        result = re.replace_all(&result, |caps: &regex::Captures| {
            format!("[{}]({})", &caps[2], &caps[1])
        }).to_string();
    }
    
    // 6. Handle inline styles for colored text
    let color_re = regex::Regex::new(r#"<span[^>]*style="color:[^"]*"[^>]*>(.*?)</span>"#).ok();
    if let Some(re) = &color_re {
        result = re.replace_all(&result, "$1").to_string();
    }
    
    // 7. Strip Confluence-specific macros (inline comments, status, etc)
    let macro_re = regex::Regex::new(r#"<ac:[^>]*>|</ac:[^>]*>|<ri:[^>]*/?>|</ri:[^>]*>"#).ok();
    if let Some(re) = &macro_re {
        result = re.replace_all(&result, "").to_string();
    }
    
    // 8. Standard HTML → markdown conversions
    // Headings
    result = result.replace("<h1>", "\n# ").replace("</h1>", "\n");
    result = result.replace("<h2>", "\n## ").replace("</h2>", "\n");
    result = result.replace("<h3>", "\n### ").replace("</h3>", "\n");
    result = result.replace("<h4>", "\n#### ").replace("</h4>", "\n");
    
    // Formatting
    result = result.replace("<strong>", "**").replace("</strong>", "**");
    result = result.replace("<em>", "_").replace("</em>", "_");
    result = result.replace("<code>", "`").replace("</code>", "`");
    
    // Paragraphs and breaks
    result = result.replace("<p>", "\n").replace("</p>", "\n");
    result = result.replace("<p />", "\n").replace("<p/>", "\n");
    result = result.replace("<br>", "\n").replace("<br/>", "\n");
    
    // Lists
    result = result.replace("<ul>", "").replace("</ul>", "\n");
    result = result.replace("<ol>", "").replace("</ol>", "\n");
    
    // Handle list items with proper bullet/number detection
    // For now, use bullets for all (ordered list numbering is lost in this simple approach)
    let li_re = regex::Regex::new(r#"<li[^>]*>"#).ok();
    if let Some(re) = &li_re {
        result = re.replace_all(&result, "- ").to_string();
    }
    result = result.replace("</li>", "\n");
    
    // Horizontal rules
    result = result.replace("<hr>", "\n---\n").replace("<hr/>", "\n---\n");
    
    // 9. Strip ALL remaining HTML tags
    let tag_re = regex::Regex::new(r#"<[^>]+>"#).ok();
    if let Some(re) = &tag_re {
        result = re.replace_all(&result, "").to_string();
    }
    
    // 10. Clean up HTML entities
    result = result.replace("&amp;", "&");
    result = result.replace("&lt;", "<");
    result = result.replace("&gt;", ">");
    result = result.replace("&quot;", "\"");
    result = result.replace("&rsquo;", "'");
    result = result.replace("&lsquo;", "'");
    result = result.replace("&rdquo;", "\"");
    result = result.replace("&ldquo;", "\"");
    result = result.replace("&ndash;", "–");
    result = result.replace("&mdash;", "—");
    result = result.replace("&nbsp;", " ");
    
    // 11. Clean up excessive blank lines
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    
    result.trim().to_string()
}

fn convert_html_table(table_html: &str) -> String {
    let mut md = String::from("\n");
    
    // Extract rows
    let row_re = regex::Regex::new(r#"<tr[^>]*>(.*?)</tr>"#).ok();
    let cell_re = regex::Regex::new(r#"<t[hd][^>]*>(.*?)</t[hd]>"#).ok();
    
    let (Some(row_regex), Some(cell_regex)) = (row_re, cell_re) else {
        return table_html.to_string();
    };
    
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut is_first_row = true;
    
    for row_cap in row_regex.captures_iter(table_html) {
        let row_html = &row_cap[1];
        let cells: Vec<String> = cell_regex.captures_iter(row_html)
            .map(|c| {
                // Strip any remaining HTML from cell content
                let content = &c[1];
                let tag_strip = regex::Regex::new(r#"<[^>]+>"#).unwrap();
                tag_strip.replace_all(content, "").trim().to_string()
            })
            .collect();
        
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    
    if rows.is_empty() {
        return String::new();
    }
    
    // Build markdown table
    for (i, row) in rows.iter().enumerate() {
        md.push_str("| ");
        md.push_str(&row.join(" | "));
        md.push_str(" |\n");
        
        // Add header separator after first row
        if i == 0 {
            md.push_str("|");
            for _ in row {
                md.push_str(" --- |");
            }
            md.push('\n');
        }
    }
    
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

// ============================================================================
// GitHub Import
// ============================================================================

pub async fn list_github_repos(
    token: &str,
    _event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<Vec<ImportableProject>> {
    let client = GitHubClient::new(token.to_string());
    
    // Try config first
    let configured_repos = if let Ok((scope_id, _)) = find_integration_scope("github") {
        if let Ok(config) = load_integration_config(&scope_id, "github") {
            parse_github_repos_config(&config)
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    
    if !configured_repos.is_empty() {
        // Use configured repos
        let mut result = Vec::new();
        for (owner, repo) in &configured_repos {
            let full_name = format!("{}/{}", owner, repo);
            result.push(ImportableProject {
                key: full_name.clone(),
                name: full_name,
                item_count: None,
            });
        }
        return Ok(result);
    }
    
    // Auto-discover: list user's repos from GitHub API
    let url = "https://api.github.com/user/repos?per_page=20&sort=pushed&affiliation=owner,organization_member";
    let response = reqwest::Client::new()
        .get(url)
        .header("Authorization", format!("token {}", token))
        .header("User-Agent", "cyan-integrations")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| anyhow!("GitHub API error: {}", e))?
        .json::<Vec<serde_json::Value>>()
        .await
        .map_err(|e| anyhow!("GitHub parse error: {}", e))?;
    
    let result: Vec<ImportableProject> = response.iter().filter_map(|repo| {
        let full_name = repo["full_name"].as_str()?.to_string();
        let description = repo["description"].as_str().unwrap_or("").to_string();
        let name = if description.is_empty() {
            full_name.clone()
        } else {
            format!("{} — {}", full_name, &description[..description.len().min(50)])
        };
        Some(ImportableProject {
            key: full_name,
            name,
            item_count: None,
        })
    }).collect();
    
    Ok(result)
}

pub async fn import_github_repo(
    repo_full_name: &str,
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    _event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let parts: Vec<&str> = repo_full_name.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid repo format. Use: owner/repo"));
    }
    let (owner, repo) = (parts[0], parts[1]);
    
    let client = GitHubClient::new(token.to_string());
    
    let mut boards_created = 0;
    let mut total_items = 0;
    
    // 1. Recent commits → changelog board
    let commits = client.list_commits(owner, repo, None).await
        .map_err(|e| anyhow!("GitHub commits error: {}", e))?;
    
    if !commits.is_empty() {
        let board_name = format!("🔀 {}/{} — Commits", owner, repo);
        let markdown = github_commits_to_markdown(owner, repo, &commits);
        create_notebook_board(workspace_id, &board_name, &markdown, command_tx).await?;
        boards_created += 1;
        total_items += commits.len();
    }
    
    // 2. Open PRs → review board
    let prs = client.list_pull_requests(owner, repo, Some("all")).await
        .map_err(|e| anyhow!("GitHub PRs error: {}", e))?;
    
    if !prs.is_empty() {
        let board_name = format!("🔀 {}/{} — Pull Requests", owner, repo);
        let markdown = github_prs_to_markdown(owner, repo, &prs);
        create_notebook_board(workspace_id, &board_name, &markdown, command_tx).await?;
        boards_created += 1;
        total_items += prs.len();
    }
    
    // 3. Issues → tracking board
    let issues = client.list_issues(owner, repo, Some("all")).await
        .map_err(|e| anyhow!("GitHub issues error: {}", e))?;
    
    // Filter out PRs (GitHub API returns PRs as issues too)
    let real_issues: Vec<_> = issues.iter()
        .filter(|i| !i.html_url.contains("/pull/"))
        .collect();
    
    if !real_issues.is_empty() {
        let board_name = format!("🔀 {}/{} — Issues", owner, repo);
        let markdown = github_issues_to_markdown(owner, repo, &real_issues);
        create_notebook_board(workspace_id, &board_name, &markdown, command_tx).await?;
        boards_created += 1;
        total_items += real_issues.len();
    }
    
    Ok(ImportResult {
        source: "github".to_string(),
        boards_created,
        items_imported: total_items,
        errors: vec![],
    })
}

/// Helper to create a board in notebook mode with a markdown cell
async fn create_notebook_board(
    workspace_id: &str,
    board_name: &str,
    markdown: &str,
    command_tx: &UnboundedSender<CommandMsg>,
) -> Result<String> {
    let _ = command_tx.send(CommandMsg::CreateBoard {
        workspace_id: workspace_id.to_string(),
        name: board_name.to_string(),
    });
    
    let board_id = blake3::hash(format!("board:{}-{}", workspace_id, board_name).as_bytes()).to_hex().to_string();
    
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    let _ = command_tx.send(CommandMsg::AddNotebookCell {
        board_id: board_id.clone(),
        cell_type: "markdown".to_string(),
        cell_order: 0,
        content: Some(markdown.to_string()),
    });
    
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Set notebook mode
    {
        let conn = storage::db().lock().map_err(|e| anyhow!("DB lock: {}", e))?;
        let _ = conn.execute(
            "UPDATE objects SET board_mode = 'notebook' WHERE id = ?1",
            rusqlite::params![board_id],
        );
    }
    
    Ok(board_id)
}

fn github_commits_to_markdown(owner: &str, repo: &str, commits: &[cyan_backend_integrations::clients::github::GitHubCommit]) -> String {
    let mut md = format!("# {}/{} — Recent Commits\n\n", owner, repo);
    md.push_str(&format!("**{}** commits\n\n", commits.len()));
    
    for commit in commits {
        let msg = commit.commit.message.lines().next().unwrap_or("");
        let author = &commit.commit.author.name;
        let date = &commit.commit.author.date;
        let short_sha = &commit.sha[..7.min(commit.sha.len())];
        
        // Detect issue/PR references in commit message
        let has_ref = msg.contains('#') || msg.to_lowercase().contains("fix") || 
                      msg.to_lowercase().contains("resolve") || msg.to_lowercase().contains("close");
        let marker = if has_ref { "🔗" } else { "•" };
        
        md.push_str(&format!("{} `{}` **{}** — {}\n", marker, short_sha, msg, author));
        
        // Show date on separate line
        if let Some(date_part) = date.split('T').next() {
            md.push_str(&format!("  _{}_\n", date_part));
        }
    }
    
    md
}

fn github_prs_to_markdown(owner: &str, repo: &str, prs: &[cyan_backend_integrations::clients::github::GitHubPullRequest]) -> String {
    let mut md = format!("# {}/{} — Pull Requests\n\n", owner, repo);
    
    let open: Vec<_> = prs.iter().filter(|p| p.state == "open").collect();
    let merged: Vec<_> = prs.iter().filter(|p| p.state == "closed").collect();
    
    let total = prs.len();
    let open_count = open.len();
    let merged_count = merged.len();
    md.push_str(&format!("**{}** PRs ({} open, {} closed)\n\n", total, open_count, merged_count));
    
    if !open.is_empty() {
        md.push_str("## 🟢 Open\n\n");
        for pr in &open {
            let assignee = pr.user.login.as_str();
            md.push_str(&format!("- [ ] **#{}**: {} → @{}\n", pr.number, pr.title, assignee));
            if let Some(body) = &pr.body {
                let first = body.lines().next().unwrap_or("");
                if !first.is_empty() && first.len() < 100 {
                    md.push_str(&format!("  > {}\n", first));
                }
            }
        }
        md.push('\n');
    }
    
    if !merged.is_empty() {
        md.push_str("## ✅ Closed/Merged\n\n");
        for pr in merged.iter().take(15) {
            md.push_str(&format!("- [x] **#{}**: {} → @{}\n", pr.number, pr.title, pr.user.login));
        }
        if merged_count > 15 {
            md.push_str(&format!("\n_...and {} more_\n", merged_count - 15));
        }
        md.push('\n');
    }
    
    md
}

fn github_issues_to_markdown(owner: &str, repo: &str, issues: &[&cyan_backend_integrations::clients::github::GitHubIssue]) -> String {
    let mut md = format!("# {}/{} — Issues\n\n", owner, repo);
    
    let open: Vec<_> = issues.iter().filter(|i| i.state == "open").collect();
    let closed: Vec<_> = issues.iter().filter(|i| i.state == "closed").collect();
    
    let open_count = open.len();
    let closed_count = closed.len();
    let total = issues.len();
    let progress = if total > 0 { (closed_count * 100) / total } else { 0 };
    
    let filled = progress / 5;
    let empty = 20 - filled;
    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    md.push_str(&format!("**Progress:** {} {}/{} ({}%)\n\n", bar, closed_count, total, progress));
    md.push_str("---\n\n");
    
    if !open.is_empty() {
        md.push_str(&format!("## 📋 Open ({})\n\n", open_count));
        for issue in &open {
            let labels = issue.labels.iter().map(|l| format!("`{}`", l.name)).collect::<Vec<_>>().join(" ");
            let assignee = issue.assignee.as_ref().map(|a| format!(" → @{}", a.login)).unwrap_or_default();
            md.push_str(&format!("- [ ] **#{}**: {}{} {}\n", issue.number, issue.title, assignee, labels));
        }
        md.push('\n');
    }
    
    if !closed.is_empty() {
        md.push_str(&format!("## ✅ Closed ({})\n\n", closed_count));
        for issue in closed.iter().take(15) {
            md.push_str(&format!("- [x] **#{}**: {}\n", issue.number, issue.title));
        }
        if closed_count > 15 {
            md.push_str(&format!("\n_...and {} more_\n", closed_count - 15));
        }
    }
    
    md
}

fn parse_github_repos_config(config: &IntegrationConfig) -> Vec<(String, String)> {
    if !config.repos.is_empty() {
        return config.repos.clone();
    }
    // Fallback: try projects field as "owner/repo" strings
    config.projects.iter().filter_map(|entry| {
        let parts: Vec<&str> = entry.splitn(2, '/').collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }).collect()
}

pub async fn import_all_github(
    workspace_id: &str,
    token: &str,
    command_tx: &UnboundedSender<CommandMsg>,
    event_tx: &UnboundedSender<SwiftEvent>,
) -> Result<ImportResult> {
    let repos = list_github_repos(token, event_tx).await?;
    let mut total_boards = 0;
    let mut total_items = 0;
    let mut errors = Vec::new();
    
    for repo in &repos {
        match import_github_repo(&repo.key, workspace_id, token, command_tx, event_tx).await {
            Ok(r) => {
                total_boards += r.boards_created;
                total_items += r.items_imported;
            }
            Err(e) => errors.push(format!("{}: {}", repo.key, e)),
        }
    }
    
    Ok(ImportResult {
        source: "github".to_string(),
        boards_created: total_boards,
        items_imported: total_items,
        errors,
    })
}
