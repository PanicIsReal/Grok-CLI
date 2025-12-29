use crate::api::{GrokClient, Message};
use crate::config::{save_config, Config};
use crate::persistence::{save_history, save_context, load_context, DEFAULT_HISTORY_FILE, DEFAULT_CONTEXT_FILE};
use crate::tools::{execute_tool, get_tool_definitions, ToolCall};
use crate::settings::SettingsModalState;
use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, ListState},
    text::Span,
};
use serde_json::{json, Value};
use std::sync::{mpsc, Arc};
use std::io::{BufWriter, Write};
use std::fs::OpenOptions;
use tui_textarea::TextArea;
use serde::{Deserialize, Serialize};

// Todo item for task tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

// Context window limits (conservative estimates)

/// Estimate token count (~4 chars per token for English)
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate tokens for a message including role overhead
fn message_tokens(msg: &Message) -> usize {
    let mut tokens = 4; // role overhead
    if let Some(content) = &msg.content {
        tokens += estimate_tokens(content);
    }
    if let Some(tool_calls) = &msg.tool_calls {
        for tc in tool_calls {
            tokens += estimate_tokens(&tc.function.name);
            tokens += estimate_tokens(&tc.function.arguments);
        }
    }
    tokens
}

/// Calculate total tokens in message history
pub fn total_context_tokens(messages: &[Message]) -> usize {
    messages.iter().map(message_tokens).sum()
}

/// Safely truncate a string at a character boundary
fn safe_truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

/// Parsed role directive from a message
#[derive(Debug, Clone)]
pub struct RoleDirective {
    pub role: String,
    pub content: String,
}

/// Parse @role: directive from start of message
/// Returns (role_name, remaining_content) if found
pub fn parse_role_directive(content: &str) -> Option<RoleDirective> {
    let trimmed = content.trim();
    if !trimmed.starts_with('@') {
        return None;
    }

    // Find the colon
    if let Some(colon_pos) = trimmed.find(':') {
        let role = trimmed[1..colon_pos].trim().to_lowercase();
        let remaining = trimmed[colon_pos + 1..].trim().to_string();

        // Validate role name (alphanumeric only)
        if role.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') && !role.is_empty() {
            return Some(RoleDirective { role, content: remaining });
        }
    }

    None
}

/// Find handoff directive in assistant response
/// Looks for patterns like "@coder:" or "hand off to @coder:" in the text
pub fn find_handoff_directive(content: &str) -> Option<RoleDirective> {
    // Look for @role: pattern anywhere in the content
    for line in content.lines() {
        let trimmed = line.trim();

        // Direct @role: at start of line
        if let Some(directive) = parse_role_directive(trimmed) {
            return Some(directive);
        }

        // "hand off to @role:" or "handoff to @role:" pattern
        let lower = trimmed.to_lowercase();
        if lower.contains("hand off to @") || lower.contains("handoff to @") {
            if let Some(at_pos) = trimmed.find('@') {
                let after_at = &trimmed[at_pos..];
                if let Some(directive) = parse_role_directive(after_at) {
                    return Some(directive);
                }
            }
        }
    }

    None
}

/// Compress message history in-place (standalone version for async context)
/// Returns true if compression was performed
fn compress_history_if_needed(history: &mut Vec<Message>, max_context: usize) -> bool {
    let tokens = total_context_tokens(history);
    let trigger_threshold = max_context * 7 / 10;

    if tokens < trigger_threshold {
        return false;
    }

    // Dynamic keep_recent
    let base_keep = 6;
    let max_keep = 20;
    let available_for_recent = max_context * 3 / 10;
    let avg_msg_tokens = tokens / history.len().max(1);
    let keep_recent = if avg_msg_tokens > 0 {
        (available_for_recent / avg_msg_tokens).clamp(base_keep, max_keep)
    } else {
        base_keep
    };

    if history.len() <= keep_recent + 1 {
        return false;
    }

    // Build tool_call_id -> tool_name map
    let mut tool_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for msg in history.iter() {
        if let Some(tcs) = &msg.tool_calls {
            for tc in tcs {
                tool_names.insert(tc.id.clone(), tc.function.name.clone());
            }
        }
    }

    let to_summarize = &history[1..history.len() - keep_recent];
    if to_summarize.is_empty() {
        return false;
    }

    // Build summary
    let mut summary_parts: Vec<String> = Vec::new();
    let max_summary_chars = 8000;
    let mut current_chars = 0;

    for msg in to_summarize {
        if current_chars >= max_summary_chars {
            break;
        }

        let part = match msg.role.as_str() {
            "user" => msg.content.as_ref().map(|c| format!("User: {}", safe_truncate(c, 120))),
            "assistant" => {
                let mut parts = Vec::new();
                if let Some(tcs) = &msg.tool_calls {
                    let tools: Vec<_> = tcs.iter().map(|t| t.function.name.as_str()).collect();
                    if !tools.is_empty() {
                        parts.push(format!("Assistant used: {}", tools.join(", ")));
                    }
                }
                if let Some(c) = &msg.content {
                    if !c.is_empty() {
                        parts.push(format!("Assistant: {}", safe_truncate(c, 150)));
                    }
                }
                if parts.is_empty() { None } else { Some(parts.join(" | ")) }
            }
            "tool" => {
                msg.content.as_ref().map(|c| {
                    let tool_name = msg.tool_call_id.as_ref()
                        .and_then(|id| tool_names.get(id))
                        .map(|s| s.as_str());
                    format!("  → {}", summarize_tool_result(c, tool_name))
                })
            }
            _ => None,
        };

        if let Some(p) = part {
            current_chars += p.len();
            summary_parts.push(p);
        }
    }

    // Trim if too large
    while current_chars > max_summary_chars && summary_parts.len() > 10 {
        let removed = summary_parts.remove(0);
        current_chars = current_chars.saturating_sub(removed.len());
    }

    // Rebuild history
    let system_msg = history[0].clone();
    let recent: Vec<_> = history[history.len() - keep_recent..].to_vec();
    let summary = summary_parts.join("\n");

    let summary_msg = Message {
        role: "system".to_string(),
        content: Some(format!("[Previous conversation summary - {} messages compressed]\n{}", to_summarize.len(), summary)),
        tool_calls: None,
        tool_call_id: None,
    };

    history.clear();
    history.push(system_msg);
    history.push(summary_msg);
    history.extend(recent);

    // Aggressive fallback if still over
    let new_tokens = total_context_tokens(history);
    if new_tokens > trigger_threshold && history.len() > 4 {
        history.remove(1); // Drop summary
    }

    true
}

/// Summarize a tool result briefly
fn summarize_tool_result(content: &str, tool_name: Option<&str>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let char_count = content.len();

    // Check for error
    if content.starts_with("Error:") || content.starts_with("error:") {
        let first_line = lines.first().unwrap_or(&"error");
        return format!("Error: {}", safe_truncate(first_line, 80));
    }

    // Brief summary based on tool type
    match tool_name {
        Some("Read") | Some("read_file") => {
            format!("Read {} lines ({} chars)", line_count, char_count)
        }
        Some("Bash") | Some("run_shell_command") => {
            if content.trim().is_empty() {
                "Command completed (no output)".to_string()
            } else if line_count == 1 {
                format!("Output: {}", safe_truncate(content.trim(), 100))
            } else {
                format!("Output: {} lines", line_count)
            }
        }
        Some("Glob") | Some("glob") => {
            let file_count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            format!("Found {} files", file_count)
        }
        Some("Grep") | Some("grep") | Some("search") => {
            let match_count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            format!("Found {} matches", match_count)
        }
        Some("Edit") | Some("edit_file") => {
            if content.contains("✓") {
                "Edit successful".to_string()
            } else {
                safe_truncate(content, 80)
            }
        }
        Some("Write") | Some("write_file") => "File written".to_string(),
        Some("List") | Some("list_dir") => {
            let item_count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            format!("Listed {} items", item_count)
        }
        _ => {
            if line_count <= 2 {
                safe_truncate(content, 150)
            } else {
                format!("{} lines of output", line_count)
            }
        }
    }
}

/// Filter out invalid messages (messages with no content and no tool_calls, and internal thought messages)
pub fn filter_valid_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|msg| {
            // Filter out internal "thought" messages
            if msg.role == "thought" {
                return false;
            }

            // Check content (must be Some and not empty)
            let has_content = msg.content.as_ref().map(|s| !s.is_empty()).unwrap_or(false);
            
            // Check tool_calls (must be Some and not empty)
            let has_tool_calls = msg.tool_calls.as_ref().map(|tc| !tc.is_empty()).unwrap_or(false);

            // Valid message must have either content or tool calls
            has_content || has_tool_calls
        })
        .cloned()
        .collect()
}

// --- App State ---

#[derive(Clone, Debug)]
pub(crate) struct Model {
    name: &'static str,
    context_tokens: usize,
}

#[derive(Clone, Debug)]
pub struct PlanningState {
    pub question: String,
    pub options: Vec<String>,
    pub selected: Vec<bool>,
    pub tool_call_id: String,
    pub tool_call_cmd: Option<(ToolCall, String)>,
    pub list_state: ListState,
}

