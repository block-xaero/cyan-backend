# Cyan Integration System Design

> Living documentation derived from `integration_bridge.rs` implementation

## Overview

Cyan's integration system connects external tools (Slack, Jira, GitHub, Confluence, Google Docs) to the collaborative whiteboard, surfacing real-time signals in a console pane. This keeps system designs honest by linking diagrams to actual codebase activity, tickets, and team conversations.

```
┌─────────────────────────────────────────────────────────────────┐
│                         CYAN APP                                │
│  ┌──────────┐  ┌──────────────────────┐  ┌──────────────────┐  │
│  │ File Tree│  │     Whiteboard       │  │    Chat Pane     │  │
│  │          │  │                      │  │                  │  │
│  │ 📁 Eng   │  │   ┌────┐  ┌────┐    │  │                  │  │
│  │  ⚙️ Config│  │   │    │  │    │    │  │                  │  │
│  │   slack  │  │   └────┘  └────┘    │  │                  │  │
│  │   jira   │  │                      │  │                  │  │
│  └──────────┘  └──────────────────────┘  └──────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ Console                                          ▼ ✕     │  │
│  │ 14:32 [GH] feat: add rate limiting          PR #241      │  │
│  │ 14:31 [SK] @rick: "cap at 100 rps?"         #auth-team   │  │
│  │ 14:28 [JR] PLAT-442 → In Review             @sarah       │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Architecture

### Component Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                            SWIFT                                    │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                 │
│  │ FileTreeView│  │ ConsoleView │  │IntegrationVM│                 │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                 │
│         │                │                │                         │
│         │    ┌───────────┴────────────────┴───────────┐            │
│         │    │         IntegrationService             │            │
│         │    │  • OAuth WebView flows                 │            │
│         │    │  • Keychain token storage              │            │
│         │    │  • FFI calls to Rust                   │            │
│         │    └───────────────────┬────────────────────┘            │
│         │                        │                                  │
└─────────┼────────────────────────┼──────────────────────────────────┘
          │                        │
          │  ┌─────────────────────▼─────────────────────┐
          │  │      cyan_integration_command(json)       │  FFI
          │  └─────────────────────┬─────────────────────┘
          │                        │
┌─────────┼────────────────────────┼──────────────────────────────────┐
│         │                        │                     RUST         │
│         │         ┌──────────────▼──────────────┐                   │
│         │         │     IntegrationBridge       │                   │
│         │         │  • Command dispatch         │                   │
│         │         │  • Actor lifecycle          │                   │
│         │         │  • Binding persistence      │                   │
│         │         └──────────────┬──────────────┘                   │
│         │                        │                                  │
│         │    ┌───────────────────┼───────────────────┐              │
│         │    │                   │                   │              │
│         │    ▼                   ▼                   ▼              │
│    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐           │
│    │ SlackActor  │    │ JiraActor   │    │ GitHubActor │  ...      │
│    │  poll 30s   │    │  poll 60s   │    │  poll 60s   │           │
│    └──────┬──────┘    └──────┬──────┘    └──────┬──────┘           │
│           │                  │                  │                   │
│           └──────────────────┼──────────────────┘                   │
│                              │                                      │
│                              ▼                                      │
│                    event_tx.send(SwiftEvent)                        │
│                              │                                      │
└──────────────────────────────┼──────────────────────────────────────┘
                               │
          ┌────────────────────▼────────────────────┐
          │         cyan_poll_events()              │  FFI
          └────────────────────┬────────────────────┘
                               │
                               ▼
                        Swift ConsoleView
```

### Data Flow

```
1. USER AUTHENTICATES
   Swift: OAuth WebView → token → Keychain.set(token, service: "slack")

2. USER ADDS INTEGRATION
   Swift: cyan_integration_command({"cmd":"start", "token": keychain.get(), ...})
   Rust:  IntegrationBridge.cmd_start() → spawn actor → save binding (no token)

3. ACTOR POLLS EXTERNAL API
   Rust:  SlackActor polls every 30s with in-memory token
   Rust:  event_tx.send(SwiftEvent::IntegrationEvent{...})

4. SWIFT RECEIVES EVENT
   Swift: cyan_poll_events() → decode → ConsoleViewModel.append(event)

5. APP RESTART
   Swift: Load bindings → fetch tokens from Keychain → start each integration
```

---

## Rust Implementation

### Core Types

```rust
/// Integration types supported
pub enum IntegrationType {
    Slack,
    Jira,
    GitHub,
    Confluence,
    GoogleDocs,
}

/// Event sent to Swift for console display
SwiftEvent::IntegrationEvent {
    id: String,          // Unique event ID (blake3 hash)
    scope_id: String,    // "group:xxx" or "workspace:xxx"
    source: String,      // "slack" | "jira" | "github" | etc
    summary: String,     // "feat: add rate limiting..."
    context: String,     // "PR #241" | "#auth" | "@sarah"
    url: Option<String>, // Deep link to source
    ts: u64,             // Unix timestamp
}

/// Status change notification
SwiftEvent::IntegrationStatus {
    scope_id: String,
    integration_type: String,
    status: String,      // "connected" | "syncing" | "error" | "stopped"
    message: Option<String>,
}
```

### Database Schema

```sql
-- Bindings only, NO tokens (tokens live in Swift Keychain)
CREATE TABLE integration_bindings (
    id TEXT PRIMARY KEY,
    scope_type TEXT NOT NULL,        -- 'group' | 'workspace'
    scope_id TEXT NOT NULL,
    integration_type TEXT NOT NULL,  -- 'slack' | 'jira' | etc
    config_json TEXT NOT NULL,       -- channels, repos, projects
    created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_integration_unique 
    ON integration_bindings(scope_id, integration_type);
```

### FFI Interface

Single JSON-dispatch function handles all commands:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn cyan_integration_command(json: *const c_char) -> *mut c_char
```

#### Commands

| Command | Payload | Response |
|---------|---------|----------|
| `start` | `{scope_type, scope_id, integration_type, token, config}` | `{success: true}` |
| `stop` | `{scope_id, integration_type}` | `{success: true}` |
| `remove_binding` | `{scope_id, integration_type}` | `{success: true}` |
| `get_bindings` | `{scope_id, scope_type?}` | `{success: true, data: [...]}` |
| `is_running` | `{scope_id, integration_type}` | `{success: true, data: {running: bool}}` |

#### Config Examples

```json
// Slack
{"channels": ["C1234ABCD", "C5678EFGH"]}

// Jira
{"domain": "company", "email": "user@company.com", "projects": ["PLAT", "AUTH"]}

// GitHub
{"repos": [{"owner": "org", "repo": "backend"}, {"owner": "org", "repo": "frontend"}]}

// Confluence
{"domain": "company", "email": "user@company.com", "spaces": ["ENG", "ARCH"]}

// Google Docs
{"document_ids": ["1abc...", "2def..."]}
```

### Actor Lifecycle

```rust
struct ActorHandle {
    shutdown_tx: mpsc::Sender<()>,
    integration_type: IntegrationType,
}