pub enum AppMode {
    Chat,
    Planning(PlanningState),
    ErrorView,
    Settings(SettingsModalState),
}

pub struct App<'a> {
    pub input: TextArea<'a>,
    pub messages: Vec<Message>,
    pub status_message: String,
    pub is_loading: bool,
    pub rx: mpsc::Receiver<AppEvent>,
    pub tx: mpsc::Sender<AppEvent>,
    pub client: Arc<GrokClient>,
    pub list_state: ListState,
    pub should_quit: bool,
    pub mode: AppMode,
    pub pending_confirmation: Option<String>,
    pub pending_plan: Option<String>,

    // Sandbox mode - restricts tools to cwd
    pub sandbox_enabled: bool,
    pub sandbox_cwd: String,

    // Debug mode - logs all I/O to file
    pub debug_mode: bool,

    // Converse mode - disables tool calls for plain conversation
    pub converse_mode: bool,

    // Autocomplete
    pub autocomplete_active: bool,
    pub autocomplete_options: Vec<&'static str>,
    pub available_models: Vec<Model>,
    pub autocomplete_filtered: Vec<String>,
    pub autocomplete_index: usize,

    pub spinner_index: usize,

    // Task timing
    pub task_start: Option<std::time::Instant>,
    pub last_task_duration: Option<std::time::Duration>,

    // Tool output expansion state (stores tool_call IDs that are expanded)
    pub expanded_tools: std::collections::HashSet<String>,

    // Errors for hidden error window
    pub errors: Vec<String>,

    // Cached API history (filters out "thought" messages)
    pub api_messages: Vec<Message>,

    // Full config for updating allowed commands
    pub config: Config,

    // Current thinking preview (shown in status while model thinks)
    pub thinking_preview: Option<String>,

    // Token usage tracking (from API response)
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,

    // Todo list for task tracking
    pub todos: Vec<TodoItem>,

    // Message history for up/down arrow navigation
    pub message_history: Vec<String>,
    pub history_index: Option<usize>,

    // Track if we should auto-scroll to bottom (set when new messages arrive)
    pub auto_scroll: bool,

    // Megamind brainstorming state
    pub megamind_active: bool,
    pub megamind_current_agent: Option<String>,
    pub megamind_buffer: String,

    // Rate limiter state
    pub rate_limit_window_start: Option<std::time::Instant>,
    pub tokens_used_this_minute: usize,
    pub requests_this_minute: usize,
    pub rate_limit_paused: bool,
    pub rate_limit_resume_at: Option<std::time::Instant>,

    // Ctrl+C tracking for double-tap exit
    pub last_ctrl_c: Option<std::time::Instant>,

    // Cancel flag for interrupting thinking/loading
    pub cancel_requested: bool,
}

pub enum AppEvent {
    NewMessage(Message),
    Token(String),         // For streaming content
    ThinkingToken(String), // For streaming thoughts
    StatusUpdate(String),
    TodoUpdate(Vec<TodoItem>), // Update the todo list
    Error(String),
    Finished,
    PlanningRequest(String, Vec<String>, String, Option<(ToolCall, String)>),
    ConfirmationRequest(String, String),
    BashApprovalRequest(ToolCall, String),
    WebSearchApprovalRequest(ToolCall, String), // (tool_call, query)
    RoleSwitch(String, String), // (from_role, to_role) - for UI display
    UsageUpdate(usize, usize), // (input_tokens, output_tokens) - from API
    // Megamind multi-agent brainstorming
    MegamindToken(String, String),     // (agent_name, token) - streaming
    MegamindAgentDone(String, String), // (agent_name, full_response)
    MegamindComplete(String),          // Final synthesis
    // Rate limiter events
    RateLimitPause(u64),               // Pause duration in seconds
    RateLimitResume,                   // Resume from rate limit pause
}

/// Active role context for multi-model orchestration
#[derive(Debug, Clone)]
pub struct ActiveRole {
    pub name: String,
    pub model: String,
    pub system_prompt: Option<String>,
}

impl<'a> App<'a> {
    pub fn new(client: GrokClient, initial_messages: Vec<Message>, config: &Config, debug: bool) -> App<'a> {
        let (tx, rx) = mpsc::channel();

        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_placeholder_text("Type a message... ( / commands, @ roles )");
        // Minimalist input style
        input.set_block(
            Block::default()
                .title(Span::styled(" ⌨️ Input ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)))
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray)),
        );