// Actors keyed by "{scope_type}:{scope_id}:{integration_type}"
actors: RwLock<HashMap<String, ActorHandle>>
```

**Start:**
1. Validate scope_type and integration_type
2. Check not already running
3. Create shutdown channel
4. Spawn actor task with token (in-memory only)
5. Store handle in actors map
6. Save binding to SQLite (no token)
7. Send `IntegrationStatus{status: "connected"}`

**Stop:**
1. Find handle by key
2. Send on shutdown_tx
3. Remove from actors map
4. Send `IntegrationStatus{status: "stopped"}`

**Remove Binding:**
1. Stop actor (if running)
2. Delete from SQLite

### Actor Poll Loops

Each actor follows the same pattern:

```rust
async fn slack_actor_loop(
    scope_id: String,
    token: String,              // In-memory only, never persisted
    channels: Vec<String>,
    event_tx: UnboundedSender<SwiftEvent>,
    mut shutdown_rx: Receiver<()>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    
    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Poll Slack API
                // Transform to SwiftEvent::IntegrationEvent
                // event_tx.send(...)
            }
            _ = shutdown_rx.recv() => {
                break;
            }
        }
    }
}
```

| Integration | Poll Interval | Notes |
|-------------|---------------|-------|
| Slack | 30s | Extract JIRA mentions via regex |
| Jira | 60s | Track status transitions |
| GitHub | 60s | Commits, PRs, extract JIRA refs |
| Confluence | 120s | Page updates |
| Google Docs | 120s | Document revisions |

---

## Swift Implementation

### Token Security Model

```
┌─────────────────────────────────────────────────────────┐
│                    KEYCHAIN                             │
│  service: "com.cyan.integration.slack"                  │
│  account: "{workspace_id}"                              │
│  data: "xoxb-..."                                       │
├─────────────────────────────────────────────────────────┤
│  service: "com.cyan.integration.jira"                   │
│  account: "{workspace_id}"                              │
│  data: "api_token_..."                                  │
└─────────────────────────────────────────────────────────┘

         │
         │ Token retrieved at runtime
         ▼

┌─────────────────────────────────────────────────────────┐
│               RUST (in-memory only)                     │
│  SlackActor { token: "xoxb-..." }  // Never persisted   │
└─────────────────────────────────────────────────────────┘
```

### IntegrationService

```swift
@MainActor
class IntegrationService: ObservableObject {
    
    // MARK: - OAuth Flow
    
    func authenticateSlack(for scopeId: String) async throws {
        let token = try await SlackOAuthWebView.authenticate(
            clientId: Config.slackClientId,
            scopes: ["channels:history", "channels:read", "users:read"]
        )
        
        try KeychainService.set(
            token,
            service: "com.cyan.integration.slack",
            account: scopeId
        )
    }
    
    // MARK: - Start Integration
    
    func startIntegration(
        scopeType: String,
        scopeId: String,
        type: IntegrationType,
        config: [String: Any]
    ) throws {
        // Get token from Keychain
        guard let token = try KeychainService.get(
            service: "com.cyan.integration.\(type.rawValue)",
            account: scopeId
        ) else {
            throw IntegrationError.notAuthenticated
        }
        
        // Build command
        let command: [String: Any] = [
            "cmd": "start",
            "scope_type": scopeType,
            "scope_id": scopeId,
            "integration_type": type.rawValue,
            "token": token,
            "config": config
        ]
        
        let json = try JSONSerialization.string(from: command)
        let result = String(cString: cyan_integration_command(json))
        
        let response = try JSONDecoder().decode(CommandResponse.self, from: result.data(using: .utf8)!)
        if !response.success {
            throw IntegrationError.startFailed(response.error ?? "Unknown")
        }
    }
    
    // MARK: - App Restart Recovery
    
    func restoreIntegrations(for scopeId: String) async throws {
        let command = """
        {"cmd": "get_bindings", "scope_id": "\(scopeId)"}
        """
        let result = String(cString: cyan_integration_command(command))
        let response = try JSONDecoder().decode(BindingsResponse.self, from: result.data(using: .utf8)!)
        
        for binding in response.data ?? [] {
            // Fetch token from Keychain
            if let token = try? KeychainService.get(
                service: "com.cyan.integration.\(binding.integrationType)",
                account: binding.scopeId
            ) {
                // Restart the integration
                try? startIntegration(
                    scopeType: binding.scopeType,
                    scopeId: binding.scopeId,
                    type: IntegrationType(rawValue: binding.integrationType)!,
                    config: binding.config
                )
            }
        }
    }
}
```

### File Tree Integration

```swift
// FileTreeNode extension for integrations
struct IntegrationNode: Identifiable {
    let id: String
    let integrationType: String
    let scopeId: String
    let config: [String: Any]
    