        // Get current working directory for sandbox
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());

        // Load persisted context, fall back to filtering initial_messages
        let api_messages = load_context(DEFAULT_CONTEXT_FILE)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| initial_messages.iter().filter(|m| m.role != "thought").cloned().collect());

        App {
            input,
            messages: initial_messages,
            status_message: if config.sandbox_enabled {
                format!("{} - {}", "Ready 🔒", &config.model)
            } else {
                "Ready".to_string()
            },
            is_loading: false,
            rx,
            tx,
            client: Arc::new(client),
            list_state: ListState::default(),
            should_quit: false,
            mode: AppMode::Chat,
            pending_confirmation: None,
            pending_plan: None,

            sandbox_enabled: config.sandbox_enabled,
            sandbox_cwd: cwd,

            debug_mode: debug,
            converse_mode: false,

            autocomplete_active: false,
            autocomplete_options: vec!["/exit", "/plan", "/help", "/clear", "/model", "/sandbox", "/context", "/converse", "/init", "/ignore", "/mm", "/megamind", "/settings"],
            available_models: vec![
                Model { name: "grok-3", context_tokens: 131072 },
                Model { name: "grok-3-mini", context_tokens: 131072 },
                Model { name: "grok-4-1-fast-reasoning", context_tokens: 2000000 },
                Model { name: "grok-4-1-fast-non-reasoning", context_tokens: 2000000 },
                Model { name: "grok-code-fast-1", context_tokens: 256000 },
                Model { name: "grok-4-fast-reasoning", context_tokens: 2000000 },
                Model { name: "grok-4-fast-non-reasoning", context_tokens: 2000000 },
                Model { name: "grok-4-0709", context_tokens: 256000 },
                Model { name: "grok-2-vision-1212", context_tokens: 32768 },
            ],
            autocomplete_filtered: Vec::new(),
            autocomplete_index: 0,

            spinner_index: 0,

            task_start: None,
            last_task_duration: None,

            expanded_tools: std::collections::HashSet::new(),

            errors: Vec::new(),

            api_messages,

            config: config.clone(),

            thinking_preview: None,

            total_input_tokens: 0,
            total_output_tokens: 0,

            todos: Vec::new(),

            message_history: Vec::new(),
            history_index: None,

            auto_scroll: true,

            megamind_active: false,
            megamind_current_agent: None,
            megamind_buffer: String::new(),

            rate_limit_window_start: None,
            tokens_used_this_minute: 0,
            requests_this_minute: 0,
            rate_limit_paused: false,
            rate_limit_resume_at: None,

            last_ctrl_c: None,
            cancel_requested: false,
        }
    }

    pub fn get_current_context(&self) -> usize {
        self.available_models.iter().find(|m| m.name == self.client.get_model()).map(|m| m.context_tokens).unwrap_or(131072)
    }

    /// Toggle expansion of the most recent tool call, or all if none specific
    pub fn toggle_tool_expansion(&mut self) {
        // Find the most recent tool result
        if let Some(tool_msg) = self.messages.iter().rev().find(|m| m.role == "tool") {
            if let Some(id) = &tool_msg.tool_call_id {
                if self.expanded_tools.contains(id) {
                    self.expanded_tools.remove(id);
                } else {
                    self.expanded_tools.insert(id.clone());
                }
            }
        }
    }

    /// Check if a tool call is expanded
    pub fn is_tool_expanded(&self, tool_call_id: &str) -> bool {
        self.expanded_tools.contains(tool_call_id)
    }

    pub fn submit_message(&mut self) {
        if self.is_loading {
            return;
        }
        let content = self.input.lines().join("\n");

        // Handle pending confirmation first (allow empty input to mean 'y')
        if let Some(tool_call_id) = self.pending_confirmation.take() {
            self.pending_plan = None;
            // Empty input or 'y' means confirm
            let feedback = if content.trim().is_empty() {
                "y".to_string()
            } else {
                content.clone()
            };
            // Reset input
            self.reset_input();
            self.handle_confirmation(true, feedback, tool_call_id);
            return;
        }

        if content.trim().is_empty() {
            return;
        }

        // Handle commands
        if content.trim() == "/exit" {
            self.should_quit = true;
            self.reset_input();
            return;
        }

        if content.trim().starts_with("/model") {
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.len() < 2 {
                self.messages.push(Message {
                    role: "system".to_string(),
                    content: Some("Usage: /model <model_name>\nAvailable: grok-3, grok-3-mini, grok-4-fast-reasoning, etc.".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                });
            } else {
                let new_model = parts[1].to_string();
                // Validate model availability
                let model_available = self.available_models.iter().any(|m| m.name == new_model);
                if !model_available {
                    let available_names: Vec<&str> = self.available_models.iter().map(|m| m.name).collect();
                    self.messages.push(Message {
                        role: "system".to_string(),
                        content: Some(format!("❌ Model '{}' is not available to your team.\nAvailable models: {}", new_model, available_names.join(", "))),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                } else if let Some(client) = Arc::get_mut(&mut self.client) {
                    client.update_model(new_model.clone());
                    // Save to config for persistence
                    self.config.model = new_model.clone();
                    save_config(&self.config).ok();
                    self.messages.push(Message {
                        role: "system".to_string(),
                        content: Some(format!("✅ Model changed to: {}", new_model)),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
            self.reset_input();
            return;
        }

        if content.trim() == "/help" {
            let sandbox_status = if self.sandbox_enabled { "ON" } else { "OFF" };
            let converse_status = if self.converse_mode { "ON" } else { "OFF" };
            let rate_limiter_status = if self.config.settings.rate_limiter_enabled { "ON" } else { "OFF" };
            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(format!(
                    "Commands:\n  /plan     - Toggle planning mode\n  /model    - Switch models\n  /sandbox  - Toggle sandbox ({})\n  /converse - Toggle converse mode ({})\n  /context  - Show context usage\n  /settings - Open settings menu (rate limiter: {})\n  /ignore   - Manage .grokignore patterns\n  /clear    - Clear history\n  /init     - Initialize config file with defaults\n  /exit     - Exit\n\nKeys: ↑↓ history, j/k scroll, PageUp/Down fast, Ctrl+O expand\nCtrl+C: clear input (2x to exit), Esc: cancel/clear/exit\nMulti-line: Ctrl+Enter, Shift+Enter, or \\ at end of line",
                    sandbox_status, converse_status, rate_limiter_status
                )),
                tool_calls: None,
                tool_call_id: None,
            });
            self.reset_input();
            return;
        }

        if content.trim() == "/converse" {
            self.converse_mode = !self.converse_mode;
            let status = if self.converse_mode { "enabled" } else { "disabled" };
            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(format!("💬 Converse mode {}. Tool calls are now {}.",
                    status,
                    if self.converse_mode { "disabled" } else { "enabled" }
                )),
                tool_calls: None,
                tool_call_id: None,
            });
            self.reset_input();
            return;
        }

        if content.trim() == "/context" {
            // Use API-reported tokens if available, otherwise estimate
            let (input_tokens, output_tokens) = if self.total_input_tokens > 0 {
                (self.total_input_tokens, self.total_output_tokens)
            } else {
                (total_context_tokens(&self.api_messages), 0)
            };
            let total_tokens = input_tokens + output_tokens;
            let max_context = self.get_current_context();
            let pct = (total_tokens as f64 / max_context as f64 * 100.0) as usize;
            let bar_len = 20;
            let filled = (pct * bar_len / 100).min(bar_len);
            let bar: String = "█".repeat(filled) + &"░".repeat(bar_len - filled);
            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(format!(
                    "Context: {}k↑ + {}k↓ = {}k / {}k tokens ({}%)\n[{}]\n{} messages ({} for API)",
                    input_tokens / 1000,
                    output_tokens / 1000,
                    total_tokens / 1000,
                    max_context / 1000,
                    pct,
                    bar,
                    self.messages.len(),
                    self.api_messages.len()
                )),
                tool_calls: None,
                tool_call_id: None,
            });
            self.reset_input();
            return;
        }

        if content.trim() == "/sandbox" {
            self.sandbox_enabled = !self.sandbox_enabled;

            // Save to config
            self.config.sandbox_enabled = self.sandbox_enabled;
            save_config(&self.config).ok();

            let status = if self.sandbox_enabled {
                format!(
                    "🔒 Sandbox ENABLED - Tools restricted to: {}",
                    self.sandbox_cwd
                )
            } else {
                "🔓 Sandbox DISABLED - Tools have full system access".to_string()
            };
            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(status),
                tool_calls: None,
                tool_call_id: None,
            });
            self.reset_input();
            return;
        }

        if content.trim() == "/plan" {
            // If in planning state, exit it
            let in_planning = matches!(self.mode, AppMode::Planning(_))
                || self.pending_plan.is_some()
                || self.pending_confirmation.is_some();

            if in_planning {
                self.mode = AppMode::Chat;
                self.pending_plan = None;
                self.pending_confirmation = None;
                self.messages.push(Message {
                    role: "system".to_string(),
                    content: Some("Exited planning mode.".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                });
                self.reset_input();
                return;
            }

            // Enter planning mode
            self.reset_input();
            self.input.set_placeholder_text("Describe your goal...");

            self.messages.push(Message {
                role: "system".to_string(),
                content: Some("Planning mode. Describe your goal and I'll help break it down.".to_string()),
                tool_calls: None,
                tool_call_id: None,
            });
            self.api_messages.push(Message {
                role: "system".to_string(),
                content: Some("You are now in INTERACTIVE PLANNING MODE.\n1. Ask the user for their goal.\n2. If clarification is needed, call `AskUser(question, options)`.\n3. Once clear, propose a plan using `ConfirmPlan(plan)`.\n4. Once confirmed, execute the plan autonomously.".to_string()),
                tool_calls: None,
                tool_call_id: None,
            });
            return;
        }

        if content.trim() == "/clear" {
            // Keep the first system message if it exists
            let system_msg = if !self.messages.is_empty() && self.messages[0].role == "system" {
                Some(self.messages[0].clone())
            } else {
                None
            };

            self.messages.clear();
            self.api_messages.clear();
            if let Some(msg) = system_msg {
                self.messages.push(msg.clone());
                self.api_messages.push(msg);
            }
            save_history(&self.messages, DEFAULT_HISTORY_FILE).ok();
            save_context(&self.api_messages, DEFAULT_CONTEXT_FILE).ok();

            self.input = TextArea::default();
            self.input
                .set_placeholder_text("Type a message... ( / for commands )");
            self.input.set_block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
            self.status_message = "History Cleared".to_string();
            self.reset_input();
            return;
        }

        if content.trim() == "/init" {
            let mut results = Vec::new();

            // Initialize config file
            let default_config = Config::default();
            match save_config(&default_config) {
                Ok(_) => results.push("Config initialized at ~/.config/grok-cli/config.json".to_string()),
                Err(e) => results.push(format!("Failed to initialize config: {}", e)),
            }

            // Create .grokignore in current directory
            match crate::tools::create_default_grokignore() {
                Ok(true) => results.push("Created .grokignore with default patterns".to_string()),
                Ok(false) => results.push(".grokignore already exists (skipped)".to_string()),
                Err(e) => results.push(format!("Failed to create .grokignore: {}", e)),
            }

            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(results.join("\n")),
                tool_calls: None,
                tool_call_id: None,
            });
            self.reset_input();
            return;
        }

        if content.trim() == "/settings" {
            // Open settings modal
            self.mode = AppMode::Settings(SettingsModalState::new());
            self.reset_input();
            return;
        }

        // /ignore command - manage .grokignore patterns
        if content.trim().starts_with("/ignore") {
            let parts: Vec<&str> = content.split_whitespace().collect();
            let grokignore_path = std::path::Path::new(".grokignore");

            if parts.len() == 1 {
                // Show current patterns
                let patterns = crate::tools::read_grokignore();
                let exists = grokignore_path.exists();
                let msg = if exists {
                    format!(
                        ".grokignore patterns:\n{}\n\nUsage:\n  /ignore add <pattern>  - Add a pattern\n  /ignore rm <pattern>   - Remove a pattern\n  /ignore init           - Create default .grokignore",
                        patterns.iter().map(|p| format!("  {}", p)).collect::<Vec<_>>().join("\n")
                    )
                } else {
                    format!(
                        "No .grokignore file found. Using defaults:\n{}\n\nUsage:\n  /ignore add <pattern>  - Add a pattern\n  /ignore rm <pattern>   - Remove a pattern\n  /ignore init           - Create default .grokignore",
                        patterns.iter().map(|p| format!("  {}", p)).collect::<Vec<_>>().join("\n")
                    )
                };
                self.messages.push(Message {
                    role: "system".to_string(),
                    content: Some(msg),
                    tool_calls: None,
                    tool_call_id: None,
                });
            } else if parts.len() >= 2 {
                match parts[1] {
                    "init" => {
                        // Create default .grokignore
                        let default_patterns = vec![
                            "# Grok CLI ignore patterns",
                            "# Lines starting with # are comments",
                            "",
                            "# Hidden files/directories",
                            ".*",
                            "",
                            "# Build directories",
                            "target",
                            "node_modules",
                            "__pycache__",
                            ".venv",
                            "venv",
                            "",
                            "# Log files",
                            "*.log",
                        ];
                        match std::fs::write(grokignore_path, default_patterns.join("\n")) {
                            Ok(_) => {
                                self.messages.push(Message {
                                    role: "system".to_string(),
                                    content: Some("Created .grokignore with default patterns".to_string()),
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                            }
                            Err(e) => {
                                self.messages.push(Message {
                                    role: "system".to_string(),
                                    content: Some(format!("Failed to create .grokignore: {}", e)),
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                            }
                        }
                    }
                    "add" if parts.len() >= 3 => {
                        let pattern = parts[2..].join(" ");
                        // Read existing or create new
                        let mut content = if grokignore_path.exists() {
                            std::fs::read_to_string(grokignore_path).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        if !content.is_empty() && !content.ends_with('\n') {
                            content.push('\n');
                        }
                        content.push_str(&pattern);
                        content.push('\n');
                        match std::fs::write(grokignore_path, content) {
                            Ok(_) => {
                                self.messages.push(Message {
                                    role: "system".to_string(),
                                    content: Some(format!("Added '{}' to .grokignore", pattern)),
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                            }
                            Err(e) => {
                                self.messages.push(Message {
                                    role: "system".to_string(),
                                    content: Some(format!("Failed to update .grokignore: {}", e)),
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                            }
                        }
                    }
                    "rm" | "remove" if parts.len() >= 3 => {
                        let pattern = parts[2..].join(" ");
                        if grokignore_path.exists() {
                            match std::fs::read_to_string(grokignore_path) {
                                Ok(content) => {
                                    let new_content: Vec<&str> = content
                                        .lines()
                                        .filter(|line| line.trim() != pattern)
                                        .collect();
                                    match std::fs::write(grokignore_path, new_content.join("\n") + "\n") {
                                        Ok(_) => {
                                            self.messages.push(Message {
                                                role: "system".to_string(),
                                                content: Some(format!("Removed '{}' from .grokignore", pattern)),
                                                tool_calls: None,
                                                tool_call_id: None,
                                            });
                                        }
                                        Err(e) => {
                                            self.messages.push(Message {
                                                role: "system".to_string(),
                                                content: Some(format!("Failed to update .grokignore: {}", e)),
                                                tool_calls: None,
                                                tool_call_id: None,
                                            });
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.messages.push(Message {
                                        role: "system".to_string(),
                                        content: Some(format!("Failed to read .grokignore: {}", e)),
                                        tool_calls: None,
                                        tool_call_id: None,
                                    });
                                }
                            }
                        } else {
                            self.messages.push(Message {
                                role: "system".to_string(),
                                content: Some("No .grokignore file exists".to_string()),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                    }
                    _ => {
                        self.messages.push(Message {
                            role: "system".to_string(),
                            content: Some("Usage:\n  /ignore          - Show current patterns\n  /ignore init     - Create default .grokignore\n  /ignore add <p>  - Add pattern\n  /ignore rm <p>   - Remove pattern".to_string()),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
            }
            self.reset_input();
            return;
        }

        // /mm or /megamind - multi-agent brainstorming
        if content.trim().starts_with("/mm ") || content.trim().starts_with("/megamind ") {
            let topic = content.trim()
                .strip_prefix("/mm ")
                .or_else(|| content.trim().strip_prefix("/megamind "))
                .unwrap_or("")
                .trim()
                .to_string();

            if topic.is_empty() {
                self.messages.push(Message {
                    role: "system".to_string(),
                    content: Some("Usage: /mm <topic>\nExample: /mm How should I structure my API?".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                });
                self.reset_input();
                return;
            }

            self.start_megamind_session(topic);
            self.reset_input();
            return;
        }

        // Standard message
        self.reset_input();

        // Add to message history for up/down arrow navigation
        if !content.trim().is_empty() {
            // Remove any duplicates of this exact message from history
            self.message_history.retain(|msg| msg != &content);
            // Add to the end of history
            self.message_history.push(content.clone());
            // Limit history to last 100 messages
            if self.message_history.len() > 100 {
                self.message_history.remove(0);
            }
            // Reset history index when submitting new message
            self.history_index = None;
        }

        // Check for @role: directive
        let (active_role, message_content) = if let Some(directive) = parse_role_directive(&content) {
            if let Some(role_config) = self.config.get_role(&directive.role) {
                let active = ActiveRole {
                    name: directive.role.clone(),
                    model: role_config.model.clone(),
                    system_prompt: role_config.prompt.clone(),
                };
                (Some(active), directive.content)
            } else {
                // Unknown role, treat as normal message
                (None, content.clone())
            }
        } else {
            (None, content.clone())
        };

        let user_msg = Message {
            role: "user".to_string(),
            content: Some(message_content),
            tool_calls: None,
            tool_call_id: None,
        };
        self.messages.push(user_msg.clone());
        save_history(&self.messages, DEFAULT_HISTORY_FILE).ok();
        self.api_messages.push(user_msg);

        // Context management - compress if too large
        self.compress_context_if_needed();
        save_context(&self.api_messages, DEFAULT_CONTEXT_FILE).ok();

        self.is_loading = true;
        self.task_start = Some(std::time::Instant::now());

        // Update status to show role if active
        if let Some(ref role) = active_role {
            self.status_message = format!("@{} thinking...", role.name);
        } else {
            self.status_message = "Thinking...".to_string();
        }

        // Auto-scroll to user message
        self.list_state
            .select(Some(self.messages.len().saturating_sub(1)));

        // Pre-flight rate limit check (synchronous, before spawning task)
        let rate_limit_config = self.config.get_rate_limit(self.client.get_model()).cloned();
        let rate_limiter_enabled = self.config.settings.rate_limiter_enabled;

        if rate_limiter_enabled {
            if let Some(ref config) = rate_limit_config {
                // Check if we're approaching 70% of limits (more conservative than the 80% in-flight check)
                let tpm_threshold = config.tpm * 70 / 100;
                let rpm_threshold = config.rpm * 70 / 100;

                if self.tokens_used_this_minute >= tpm_threshold || self.requests_this_minute >= rpm_threshold {
                    // Set rate limit state
                    self.rate_limit_paused = true;
                    self.rate_limit_resume_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
                    self.status_message = format!(
                        "⏸ Rate limit: {:.0}% TPM, {:.0}% RPM - waiting for window reset",
                        self.tokens_used_this_minute as f64 / config.tpm as f64 * 100.0,
                        self.requests_this_minute as f64 / config.rpm as f64 * 100.0
                    );
                    self.is_loading = false;

                    // Add a system message so user knows what happened
                    self.messages.push(Message {
                        role: "system".to_string(),
                        content: Some(format!(
                            "⏸ Rate limit approaching ({:.0}% of {} TPM). Message queued - will send when limit resets.",
                            self.tokens_used_this_minute as f64 / config.tpm as f64 * 100.0,
                            config.tpm
                        )),
                        tool_calls: None,
                        tool_call_id: None,
                    });

                    // Store the pending message for retry (re-add user message that was just added)
                    // The message is already in api_messages, so we just need to return and let the
                    // rate limit window reset. User can manually retry or we could auto-retry.
                    return;
                }
            }
        }

        let client = self.client.clone();
        let tx = self.tx.clone();
        let history = self.api_messages.clone();
        let sandbox = if self.sandbox_enabled {
            Some(self.sandbox_cwd.clone())
        } else {
            None
        };
        let debug = self.debug_mode;
        let converse = self.converse_mode;
        let allowed_commands = self.config.allowed_commands
            .get(&self.sandbox_cwd)
            .cloned()
            .unwrap_or_default();
        let max_context = self.get_current_context();
        let roles = self.config.roles.clone();
        let tokens_used = self.tokens_used_this_minute;
        let requests_used = self.requests_this_minute;

        // Increment request counter immediately (before the actual request)
        // This provides more accurate rate limiting for rapid successive requests
        self.requests_this_minute += 1;

        tokio::spawn(async move {
            process_conversation(client, history, tx, sandbox, debug, allowed_commands, max_context, roles, active_role, converse, rate_limit_config, rate_limiter_enabled, tokens_used, requests_used).await;
        });
    }

    /// Compress old context when exceeding threshold
    fn compress_context_if_needed(&mut self) {
        let tokens = total_context_tokens(&self.api_messages);
        let max_context = self.get_current_context();
        let target_threshold = max_context * 3 / 10; // Target 30% after compression
        let trigger_threshold = max_context * 6 / 10; // Trigger at 60%

        if tokens < trigger_threshold {
            return;
        }

        // Calculate how many tokens we need to free (for future adaptive logic)
        let _tokens_to_free = tokens.saturating_sub(target_threshold);

        // Dynamic keep_recent: keep more if we have room, fewer if tight
        let base_keep = 6;
        let max_keep = 20;
        let available_for_recent = max_context * 3 / 10; // 30% for recent messages
        let avg_msg_tokens = tokens / self.api_messages.len().max(1);
        let keep_recent = if avg_msg_tokens > 0 {
            (available_for_recent / avg_msg_tokens).clamp(base_keep, max_keep)
        } else {
            base_keep
        };

        if self.api_messages.len() <= keep_recent + 1 {
            return; // Not enough to compress
        }

        // Build a map of tool_call_id -> tool_name for summarizing tool results
        let mut tool_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for msg in &self.api_messages {
            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    tool_names.insert(tc.id.clone(), tc.function.name.clone());
                }
            }
        }

        // Extract messages to summarize (skip system prompt, keep recent)
        let to_summarize = &self.api_messages[1..self.api_messages.len() - keep_recent];
        if to_summarize.is_empty() {
            return;
        }

        // Build summary of old conversation
        let mut summary_parts: Vec<String> = Vec::new();
        let max_summary_chars = 8000; // Cap summary at ~2k tokens
        let mut current_chars = 0;

        for msg in to_summarize {
            if current_chars >= max_summary_chars {
                break;
            }

            let part = match msg.role.as_str() {
                "user" => {
                    msg.content.as_ref().map(|c| {
                        format!("User: {}", safe_truncate(c, 120))
                    })
                }
                "assistant" => {
                    let mut parts = Vec::new();
                    if let Some(tcs) = &msg.tool_calls {
                        let tools: Vec<_> = tcs.iter().map(|t| t.function.name.as_str()).collect();
                        if !tools.is_empty() {
                            parts.push(format!("Assistant used: {}", tools.join(", ")));
                        }
                    }
                    if let Some(c) = &msg.content {
                        if !c.is_empty() {
                            parts.push(format!("Assistant: {}", safe_truncate(c, 150)));
                        }
                    }
                    if parts.is_empty() { None } else { Some(parts.join(" | ")) }
                }
                "tool" => {
                    // Include brief tool result summaries
                    msg.content.as_ref().map(|c| {
                        let tool_name = msg.tool_call_id.as_ref()
                            .and_then(|id| tool_names.get(id))
                            .map(|s| s.as_str());
                        format!("  → {}", summarize_tool_result(c, tool_name))
                    })
                }
                _ => None,
            };

            if let Some(p) = part {
                current_chars += p.len();
                summary_parts.push(p);
            }
        }

        // If summary is still too large, keep only the most recent parts
        while current_chars > max_summary_chars && summary_parts.len() > 10 {
            let removed = summary_parts.remove(0);
            current_chars = current_chars.saturating_sub(removed.len());
        }

        // Create compressed history
        let system_msg = self.api_messages[0].clone();
        let recent: Vec<_> = self.api_messages[self.api_messages.len() - keep_recent..].to_vec();
        let summary = summary_parts.join("\n");

        let summary_msg = Message {
            role: "system".to_string(),
            content: Some(format!("[Previous conversation summary - {} messages compressed]\n{}", to_summarize.len(), summary)),
            tool_calls: None,
            tool_call_id: None,
        };

        // Rebuild api_messages: system + summary + recent
        self.api_messages = vec![system_msg, summary_msg];
        self.api_messages.extend(recent);

        // Check if we're still over and need more aggressive compression
        let new_tokens = total_context_tokens(&self.api_messages);
        if new_tokens > trigger_threshold && self.api_messages.len() > 4 {
            // Drop the summary entirely if still too large
            self.api_messages.remove(1);
            let final_tokens = total_context_tokens(&self.api_messages);
            self.status_message = format!(
                "Context aggressively compressed: {}k → {}k tokens",
                tokens / 1000,
                final_tokens / 1000
            );
        } else {
            self.status_message = format!(
                "Context compressed: {}k → {}k tokens (kept {} recent)",
                tokens / 1000,
                new_tokens / 1000,
                keep_recent
            );
        }
    }

    pub fn handle_planning_selection(&mut self) {
        if let AppMode::Planning(state) = &self.mode {
            // Get selections - if none explicitly selected, use the highlighted item
            let mut selections: Vec<String> = state
                .options
                .iter()
                .enumerate()
                .filter(|(i, _)| state.selected[*i])
                .map(|(_, v)| v.clone())
                .collect();

            // If nothing explicitly selected, use the currently highlighted option
            if selections.is_empty() {
                if let Some(idx) = state.list_state.selected() {
                    if idx < state.options.len() {
                        selections.push(state.options[idx].clone());
                    }
                }
            }

            // Check if this is a bash command approval
            if let Some((ref tc, ref cmd)) = state.tool_call_cmd {
                let approved = selections.iter().any(|s| s.to_lowercase().contains("approve"));
                let always_approve = selections.iter().any(|s| s.to_lowercase().contains("always"));
                let tc_id = tc.id.clone();
                let tc_clone = tc.clone();
                let cmd_clone = cmd.clone();

                self.mode = AppMode::Chat;

                if approved || always_approve {
                    // If "Always Approve", save to config
                    if always_approve {
                        self.config.allow_command(cmd_clone, self.sandbox_cwd.clone());
                        save_config(&self.config).ok();
                        self.status_message = format!("Command saved to allowed list for {}", self.sandbox_cwd);
                    }

                    // Execute the bash command
                    let result = execute_tool(
                        &tc_clone.function.name,
                        &tc_clone.function.arguments,
                        if self.sandbox_enabled { Some(&self.sandbox_cwd) } else { None },
                    );
                    let tool_msg = Message {
                        role: "tool".to_string(),
                        content: Some(result),
                        tool_calls: None,
                        tool_call_id: Some(tc_id),
                    };
                    self.messages.push(tool_msg.clone());
                    save_history(&self.messages, DEFAULT_HISTORY_FILE).ok();
                    self.api_messages.push(tool_msg);
                    self.compress_context_if_needed();
                    save_context(&self.api_messages, DEFAULT_CONTEXT_FILE).ok();
                } else {
                    // Reject
                    let tool_msg = Message {
                        role: "tool".to_string(),
                        content: Some("Command rejected by user.".to_string()),
                        tool_calls: None,
                        tool_call_id: Some(tc_id),
                    };
                    self.messages.push(tool_msg.clone());
                    save_history(&self.messages, DEFAULT_HISTORY_FILE).ok();
                    self.api_messages.push(tool_msg);
                    self.compress_context_if_needed();
                    save_context(&self.api_messages, DEFAULT_CONTEXT_FILE).ok();
                }

                // Continue conversation
                self.is_loading = true;
                self.task_start = Some(std::time::Instant::now());
                self.status_message = "Thinking...".to_string();
                let client = self.client.clone();
                let tx = self.tx.clone();
                let history = self.api_messages.clone();
                let sandbox = if self.sandbox_enabled {
                    Some(self.sandbox_cwd.clone())
                } else {
                    None
                };
                let debug = self.debug_mode;
                let converse = self.converse_mode;
                let allowed_commands = self.config.allowed_commands
                    .get(&self.sandbox_cwd)
                    .cloned()
                    .unwrap_or_default();
                let max_context = self.get_current_context();
                let roles = self.config.roles.clone();
                let rate_limit_config = self.config.get_rate_limit(self.client.get_model()).cloned();
                let rate_limiter_enabled = self.config.settings.rate_limiter_enabled;
                let tokens_used = self.tokens_used_this_minute;
                let requests_used = self.requests_this_minute;
                tokio::spawn(async move {
                    process_conversation(client, history, tx, sandbox, debug, allowed_commands, max_context, roles, None, converse, rate_limit_config, rate_limiter_enabled, tokens_used, requests_used).await;
                });
                return;
            }

            // Regular planning selection
            let response = format!("User selected: {:?}", selections);
            let id = state.tool_call_id.clone();
            self.respond_with_tool_output(id, response);
        }
    }

    pub fn handle_confirmation(&mut self, accepted: bool, feedback: String, tool_call_id: String) {
        let content = if accepted {
            if feedback.trim().is_empty() || feedback.trim().to_lowercase() == "y" {
                "Plan Confirmed. Proceed.".to_string()
            } else if feedback.trim().to_lowercase() == "n" {
                "Plan Rejected by user.".to_string()
            } else {
                format!("Plan Feedback: {}", feedback)
            }
        } else {
            "Plan Rejected by user.".to_string()
        };

        self.respond_with_tool_output(tool_call_id, content);
    }

    fn respond_with_tool_output(&mut self, id: String, content: String) {
        let tool_msg = Message {
            role: "tool".to_string(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(id),
        };
        self.messages.push(tool_msg.clone());
        save_history(&self.messages, DEFAULT_HISTORY_FILE).ok();
        self.api_messages.push(tool_msg);
        self.compress_context_if_needed();
        save_context(&self.api_messages, DEFAULT_CONTEXT_FILE).ok();
        self.mode = AppMode::Chat;
        self.is_loading = true;
        self.task_start = Some(std::time::Instant::now());

        self.reset_input();

        let client = self.client.clone();
        let tx = self.tx.clone();
        // Filter out "thought" messages - they're for display only, not valid API roles
        let history = self.api_messages.clone();
        let sandbox = if self.sandbox_enabled {
            Some(self.sandbox_cwd.clone())
        } else {
            None
        };
        let debug = self.debug_mode;
        let converse = self.converse_mode;
        let allowed_commands = self.config.allowed_commands
            .get(&self.sandbox_cwd)
            .cloned()
            .unwrap_or_default();
        let max_context = self.get_current_context();
        let roles = self.config.roles.clone();
        let rate_limit_config = self.config.get_rate_limit(self.client.get_model()).cloned();
        let rate_limiter_enabled = self.config.settings.rate_limiter_enabled;
        let tokens_used = self.tokens_used_this_minute;
        let requests_used = self.requests_this_minute;
        tokio::spawn(async move {
            process_conversation(client, history, tx, sandbox, debug, allowed_commands, max_context, roles, None, converse, rate_limit_config, rate_limiter_enabled, tokens_used, requests_used).await;
        });
    }

    pub fn update_autocomplete(&mut self) {
        let content = self.input.lines().join("\n");
        if content.starts_with('/') {
            self.autocomplete_active = true;
            if let Some(query) = content.strip_prefix("/model ") {
                self.autocomplete_filtered = self
                    .available_models
                    .iter()
                    .filter(|m| m.name.starts_with(query))
                    .map(|m| format!("/model {}", m.name))
                    .collect();
            } else if !content.contains(' ') {
                let query = &content;
                self.autocomplete_filtered = self
                    .autocomplete_options
                    .iter()
                    .filter(|opt| opt.starts_with(query))
                    .map(|s| s.to_string())
                    .collect();
            } else {
                self.autocomplete_active = false;
            }

            if self.autocomplete_active {
                if self.autocomplete_index >= self.autocomplete_filtered.len() {
                    self.autocomplete_index = 0;
                }
                if self.autocomplete_filtered.is_empty() {
                    self.autocomplete_active = false;
                }
            }
        } else if content.starts_with('@') {
            // Role autocomplete - show available roles with their models
            let after_at = &content[1..];

            // Check if role is already complete (has colon followed by content)
            if let Some(colon_pos) = after_at.find(':') {
                let after_colon = &after_at[colon_pos + 1..];
                if !after_colon.trim().is_empty() {
                    // Role is complete and user is typing message - don't show autocomplete
                    self.autocomplete_active = false;
                    return;
                }
            }

            self.autocomplete_active = true;

            // Extract the role query (everything after @ before any space or colon)
            let query_end = after_at.find(|c| c == ' ' || c == ':').unwrap_or(after_at.len());
            let query = &after_at[..query_end].to_lowercase();

            // Filter and format roles - format: "@role: (model)"
            // The model part will be stripped on selection
            self.autocomplete_filtered = self.config.roles
                .iter()
                .filter(|(name, _)| name.starts_with(query))
                .map(|(name, role)| format!("@{}:  ({})", name, role.model))
                .collect();

            // Sort alphabetically
            self.autocomplete_filtered.sort();

            if self.autocomplete_index >= self.autocomplete_filtered.len() {
                self.autocomplete_index = 0;
            }
            if self.autocomplete_filtered.is_empty() {
                self.autocomplete_active = false;
            }
        } else {
            self.autocomplete_active = false;
        }
    }

    pub fn reset_input(&mut self) {
        self.input = TextArea::default();
        self.input.set_placeholder_text("Type a message... ( / commands, @ roles )");
        self.input.set_block(
            Block::default()
                .title(Span::styled(" ⌨️ Input ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)))
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        // Reset history index when resetting input
        self.history_index = None;
    }

    /// Navigate through message history with up/down arrows
    pub fn navigate_history(&mut self, direction_up: bool) {
        if self.message_history.is_empty() {
            return;
        }

        let new_index = match self.history_index {
            None => {
                // Not currently navigating - start from the most recent message
                if direction_up {
                    Some(self.message_history.len() - 1)
                } else {
                    // Down when not navigating - do nothing
                    None
                }
            }
            Some(current_idx) => {
                if direction_up {
                    if current_idx > 0 {
                        Some(current_idx - 1)
                    } else {
                        // At the beginning - stay at current
                        Some(current_idx)
                    }
                } else {
                    if current_idx < self.message_history.len() - 1 {
                        Some(current_idx + 1)
                    } else {
                        // At the end - go back to empty input
                        None
                    }
                }
            }
        };

        self.history_index = new_index;

        // Update the input field
        if let Some(idx) = new_index {
            self.input = TextArea::default();
            self.input.set_cursor_line_style(Style::default());
            self.input.set_placeholder_text("Type a message... ( / commands, @ roles )");
            self.input.set_block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
            self.input.insert_str(&self.message_history[idx]);
            // Move cursor to end of input
            let lines = self.input.lines();
            if !lines.is_empty() {
                self.input.move_cursor(tui_textarea::CursorMove::End);
            }
        } else {
            // No history selected - empty input
            self.reset_input();
        }
    }

    /// Start a megamind multi-agent brainstorming session
    pub fn start_megamind_session(&mut self, topic: String) {
        self.megamind_active = true;
        self.megamind_current_agent = None;
        self.megamind_buffer.clear();
        self.is_loading = true;
        self.task_start = Some(std::time::Instant::now());
        self.status_message = "Megamind: Starting brainstorm...".to_string();

        // Show topic in messages
        self.messages.push(Message {
            role: "system".to_string(),
            content: Some(format!("=== Megamind Brainstorm ===\nTopic: {}", topic)),
            tool_calls: None,
            tool_call_id: None,
        });
        self.auto_scroll = true;

        let client = self.client.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            crate::megamind::run_megamind(client, topic, tx).await;
        });
    }
}

pub async fn process_conversation(
    client: Arc<GrokClient>,
    history: Vec<Message>,
    tx: mpsc::Sender<AppEvent>,
    sandbox_cwd: Option<String>,
    debug: bool,
    allowed_commands: Vec<String>,
    max_context: usize,
    roles: std::collections::HashMap<String, crate::config::ModelRole>,
    active_role: Option<ActiveRole>,
    converse_mode: bool,
    rate_limit_config: Option<crate::settings::RateLimitConfig>,
    rate_limiter_enabled: bool,
    tokens_used_this_minute: usize,
    requests_this_minute: usize,
) {
    // If we have an active role, update status and optionally inject system prompt
    if let Some(ref role) = active_role {
        let _ = tx.send(AppEvent::StatusUpdate(format!("@{} thinking...", role.name)));
    }

    // Make history mutable for potential modifications
    let mut history = history;

    // Inject role-specific system prompt if available
    if let Some(ref role) = active_role {
        if let Some(ref prompt) = role.system_prompt {
            // Add role context as a system message after the main system prompt
            let role_context = Message {
                role: "system".to_string(),
                content: Some(format!("[Role: @{}]\n{}", role.name, prompt)),
                tool_calls: None,
                tool_call_id: None,
            };
            // Insert after the first system message
            if history.len() > 1 {
                history.insert(1, role_context);
            } else {
                history.push(role_context);
            }
        }
    }

    // Debug logging helper
    let mut debug_file = if debug {
        Some(BufWriter::new(OpenOptions::new().create(true).append(true).open("grok-debug.log").unwrap()))
    } else { None };

    let mut log_debug = |msg: &str| {
        if let Some(ref mut file) = debug_file {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            writeln!(file, "[{}] {}", timestamp, msg).ok();
        }
    };

    // Start transaction for this request - rollback on error, commit on success
    crate::transactions::begin_transaction();
    log_debug("Transaction started for request");

    // Retry counter for empty responses
    let mut empty_response_retries = 0;
    const MAX_EMPTY_RETRIES: u8 = 2;

    // Log the initial request
    if debug {
        log_debug("=== NEW API REQUEST ===");
        log_debug(&format!("History ({} messages, after filtering):", history.len()));
        for (i, msg) in history.iter().enumerate() {
            log_debug(&format!("  [{}] role={}, content={:?}, tool_calls={:?}, tool_call_id={:?}",
                i, msg.role,
                msg.content.as_ref().map(|c| safe_truncate(c, 200)),
                msg.tool_calls.as_ref().map(|tc| tc.iter().map(|t| &t.function.name).collect::<Vec<_>>()),
                msg.tool_call_id
            ));
        }
    }
    loop {
        // Compress history if approaching context limit (mid-thinking protection)
        if compress_history_if_needed(&mut history, max_context) {
            log_debug("Context compressed mid-conversation");
            let _ = tx.send(AppEvent::StatusUpdate("Context compressed...".to_string()));
        }

        let _ = tx.send(AppEvent::StatusUpdate("Thinking...".to_string()));
        log_debug("--- Starting API call ---");

        // Rate limiter check (if enabled and configured for this model)
        if rate_limiter_enabled {
            if let Some(ref config) = rate_limit_config {
                // Check if we're approaching 80% of TPM limit
                let tpm_threshold = config.tpm * 80 / 100;
                let rpm_threshold = config.rpm * 80 / 100;

                if tokens_used_this_minute >= tpm_threshold || requests_this_minute >= rpm_threshold {
                    log_debug(&format!(
                        "RATE LIMIT: tokens {}/{} ({}%), requests {}/{} ({}%) - pausing 60s",
                        tokens_used_this_minute, config.tpm,
                        tokens_used_this_minute * 100 / config.tpm.max(1),
                        requests_this_minute, config.rpm,
                        requests_this_minute * 100 / config.rpm.max(1)
                    ));
                    let _ = tx.send(AppEvent::RateLimitPause(60));
                    let _ = tx.send(AppEvent::StatusUpdate(format!(
                        "Rate limit approaching - pausing 60s ({:.0}% TPM, {:.0}% RPM)",
                        tokens_used_this_minute as f64 / config.tpm as f64 * 100.0,
                        requests_this_minute as f64 / config.rpm as f64 * 100.0
                    )));

                    // Sleep for 60 seconds
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

                    let _ = tx.send(AppEvent::RateLimitResume);
                    let _ = tx.send(AppEvent::StatusUpdate("Rate limit cleared - resuming...".to_string()));
                    log_debug("RATE LIMIT: pause complete, resuming");
                }
            }
        }

        // Determine which model to use
        let model_to_use = active_role.as_ref()
            .map(|r| r.model.as_str())
            .unwrap_or(client.get_model());

        // Start streaming request (no tools in converse mode)
        let tools = if converse_mode { vec![] } else { get_tool_definitions() };
        match client
            .chat_completion_stream_with_model(history.clone(), tools, model_to_use)
            .await
        {
            Ok(mut response) => {
                log_debug("API response stream started");
                let mut full_content = String::with_capacity(4096);
                let mut tool_calls_buffer: Vec<ToolCall> = Vec::new();
                let mut sse_buffer = String::new(); // Buffer for incomplete SSE lines

                // Stream processing
                while let Ok(Some(chunk)) = response.chunk().await {
                    let text = String::from_utf8_lossy(&chunk);
                    sse_buffer.push_str(&text);

                    // Process complete lines only (prevents truncated JSON)
                    while let Some(newline_pos) = sse_buffer.find('\n') {
                        let line = sse_buffer[..newline_pos].trim().to_string();
                        sse_buffer = sse_buffer[newline_pos + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        let json_str = match line.strip_prefix("data: ") {
                            Some(s) => s,
                            None => continue,
                        };

                        if json_str == "[DONE]" {
                            sse_buffer.clear();
                            break;
                        }

                        match serde_json::from_str::<Value>(json_str) {
                            Ok(val) => {
                                // Debug: log ALL chunks to see what's coming
                                log_debug(&format!("CHUNK: {}", safe_truncate(&val.to_string(), 800)));

                                // Check for usage info (comes with include_usage: true)
                                if let Some(usage) = val.get("usage") {
                                    let prompt_tokens = usage.get("prompt_tokens")
                                        .and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                    let completion_tokens = usage.get("completion_tokens")
                                        .and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                    log_debug(&format!("USAGE: prompt={}, completion={}, total={}",
                                        prompt_tokens, completion_tokens, prompt_tokens + completion_tokens));
                                    let _ = tx.send(AppEvent::UsageUpdate(prompt_tokens, completion_tokens));
                                }

                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array())
                                {
                                    if let Some(choice) = choices.first() {
                                        // Try delta first (streaming), then message (final)
                                        let delta = choice.get("delta")
                                            .or_else(|| choice.get("message"));

                                        if let Some(delta) = delta {
                                            // 0. Thoughts (reasoning_content or thinking)
                                            let thought = delta.get("reasoning_content")
                                                .or_else(|| delta.get("thinking"))
                                                .and_then(|c| c.as_str());
                                            if let Some(thought) = thought {
                                                let _ = tx.send(AppEvent::ThinkingToken(
                                                    thought.to_string(),
                                                ));
                                            }

                                            // 1. Content
                                            if let Some(content) =
                                                delta.get("content").and_then(|c| c.as_str())
                                            {
                                                full_content.push_str(content);
                                                let _ =
                                                    tx.send(AppEvent::Token(content.to_string()));
                                            }

                                            // 2. Tool Calls
                                            if let Some(tcs) =
                                                delta.get("tool_calls").and_then(|t| t.as_array())
                                            {
                                                for tc_val in tcs {
                                                    let index =
                                                        tc_val["index"].as_u64().unwrap_or(0)
                                                            as usize;

                                                    // Ensure buffer size
                                                    if index >= tool_calls_buffer.len() {
                                                        tool_calls_buffer.resize(
                                                            index + 1,
                                                            ToolCall {
                                                                id: "".to_string(),
                                                                r#type: "function".to_string(),
                                                                function:
                                                                    crate::tools::FunctionCall {
                                                                        name: "".to_string(),
                                                                        arguments: "".to_string(),
                                                                    },
                                                            },
                                                        );
                                                    }

                                                    if let Some(id) =
                                                        tc_val.get("id").and_then(|s| s.as_str())
                                                    {
                                                        tool_calls_buffer[index].id =
                                                            id.to_string();
                                                    }

                                                    if let Some(func) = tc_val.get("function") {
                                                        if let Some(name) = func
                                                            .get("name")
                                                            .and_then(|s| s.as_str())
                                                        {
                                                            tool_calls_buffer[index]
                                                                .function
                                                                .name
                                                                .push_str(name);
                                                        }
                                                        if let Some(args) = func
                                                            .get("arguments")
                                                            .and_then(|s| s.as_str())
                                                        {
                                                            tool_calls_buffer[index]
                                                                .function
                                                                .arguments
                                                                .push_str(args);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log_debug(&format!("JSON PARSE ERROR: {} for: {}", e, safe_truncate(json_str, 200)));
                            }
                        }
                    }
                }

                // Stream finished.
                log_debug("Stream finished");

                // Construct final message
                let assistant_msg = Message {
                    role: "assistant".to_string(),
                    content: if full_content.is_empty() {
                        None
                    } else {
                        Some(full_content.clone())
                    },
                    tool_calls: if tool_calls_buffer.is_empty() {
                        None
                    } else {
                        Some(tool_calls_buffer.clone())
                    },
                    tool_call_id: None,
                };

                // Log the assistant response
                log_debug(&format!("ASSISTANT RESPONSE: content={:?}, tool_calls={}",
                    safe_truncate(&full_content, 500),
                    tool_calls_buffer.len()
                ));
                for tc in &tool_calls_buffer {
                    log_debug(&format!("  TOOL_CALL: id={}, name={}, args={}",
                        tc.id, tc.function.name,
                        safe_truncate(&tc.function.arguments, 300)
                    ));
                }

                // Only add valid messages (must have content or tool_calls)
                let is_valid = assistant_msg.content.is_some() || assistant_msg.tool_calls.is_some();
                if is_valid {
                    // Reset retry counter on successful response
                    empty_response_retries = 0;
                    // Send NewMessage to ensure the state is finalized (replacing the streaming partial)
                    let _ = tx.send(AppEvent::NewMessage(assistant_msg.clone()));
                    history.push(assistant_msg.clone());
                } else {
                    empty_response_retries += 1;
                    log_debug(&format!("WARNING: Empty assistant message (retry {}/{})",
                        empty_response_retries, MAX_EMPTY_RETRIES));

                    if empty_response_retries >= MAX_EMPTY_RETRIES {
                        // Max retries reached, inform user and break
                        let _ = tx.send(AppEvent::StatusUpdate("Model returned empty response".to_string()));
                        let _ = tx.send(AppEvent::NewMessage(Message {
                            role: "assistant".to_string(),
                            content: Some("⚠️ The model returned an empty response. This may be due to safety filters or API issues. Try rephrasing your request.".to_string()),
                            tool_calls: None,
                            tool_call_id: None,
                        }));

                        // Rollback transaction on empty response error
                        if let Err(e) = crate::transactions::rollback_transaction() {
                            log_debug(&format!("Transaction rollback failed: {}", e));
                        }

                        break;
                    }

                    // Retry - add a nudge to the history to encourage response
                    let _ = tx.send(AppEvent::StatusUpdate(format!("Retrying ({}/{})...",
                        empty_response_retries, MAX_EMPTY_RETRIES)));
                    history.push(Message {
                        role: "user".to_string(),
                        content: Some("Please continue with your response.".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    continue;
                }

                if !tool_calls_buffer.is_empty() {
                    for tc in tool_calls_buffer {
                        if tc.function.name == "ask_multiple_choice" || tc.function.name == "AskUser" {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                            let question = args["question"]
                                .as_str()
                                .unwrap_or("Select options")
                                .to_string();
                            let options = args["options"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .map(|v| v.as_str().unwrap_or("").to_string())
                                        .collect()
                                })
                                .unwrap_or_default();

                            let _ = tx.send(AppEvent::PlanningRequest(question, options, tc.id, None));
                            return;
                        }

                        if tc.function.name == "confirm_plan" || tc.function.name == "ConfirmPlan" {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                            let plan = args["plan"].as_str().unwrap_or("").to_string();

                            let _ = tx.send(AppEvent::ConfirmationRequest(plan, tc.id));
                            return;
                        }

                        if tc.function.name == "Bash" || tc.function.name == "run_shell_command" {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                            let command = args["command"].as_str().unwrap_or("");
                            if allowed_commands.is_empty() || !allowed_commands.contains(&command.to_string()) {
                                log_debug(&format!("Bash command '{}' not allowed, requesting approval", command));
                                let _ = tx.send(AppEvent::BashApprovalRequest(tc, command.to_string()));
                                return;
                            }
                        }

                        // WebSearch requires approval
                        if tc.function.name == "WebSearch" || tc.function.name == "web_search" {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                            let query = args["query"].as_str().unwrap_or("");
                            log_debug(&format!("WebSearch '{}' requesting approval", query));
                            let _ = tx.send(AppEvent::WebSearchApprovalRequest(tc, query.to_string()));
                            return;
                        }

                        // TodoWrite - update the todo list (no filesystem action)
                        if tc.function.name == "TodoWrite" {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                            if let Some(todos_arr) = args.get("todos").and_then(|v| v.as_array()) {
                                let todos: Vec<TodoItem> = todos_arr.iter().filter_map(|item| {
                                    let content = item.get("content")?.as_str()?.to_string();
                                    let status_str = item.get("status")?.as_str()?;
                                    let active_form = item.get("activeForm")?.as_str()?.to_string();
                                    let status = match status_str {
                                        "in_progress" => TodoStatus::InProgress,
                                        "completed" => TodoStatus::Completed,
                                        _ => TodoStatus::Pending,
                                    };
                                    Some(TodoItem { content, status, active_form })
                                }).collect();

                                log_debug(&format!("TodoWrite: {} items", todos.len()));
                                let _ = tx.send(AppEvent::TodoUpdate(todos));
                            }

                            // Add tool result
                            let tool_msg = Message {
                                role: "tool".to_string(),
                                content: Some("Todo list updated.".to_string()),
                                tool_calls: None,
                                tool_call_id: Some(tc.id.clone()),
                            };
                            history.push(tool_msg.clone());
                            let _ = tx.send(AppEvent::NewMessage(tool_msg));
                            continue;
                        }

                        let _ = tx.send(AppEvent::StatusUpdate(format!(
                            "Running tool: {}...",
                            tc.function.name
                        )));
                        log_debug(&format!("Executing tool: {}", tc.function.name));

                        let result = execute_tool(
                            &tc.function.name,
                            &tc.function.arguments,
                            sandbox_cwd.as_deref(),
                        );

                        // Log tool result (debug mode only, no terminal spam)
                        let is_error = result.starts_with("Error:") || result.starts_with("error:");
                        if is_error {
                            log_debug(&format!("TOOL ERROR for {}: {}", tc.function.name, result));
                        } else {
                            log_debug(&format!("TOOL RESULT for {}: {}",
                                tc.function.name,
                                safe_truncate(&result, 500)
                            ));
                        }

                        let tool_msg = Message {
                            role: "tool".to_string(),
                            content: Some(result),
                            tool_calls: None,
                            tool_call_id: Some(tc.id),
                        };

                        history.push(tool_msg.clone());
                        let _ = tx.send(AppEvent::NewMessage(tool_msg));
                    }
                } else {
                    // No tool calls - check for handoff directive in assistant's response
                    if let Some(handoff) = find_handoff_directive(&full_content) {
                        if let Some(role_config) = roles.get(&handoff.role) {
                            log_debug(&format!("HANDOFF detected: @{} -> content: {}", handoff.role, safe_truncate(&handoff.content, 100)));

                            // Notify UI of role switch
                            let from_role = active_role.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| "default".to_string());
                            let _ = tx.send(AppEvent::RoleSwitch(from_role, handoff.role.clone()));

                            // Create new active role
                            let new_role = ActiveRole {
                                name: handoff.role.clone(),
                                model: role_config.model.clone(),
                                system_prompt: role_config.prompt.clone(),
                            };

                            // Add a user message with the handoff content to continue the conversation
                            let handoff_msg = Message {
                                role: "user".to_string(),
                                content: Some(format!("Continue with the following task:\n{}", handoff.content)),
                                tool_calls: None,
                                tool_call_id: None,
                            };
                            history.push(handoff_msg.clone());
                            let _ = tx.send(AppEvent::NewMessage(handoff_msg));

                            // Recursively call with the new role
                            return Box::pin(process_conversation(
                                client,
                                history,
                                tx,
                                sandbox_cwd,
                                debug,
                                allowed_commands,
                                max_context,
                                roles,
                                Some(new_role),
                                converse_mode,
                                rate_limit_config,
                                rate_limiter_enabled,
                                tokens_used_this_minute,
                                requests_this_minute,
                            )).await;
                        }
                    }
                    break;
                }
            }
            Err(e) => {
                let error_str = e.to_string();
                log_debug(&format!("API ERROR: {}", error_str));

                // Parse and provide user-friendly error messages
                let user_message = if error_str.contains("SAFETY_CHECK") || error_str.contains("violates usage guidelines") {
                    "⚠️ Request blocked by safety filters. Try rephrasing your message.".to_string()
                } else if error_str.contains("rate_limit") || error_str.contains("429") {
                    "⚠️ Rate limit exceeded. Please wait a moment before trying again.".to_string()
                } else if error_str.contains("401") || error_str.contains("permission") {
                    "⚠️ API authentication error. Check your API key.".to_string()
                } else {
                    format!("API Error: {}", error_str)
                };

                // Rollback transaction on API error
                if let Err(e) = crate::transactions::rollback_transaction() {
                    log_debug(&format!("Transaction rollback failed: {}", e));
                }

                let _ = tx.send(AppEvent::Error(user_message.clone()));
                // Also add as a message so user sees it in the chat
                let _ = tx.send(AppEvent::NewMessage(Message {
                    role: "assistant".to_string(),
                    content: Some(user_message),
                    tool_calls: None,
                    tool_call_id: None,
                }));
                break;
            }
        }
    }
    log_debug("=== CONVERSATION FINISHED ===");
    let _ = tx.send(AppEvent::Finished);

    // Commit transaction on successful completion
    crate::transactions::commit_transaction();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Message;

    #[test]
    fn test_filter_valid_messages() {
        let messages = vec![
            Message { role: "system".to_string(), content: Some("sys".to_string()), tool_calls: None, tool_call_id: None },
            Message { role: "user".to_string(), content: Some("hi".to_string()), tool_calls: None, tool_call_id: None },
            // Invalid assistant message (None/None) - should be removed
            Message { role: "assistant".to_string(), content: None, tool_calls: None, tool_call_id: None },
            // Valid assistant message
            Message { role: "assistant".to_string(), content: Some("response".to_string()), tool_calls: None, tool_call_id: None },
            // Invalid assistant message (empty tool_calls) - should be removed
            Message { role: "assistant".to_string(), content: None, tool_calls: Some(vec![]), tool_call_id: None },
            // Invalid assistant message (empty content) - should be removed
            Message { role: "assistant".to_string(), content: Some("".to_string()), tool_calls: None, tool_call_id: None },
            // Thought message - should be removed
            Message { role: "thought".to_string(), content: Some("thinking".to_string()), tool_calls: None, tool_call_id: None },
        ];

        let filtered = filter_valid_messages(&messages);
        
        println!("Filtered: {:?}", filtered);
        
        assert_eq!(filtered.len(), 3);
        assert!(filtered.iter().any(|m| m.role == "system"));
        assert!(filtered.iter().any(|m| m.role == "user"));
        assert!(filtered.iter().any(|m| m.role == "assistant" && m.content.as_deref() == Some("response")));
        
        // Ensure invalid ones are gone
        assert!(!filtered.iter().any(|m| m.role == "thought"));
        assert!(!filtered.iter().any(|m| m.content.as_deref() == Some("")));
        assert!(!filtered.iter().any(|m| m.tool_calls.as_ref().map(|tc| tc.is_empty()).unwrap_or(false)));
    }

    #[test]
    fn test_parse_role_directive() {
        // Basic @role: syntax
        let result = parse_role_directive("@planner: How do I implement this?");
        assert!(result.is_some());
        let directive = result.unwrap();
        assert_eq!(directive.role, "planner");
        assert_eq!(directive.content, "How do I implement this?");

        // With whitespace
        let result = parse_role_directive("  @coder:  implement the plan  ");
        assert!(result.is_some());
        let directive = result.unwrap();
        assert_eq!(directive.role, "coder");
        assert_eq!(directive.content, "implement the plan");

        // No directive
        let result = parse_role_directive("Just a normal message");
        assert!(result.is_none());

        // Invalid - no colon
        let result = parse_role_directive("@planner do this");
        assert!(result.is_none());

        // Role with underscore/hyphen
        let result = parse_role_directive("@code-reviewer: check this");
        assert!(result.is_some());
        assert_eq!(result.unwrap().role, "code-reviewer");
    }

    #[test]
    fn test_find_handoff_directive() {
        // Direct @role: at start of line
        let content = "Here's my plan:\n1. Do X\n2. Do Y\n\n@coder: Please implement this.";
        let result = find_handoff_directive(content);
        assert!(result.is_some());
        let directive = result.unwrap();
        assert_eq!(directive.role, "coder");
        assert_eq!(directive.content, "Please implement this.");

        // "hand off to @role:" pattern
        let content = "I've finished planning. Hand off to @coder: implement the changes above.";
        let result = find_handoff_directive(content);
        assert!(result.is_some());
        assert_eq!(result.unwrap().role, "coder");

        // No handoff
        let content = "This is just a regular response without any role mentions.";
        let result = find_handoff_directive(content);
        assert!(result.is_none());

        // Email-like @ should not trigger
        let content = "Contact me at user@example.com for more info.";
        let result = find_handoff_directive(content);
        assert!(result.is_none());
    }
}