    var icon: String {
        switch integrationType {
        case "slack": return "slack_icon"
        case "jira": return "jira_icon"
        case "github": return "github_icon"
        case "confluence": return "confluence_icon"
        case "googledocs": return "gdocs_icon"
        default: return "integration_icon"
        }
    }
    
    var displayName: String {
        switch integrationType {
        case "slack": 
            let channels = (config["channels"] as? [String]) ?? []
            return "Slack (\(channels.count) channels)"
        case "jira":
            let projects = (config["projects"] as? [String]) ?? []
            return "Jira (\(projects.joined(separator: ", ")))"
        // ... etc
        default: return integrationType.capitalized
        }
    }
}

// In FileTreeView
struct ConfigFolderView: View {
    let scopeId: String
    @State private var integrations: [IntegrationNode] = []
    
    var body: some View {
        DisclosureGroup {
            // Integrations subfolder
            DisclosureGroup {
                ForEach(integrations) { integration in
                    HStack {
                        Image(integration.icon)
                        Text(integration.displayName)
                    }
                    .contextMenu {
                        Button("Remove") {
                            removeIntegration(integration)
                        }
                    }
                }
                
                Button("Add Integration...") {
                    showAddIntegrationSheet = true
                }
            } label: {
                Label("Integrations", systemImage: "link")
            }
            
            // Model subfolder (future)
            DisclosureGroup {
                // ...
            } label: {
                Label("Model", systemImage: "brain")
            }
        } label: {
            Label("Config", systemImage: "gearshape")
        }
    }
}
```

### Console View

```swift
struct ConsoleView: View {
    @ObservedObject var viewModel: ConsoleViewModel
    @State private var isExpanded = false
    @State private var sourceFilter: Set<String> = []
    
    var body: some View {
        VStack(spacing: 0) {
            // Header bar
            HStack {
                Text("Console")
                    .font(.headline)
                
                Spacer()
                
                // Filter chips
                ForEach(["slack", "jira", "github"], id: \.self) { source in
                    FilterChip(
                        source: source,
                        isSelected: sourceFilter.contains(source),
                        onTap: { toggleFilter(source) }
                    )
                }
                
                // Event count badge
                Text("\(viewModel.events.count)")
                    .font(.caption)
                    .padding(.horizontal, 6)
                    .background(Color.secondary.opacity(0.2))
                    .clipShape(Capsule())
                
                // Expand/collapse
                Button(action: { isExpanded.toggle() }) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.up")
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Color(.systemBackground))
            
            if isExpanded {
                Divider()
                
                // Event list
                ScrollViewReader { proxy in
                    List(filteredEvents) { event in
                        ConsoleEventRow(event: event)
                            .id(event.id)
                    }
                    .listStyle(.plain)
                    .onChange(of: viewModel.events.count) { _ in
                        if let last = filteredEvents.last {
                            proxy.scrollTo(last.id, anchor: .bottom)
                        }
                    }
                }
                .frame(height: 200)
            }
        }
        .background(Color(.secondarySystemBackground))
    }
    
    var filteredEvents: [IntegrationEvent] {
        if sourceFilter.isEmpty {
            return viewModel.events
        }
        return viewModel.events.filter { sourceFilter.contains($0.source) }
    }
}

struct ConsoleEventRow: View {
    let event: IntegrationEvent
    
    var body: some View {
        HStack(spacing: 8) {
            // Timestamp
            Text(event.formattedTime)
                .font(.caption.monospaced())
                .foregroundColor(.secondary)
            
            // Source icon
            Image(event.sourceIcon)
                .resizable()
                .frame(width: 16, height: 16)
            
            // Summary
            Text(event.summary)
                .font(.body)
                .lineLimit(1)
            
            Spacer()
            
            // Context
            Text(event.context)
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .onTapGesture {
            if let url = event.url {
                NSWorkspace.shared.open(URL(string: url)!)
            }
        }
    }
}
```

### ConsoleViewModel

```swift
@MainActor
class ConsoleViewModel: ObservableObject {
    @Published var events: [IntegrationEvent] = []
    @Published var statuses: [String: IntegrationStatus] = [:]
    
    private let maxEvents = 500
    
    func append(_ event: IntegrationEvent) {
        events.append(event)
        if events.count > maxEvents {
            events.removeFirst(events.count - maxEvents)
        }
    }
    
    func updateStatus(_ status: IntegrationStatus) {
        let key = "\(status.scopeId):\(status.integrationType)"
        statuses[key] = status
    }
    
    func filterByCitations(_ eventIds: [String]) {
        // Called when user clicks citation in AI chat
        // Filter events to show only cited ones
    }
    
    func clearFilter() {
        // Reset to show all events
    }
}
```

### Event Polling Integration

```swift
// In CyanService or EventPoller
func processEvent(_ json: String) {
    guard let data = json.data(using: .utf8) else { return }
    
    do {
        let event = try JSONDecoder().decode(SwiftEvent.self, from: data)
        
        switch event {
        case .integrationEvent(let e):
            Task { @MainActor in
                consoleViewModel.append(e)
            }
            
        case .integrationStatus(let s):
            Task { @MainActor in
                consoleViewModel.updateStatus(s)
                integrationService.handleStatusChange(s)
            }
            
        // ... other event types
        default:
            break
        }
    } catch {
        print("Failed to decode event: \(error)")
    }
}
```

---

## OAuth Flows

### Slack

```swift
class SlackOAuthWebView: NSViewRepresentable {
    static func authenticate(
        clientId: String,
        scopes: [String]
    ) async throws -> String {
        
        let state = UUID().uuidString
        let scopeString = scopes.joined(separator: ",")
        
        let authURL = URL(string: """
            https://slack.com/oauth/v2/authorize?\
            client_id=\(clientId)&\
            scope=\(scopeString)&\
            redirect_uri=\(Config.slackRedirectUri)&\
            state=\(state)
            """)!
        
        // Present WebView, wait for redirect
        let code = try await presentOAuthWebView(url: authURL, state: state)
        
        // Exchange code for token (via your backend or directly)
        let token = try await exchangeCodeForToken(code)
        
        return token
    }
}
```

**Required Slack Scopes:**
- `channels:history` - Read messages
- `channels:read` - List channels
- `users:read` - Resolve user names
- `links:read` - Extract URLs

### Jira

```swift
class JiraOAuthWebView {
    static func authenticate(cloudId: String) async throws -> String {
        let authURL = URL(string: """
            https://auth.atlassian.com/authorize?\
            audience=api.atlassian.com&\
            client_id=\(Config.jiraClientId)&\
            scope=read:jira-work read:jira-user&\
            redirect_uri=\(Config.jiraRedirectUri)&\
            response_type=code&\
            prompt=consent
            """)!
        
        // ... similar flow
    }
}
```

### GitHub

```swift
class GitHubOAuthWebView {
    static func authenticate() async throws -> String {
        let authURL = URL(string: """
            https://github.com/login/oauth/authorize?\
            client_id=\(Config.githubClientId)&\
            scope=repo read:org&\
            redirect_uri=\(Config.githubRedirectUri)
            """)!
        
        // ... similar flow
    }
}
```

---

## Console Display Formats

| Source | Event Type | Summary | Context |
|--------|-----------|---------|---------|
| Slack | message | `@rick: "should we cap at 100 rps?"` | `#auth-team` |
| Slack | jira_mention | `PLAT-442 mentioned` | `#auth-team` |
| GitHub | commit | `feat: add rate limiting` | `PR #241` |
| GitHub | pr_opened | `Add rate limiting to auth` | `@rick` |
| Jira | status_change | `PLAT-442 → In Review` | `@sarah` |
| Jira | comment | `Comment on PLAT-442` | `@mike` |
| Confluence | page_updated | `Auth Service doc updated` | `@mike` |
| Google Docs | revision | `Design Spec v3` | `@team` |

---

## Scope Inheritance

Integrations can bind at group or workspace level:

```
Group: Engineering
├── Integration: Slack #engineering (group-level)
├── Workspace: Auth Service
│   ├── Integration: Jira PLAT (workspace-level)
│   └── Integration: GitHub org/auth (workspace-level)
└── Workspace: API Gateway
    └── (inherits Slack from group)
```

**Resolution:** Swift handles inheritance. When displaying console for a workspace:
1. Get workspace-level integrations
2. Get parent group-level integrations
3. Merge, workspace overrides group if conflict

Rust just stamps `scope_id` on events; doesn't resolve inheritance.

---

## Error Handling

### OAuth Failures

```swift
enum IntegrationError: Error {
    case oauthCancelled
    case oauthFailed(String)
    case tokenExpired
    case notAuthenticated
    case startFailed(String)
    case apiError(String)
}

// In IntegrationService
func handleStatusChange(_ status: IntegrationStatus) {
    if status.status == "error" {
        if status.message?.contains("token_expired") == true {
            // Prompt re-authentication
            showReauthPrompt(for: status.integrationType, scopeId: status.scopeId)
        }
    }
}
```

### Rust-Side Errors

Actors catch API errors and emit status events:

```rust
SwiftEvent::IntegrationStatus {
    scope_id,
    integration_type: "slack".into(),
    status: "error".into(),
    message: Some("Rate limited, retrying in 60s".into()),
}
```

---

## File Structure

```
cyan-app/
├── Cyan/
│   ├── Services/
│   │   ├── IntegrationService.swift
│   │   ├── KeychainService.swift
│   │   └── OAuth/
│   │       ├── SlackOAuthWebView.swift
│   │       ├── JiraOAuthWebView.swift
│   │       ├── GitHubOAuthWebView.swift
│   │       ├── ConfluenceOAuthWebView.swift
│   │       └── GoogleOAuthWebView.swift
│   ├── ViewModels/
│   │   ├── ConsoleViewModel.swift
│   │   └── IntegrationListViewModel.swift
│   └── Views/
│       ├── Console/
│       │   ├── ConsoleView.swift
│       │   ├── ConsoleEventRow.swift
│       │   └── FilterChip.swift
│       └── FileTree/
│           ├── ConfigFolderView.swift
│           └── AddIntegrationSheet.swift

cyan-backend/
└── src/
    ├── lib.rs
    └── integration_bridge.rs
```

---

## Implementation Checklist

### Rust (Complete ✓)

- [x] `integration_bridge.rs` module
- [x] `IntegrationBridge` struct
- [x] JSON command dispatch
- [x] `integration_bindings` SQLite table
- [x] Actor spawn/shutdown lifecycle
- [x] `SwiftEvent::IntegrationEvent` variant
- [x] `SwiftEvent::IntegrationStatus` variant
- [x] `cyan_integration_command` FFI
- [x] Stub actor loops (replace with real API clients)

### Swift (TODO)

- [ ] `KeychainService` for token storage
- [ ] `IntegrationService` orchestration
- [ ] OAuth WebViews (Slack, Jira, GitHub, Confluence, Google)
- [ ] `SwiftEvent` enum updates (IntegrationEvent, IntegrationStatus)
- [ ] `ConsoleViewModel`
- [ ] `ConsoleView` with expand/collapse
- [ ] `ConsoleEventRow` formatting
- [ ] Filter chips by source
- [ ] File tree `.config/integrations/` display
- [ ] `AddIntegrationSheet` modal
- [ ] Integration context menu (remove)
- [ ] App restart integration recovery
- [ ] Error handling and re-auth prompts

---

## Future Enhancements

1. **Webhook Support** - Real-time push instead of polling for services that support it
2. **Event Persistence** - SQLite table for event history (currently in-memory)
3. **AI Citations** - Link AI insights to source events
4. **Cross-Peer Sync** - Broadcast integration events via gossip (requires `network_tx`)
5. **Custom Integrations** - User-defined webhook endpoints
6. **Event Search** - Full-text search across integration events