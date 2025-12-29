use std::{io, io::{BufRead, Write}, time::Duration, panic, sync::atomic::{AtomicBool, Ordering}};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Global flag to track if terminal is in raw mode
static TERMINAL_RAW: AtomicBool = AtomicBool::new(false);

const CRASH_LOG_FILE: &str = "grok-cli-crash.log";

mod api;
mod tools;
mod markdown;
mod persistence;
mod app;
mod ui;
mod config;
mod megamind;
mod transactions;
mod settings;
mod tool_plugins;

use crate::api::{GrokClient, Message};
use crate::persistence::{save_history, load_history, save_context, DEFAULT_HISTORY_FILE, DEFAULT_CONTEXT_FILE};
use crate::app::{App, AppMode, AppEvent, PlanningState};
use crate::ui::ui;
use crate::config::{load_config, Config, save_config};
// use crate::tools::execute_tool;  // Used by tool execution in app.rs

fn get_default_system_prompt() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    // Get plugin tools descriptions
    let plugin_tools = crate::tool_plugins::get_plugin_tool_descriptions();
    let plugin_section = if plugin_tools.is_empty() {
        String::new()
    } else {
        let mut section = String::from("\n## Custom Tools (Plugins)\n");
        for (name, description, icon) in plugin_tools {
            let icon_str = icon.unwrap_or_default();
            section.push_str(&format!("- **{}** {}: {}\n", name, icon_str, description));
        }
        section
    };

    format!(r#"You are Grok CLI, an AI coding assistant running in the user's terminal.

# Environment
- Working directory: {}
- Platform: {}

# Available Tools
You have access to the following tools to help complete tasks:

## File Operations
- **Read**: Read file contents with line numbers. Always read before editing.
- **Edit**: Exact string replacement in files. old_string must match exactly.
- **Write**: Create new files or completely overwrite existing ones.
- **Glob**: Find files by pattern (e.g., "**/*.rs", "src/**/*.ts").
- **Grep**: Search file contents with regex patterns.
- **List**: List directory contents.

## System
- **Bash**: Execute shell commands. Use for git, builds, running programs. Requires user approval.
- **WebSearch**: Search the web for current information. Requires user approval.

## Planning
- **AskUser**: Ask the user a multiple choice question.
- **ConfirmPlan**: Present a plan for user confirmation before executing.
- **TodoWrite**: Update task progress and track multiple steps.
{}
# Guidelines
1. Read files before editing them - never guess at content.
2. Use Edit for modifications, Write only for new files.
3. Be concise in responses - this is a terminal interface.
4. When editing, include enough context in old_string to ensure uniqueness.
5. Use tools proactively to gather information and complete tasks.
6. For complex tasks, break them into steps and confirm with the user.
7. Bash and WebSearch require user approval for safety.

# Style
- Keep responses brief and focused
- Use markdown formatting sparingly
- Show file paths and code when relevant
- Explain what you're doing before using tools"#, cwd, std::env::consts::OS, plugin_section)
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Resume the previous chat session
    #[arg(short, long)]
    resume: bool,

    /// Set a custom system prompt
    #[arg(short, long)]
    system: Option<String>,

    /// Select the model (default: grok-3)
    #[arg(short, long)]
    model: Option<String>,

    /// Enable debug logging to grok-debug.log
    #[arg(short, long)]
    debug: bool,

    /// Initialize config file with defaults
    #[arg(long)]
    init: bool,

    /// Run as MCP server
    #[arg(long)]
    mcp: bool,

    /// Auto-fix mode: review crash log and attempt to fix
    #[arg(long)]
    auto_fix: bool,
}

// MCP JSON-RPC structures
#[derive(Serialize, Deserialize, Debug)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize, Deserialize, Debug)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
struct InitializeResult {
    protocol_version: String,
    capabilities: Value,
    server_info: Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct ToolListResult {
    tools: Vec<McpTool>,
}

#[derive(Serialize, Deserialize, Debug)]
struct McpTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct ToolCallResult {
    content: Vec<Content>,
    #[serde(rename = "isError")]
    is_error: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Content {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

/// Handle MCP requests
fn handle_mcp_request(request: JsonRpcRequest, config: &Config) -> JsonRpcResponse {
    let id = request.id.clone();

    match request.method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: "2024-11-05".to_string(),
                capabilities: serde_json::json!({
                    "tools": {
                        "listChanged": false
                    }
                }),
                server_info: serde_json::json!({
                    "name": "grok-cli",
                    "version": "0.1.0"
                }),
            };
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            }
        }
        "tools/list" => {
            let tools = crate::tools::get_tool_definitions()
                .into_iter()
                .enumerate()
                .map(|(i, def)| {
                    let func = &def["function"];
                    McpTool {
                        name: func["name"].as_str().unwrap().to_string(),
                        description: func["description"].as_str().unwrap().to_string(),
                        input_schema: func["parameters"].clone(),
                    }
                })
                .collect::<Vec<_>>();

            let result = ToolListResult { tools };
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            }
        }
        "tools/call" => {
            if let Some(params) = request.params {
                if let Some(name) = params.get("name").and_then(|n| n.as_str()) {
                    if let Some(args) = params.get("arguments") {
                        let args_json = args.to_string();
                        let sandbox_cwd = None; // MCP doesn't use sandbox mode
                        let output = crate::tools::execute_tool(name, &args_json, sandbox_cwd);

                        let result = ToolCallResult {
                            content: vec![Content {
                                content_type: "text".to_string(),
                                text: output,
                            }],
                            is_error: None,
                        };
                        JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id,
                            result: Some(serde_json::to_value(result).unwrap()),
                            error: None,
                        }
                    } else {
                        JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: "Invalid params".to_string(),
                                data: Some(Value::String("Missing arguments".to_string())),
                            }),
                        }
                    }
                } else {
                    JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Invalid params".to_string(),
                            data: Some(Value::String("Missing name".to_string())),
                        }),
                    }
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid params".to_string(),
                        data: None,
                    }),
                }
            }
        }
        "ping" => {
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(Value::Null),
                error: None,
            }
        }
        _ => {
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            }
        }
    }
}

/// Set up panic hook to log crashes and restore terminal
fn setup_panic_hook() {
    let original_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Restore terminal state first
        if TERMINAL_RAW.load(Ordering::SeqCst) {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        }

        // Build crash report
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let location = panic_info.location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());

        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic payload".to_string()
        };

        // Get backtrace
        let backtrace = std::backtrace::Backtrace::force_capture();

        let crash_report = format!(
            "=== GROK-CLI CRASH REPORT ===\n\
             Timestamp: {}\n\
             Location: {}\n\
             Message: {}\n\n\
             Backtrace:\n{}\n\
             =============================\n\n",
            timestamp, location, message, backtrace
        );

        // Write to crash log
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(CRASH_LOG_FILE)
        {
            use std::io::Write;
            let _ = file.write_all(crash_report.as_bytes());
        }

        // Print to stderr
        eprintln!("\n{}", crash_report);
        eprintln!("Crash logged to: {}", CRASH_LOG_FILE);
        eprintln!("\nTo attempt auto-fix, run:");
        eprintln!("  grok-cli --auto-fix\n");

        // Call original hook
        original_hook(panic_info);
    }));
}



/// MCP server mode
fn run_mcp_server() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut handle = stdin.lock();

    // Load config for sandbox mode
    let config = load_config();

    eprintln!("Starting MCP server on stdio...");

    loop {
        let mut line = String::new();
        match handle.read_line(&mut line) {
            Ok(0) => {
                // EOF reached
                break;
            }
            Ok(_) => {
                // Parse JSON-RPC request
                match serde_json::from_str::<JsonRpcRequest>(&line.trim()) {
                    Ok(request) => {
                        let response = handle_mcp_request(request, &config);
                        let response_json = serde_json::to_string(&response)?;
                        writeln!(stdout, "{}", response_json)?;
                        stdout.flush()?;
                    }
                    Err(e) => {
                        // Parse error (-32700)
                        let error_response = JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: None, // No ID available for parse error
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32700,
                                message: "Parse error".to_string(),
                                data: Some(Value::String(e.to_string())),
                            }),
                        };
                        let response_json = serde_json::to_string(&error_response)?;
                        writeln!(stdout, "{}", response_json)?;
                        stdout.flush()?;
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading from stdin: {}", e);
                break;
            }
        }
    }

    Ok(())
}

/// Auto-fix mode: read crash log and invoke Claude to fix
fn run_auto_fix() -> Result<()> {
    use std::fs;
    use std::process::Command;

    let crash_log = fs::read_to_string(CRASH_LOG_FILE)
        .map_err(|e| anyhow::anyhow!("No crash log found: {}", e))?;

    // Get the most recent crash (last entry)
    let last_crash = crash_log
        .split("=== GROK-CLI CRASH REPORT ===")
        .filter(|s| !s.trim().is_empty())
        .last()
        .ok_or_else(|| anyhow::anyhow!("No crash entries found"))?;

    println!("Found crash report:");
    println!("{}", last_crash.lines().take(10).collect::<Vec<_>>().join("\n"));
    println!("...\n");

    // Invoke Claude Code to fix the issue
    println!("Invoking Claude Code to analyze and fix...\n");

    let prompt = format!(
        "The Grok CLI application crashed with this error:\n\n{}\n\n\
         Please analyze the crash, find the root cause in the source code, and fix it. \
         The source is in src/. After fixing, run `cargo check` to verify.",
        last_crash
    );

    // Try to invoke claude command
    let status = Command::new("claude")
        .arg("-p")
        .arg(&prompt)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("\nAuto-fix completed. Run `cargo build` to rebuild.");
            // Clear the crash log after successful fix
            fs::write(CRASH_LOG_FILE, "")?;
        }
        Ok(_) => {
            eprintln!("Claude Code exited with error");
        }
        Err(e) => {
            eprintln!("Could not invoke Claude Code: {}", e);
            eprintln!("Make sure 'claude' is in your PATH");
            eprintln!("\nManual fix required. Crash details:");
            println!("{}", last_crash);
        }
    }

    Ok(())
}

/// Initialize default tool YAML files if they don't exist
fn init_default_tools() -> Result<usize> {
    use std::fs;
    use std::path::PathBuf;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let tools_dir = PathBuf::from(home).join(".config/grok-cli/tools");

    // Create tools directory if it doesn't exist
    fs::create_dir_all(&tools_dir)?;

    let mut created_count = 0;

    // Define default tools
    let default_tools = vec![
        ("system_info.yaml", r#"# System Info Tool
# Get system information like hostname, OS, uptime, and memory usage

name: SystemInfo
description: Get system information like hostname, OS, uptime, and memory usage. Useful for debugging environment issues.

parameters:
  - name: info_type
    type: string
    description: "Type of info to get: all, hostname, os, uptime, memory, disk"
    required: false
    default: all

command: |
  case "{{info_type}}" in
    hostname) hostname;;
    os) uname -a;;
    uptime) uptime;;
    memory) free -h 2>/dev/null || vm_stat;;
    disk) df -h .;;
    all|*)
      echo "=== System Info ===" &&
      echo "Hostname: $(hostname)" &&
      echo "OS: $(uname -s) $(uname -r)" &&
      echo "Uptime: $(uptime)" &&
      echo "" &&
      echo "=== Memory ===" &&
      (free -h 2>/dev/null || vm_stat) &&
      echo "" &&
      echo "=== Disk ===" &&
      df -h .
    ;;
  esac

sandbox_aware: false
category: system
icon: "🖥️"
"#),
        ("git_status.yaml", r#"# Git Status Tool
# Quickly check git repository status

name: GitStatus
description: Get git repository status including branch, changes, and recent commits. Faster than running multiple git commands.

parameters:
  - name: verbose
    type: boolean
    description: Show verbose output including recent commits
    required: false
    default: "false"

command: |
  if [ "{{verbose}}" = "true" ]; then
    echo "=== Branch ===" &&
    git branch --show-current 2>/dev/null || echo "(not a git repo)" &&
    echo "" &&
    echo "=== Status ===" &&
    git status -s 2>/dev/null || echo "(not a git repo)" &&
    echo "" &&
    echo "=== Recent Commits ===" &&
    git log --oneline -5 2>/dev/null || echo "(no commits)"
  else
    git status -s 2>/dev/null || echo "(not a git repo)"
  fi

sandbox_aware: true
category: git
icon: "📋"
"#),
        ("word_count.yaml", r#"# Word Count Tool
# Count lines, words, and characters in files

name: WordCount
description: Count lines, words, and characters in files. Supports glob patterns.

parameters:
  - name: pattern
    type: string
    description: "File path or glob pattern (e.g., '*.rs', 'src/**/*.ts')"
    required: true
  - name: summary_only
    type: boolean
    description: Only show totals, not per-file counts
    required: false
    default: "false"

command: |
  if [ "{{summary_only}}" = "true" ]; then
    wc {{pattern}} 2>/dev/null | tail -1 || echo "No matching files"
  else
    wc {{pattern}} 2>/dev/null || echo "No matching files"
  fi

sandbox_aware: true
category: files
icon: "🔢"
"#),
        ("find_large_files.yaml", r#"# Find Large Files Tool
# Find the largest files in a directory

name: FindLargeFiles
description: Find the largest files in a directory. Useful for finding what's taking up disk space.

parameters:
  - name: path
    type: string
    description: Directory to search in
    required: false
    default: "."
  - name: count
    type: integer
    description: Number of files to show
    required: false
    default: "10"

command: |
  find {{path}} -type f -exec du -h {} + 2>/dev/null | sort -rh | head -{{count}}

sandbox_aware: true
category: files
icon: "📦"
"#),
        ("process_list.yaml", r#"# Process List Tool
# List running processes

name: ProcessList
description: List running processes, optionally filtered by name. Shows PID, CPU, memory, and command.

parameters:
  - name: filter
    type: string
    description: Filter processes by name (optional)
    required: false
    default: ""

command: |
  if [ -z "{{filter}}" ]; then
    ps aux --sort=-%mem 2>/dev/null | head -15 || ps aux | head -15
  else
    ps aux 2>/dev/null | grep -i "{{filter}}" | grep -v grep || echo "No matching processes"
  fi

sandbox_aware: false
category: system
icon: "⚙️"
"#),
    ];

    for (filename, content) in default_tools {
        let file_path = tools_dir.join(filename);
        if !file_path.exists() {
            fs::write(&file_path, content)?;
            created_count += 1;
        }
    }

    Ok(created_count)
}

#[tokio::main] async fn main() -> Result<()> {
    // Set up panic hook for crash recovery
    setup_panic_hook();

    dotenv::dotenv().ok();
    let args = Args::parse();

    // Handle init mode
    if args.init {
        let config = Config::default();
        match save_config(&config) {
            Ok(_) => println!("Config initialized at ~/.config/grok-cli/config.json"),
            Err(e) => eprintln!("Failed to initialize config: {}", e),
        }

        // Initialize default tools
        match init_default_tools() {
            Ok(count) => {
                if count > 0 {
                    println!("Created {} default tool(s) in ~/.config/grok-cli/tools/", count);
                } else {
                    println!("Tools directory ready (default tools already exist)");
                }
            }
            Err(e) => eprintln!("Failed to initialize tools: {}", e),
        }

        // Create .grokignore in current directory
        match tools::create_default_grokignore() {
            Ok(true) => println!("Created .grokignore with default patterns"),
            Ok(false) => println!(".grokignore already exists (skipped)"),
            Err(e) => eprintln!("Failed to create .grokignore: {}", e),
        }

        return Ok(());
    }

    // Handle MCP mode
    if args.mcp {
        return run_mcp_server();
    }

    // Handle auto-fix mode
    if args.auto_fix {
        return run_auto_fix();
    }

    // Load config, use CLI arg if provided, otherwise use saved config
    let mut config = load_config();
    if let Some(model) = args.model {
        config.model = model;
        crate::config::save_config(&config).ok();
    }

    let client = match GrokClient::new(config.model.clone()) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("Error: XAI_API_KEY not found.");
            std::process::exit(1);
        }
    };

    let mut messages = if args.resume {
        load_history(DEFAULT_HISTORY_FILE).unwrap_or_default()
    } else {
        Vec::new()
    };

    if messages.is_empty() {
        let system_prompt = args.system.unwrap_or_else(|| get_default_system_prompt());
        messages.push(Message {
            role: "system".to_string(),
            content: Some(system_prompt),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    // Enter raw mode and track state for panic recovery
    enable_raw_mode()?;
    TERMINAL_RAW.store(true, Ordering::SeqCst);

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(client, messages, &config, args.debug);
    
    // Initialize transaction manager with sandbox settings
    crate::transactions::init_transaction_manager(
        if config.sandbox_enabled {
            Some(std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()))
        } else {
            None
        }
    );

    let res = run_app(&mut terminal, &mut app).await;

    // Clean up terminal and clear panic tracking
    disable_raw_mode()?;
    TERMINAL_RAW.store(false, Ordering::SeqCst);

    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App<'_>) -> Result<()> {
    loop {
        if app.is_loading {
             app.spinner_index = app.spinner_index.wrapping_add(1);
        }
        terminal.draw(|f| ui(f, app))?;

        if app.should_quit {
            break;
        }

        // Event Handling
        if event::poll(Duration::from_millis(50))? {
            let event = event::read()?;
            if let Event::Key(key) = &event {
                match &mut app.mode {
                     AppMode::Chat => {
                         // Autocomplete Navigation
                         if app.autocomplete_active && !app.autocomplete_filtered.is_empty() {
                             match key.code {
                                 KeyCode::Up => {
                                     if app.autocomplete_index > 0 {
                                         app.autocomplete_index -= 1;
                                     } else {
                                         app.autocomplete_index = app.autocomplete_filtered.len() - 1;
                                     }
                                     continue;
                                 }
                                 KeyCode::Down => {
                                     if app.autocomplete_index < app.autocomplete_filtered.len() - 1 {
                                         app.autocomplete_index += 1;
                                     } else {
                                         app.autocomplete_index = 0;
                                     }
                                     continue;
                                 }
                                 KeyCode::Enter | KeyCode::Tab => {
                                     let selection = app.autocomplete_filtered[app.autocomplete_index].clone();
                                     // For role autocomplete, strip the model hint: "@role:  (model)" -> "@role: "
                                     let insert_text = if selection.starts_with('@') {
                                         if let Some(paren_pos) = selection.find("  (") {
                                             format!("{} ", &selection[..paren_pos])
                                         } else {
                                             selection
                                         }
                                     } else {
                                         selection
                                     };
                                     app.reset_input();
                                     app.input.insert_str(insert_text);
                                     app.autocomplete_active = false;
                                     continue;
                                 }
                                 KeyCode::Esc => {
                                     app.autocomplete_active = false;
                                     continue;
                                 }
                                 _ => {} 
                             }
                         }

                         // History Navigation (Up/Down arrows)
                         match key.code {
                             KeyCode::Up => {
                                 if !app.autocomplete_active {
                                     app.navigate_history(true);
                                     continue;
                                 }
                             }
                             KeyCode::Down => {
                                 if !app.autocomplete_active {
                                     app.navigate_history(false);
                                     continue;
                                 }
                             }
                             _ => {}
                         }

                         // Ctrl+Up/Down for scrolling, Ctrl+O for expand/collapse, Ctrl+C for clear/exit
                         if key.modifiers.contains(KeyModifiers::CONTROL) {
                             match key.code {
                                 KeyCode::Up => {
                                     let i = app.list_state.selected().unwrap_or(0);
                                     if i > 0 {
                                         app.list_state.select(Some(i.saturating_sub(1)));
                                         app.auto_scroll = false; // Disable auto-scroll when user manually scrolls
                                     }
                                     continue;
                                 }
                                 KeyCode::Down => {
                                     let i = app.list_state.selected().unwrap_or(0);
                                     let max = app.messages.len().saturating_sub(1);
                                     if i < max {
                                         app.list_state.select(Some(i + 1));
                                         app.auto_scroll = false; // Disable auto-scroll when user manually scrolls
                                     }
                                     continue;
                                 }
                                 KeyCode::Char('o') => {
                                     app.toggle_tool_expansion();
                                     continue;
                                 }
                                 KeyCode::Char('c') => {
                                     let now = std::time::Instant::now();
                                     let input_empty = app.input.lines().join("").is_empty();

                                     // If loading/thinking, cancel the operation
                                     if app.is_loading {
                                         app.cancel_requested = true;
                                         app.is_loading = false;
                                         app.status_message = "Cancelled".to_string();
                                         continue;
                                     }

                                     // If input has content, clear it
                                     if !input_empty {
                                         app.reset_input();
                                         app.last_ctrl_c = Some(now);
                                         continue;
                                     }

                                     // If input is empty, check for double Ctrl+C
                                     if let Some(last) = app.last_ctrl_c {
                                         if now.duration_since(last).as_millis() < 500 {
                                             // Double Ctrl+C - exit
                                             break;
                                         }
                                     }
                                     app.last_ctrl_c = Some(now);
                                     app.status_message = "Press Ctrl+C again to exit".to_string();
                                     continue;
                                 }
                                 _ => {}
                             }
                         }

                         // j/k for scrolling only when input is empty
                         let input_empty = app.input.lines().join("").is_empty();
                         if input_empty {
                             match key.code {
                                 KeyCode::Char('j') => {
                                     let i = app.list_state.selected().unwrap_or(0);
                                     let max = app.messages.len().saturating_sub(1);
                                     if i < max {
                                         app.list_state.select(Some(i + 1));
                                         app.auto_scroll = false; // Disable auto-scroll when user manually scrolls
                                     }
                                     continue;
                                 }
                                 KeyCode::Char('k') => {
                                     let i = app.list_state.selected().unwrap_or(0);
                                     if i > 0 {
                                         app.list_state.select(Some(i.saturating_sub(1)));
                                         app.auto_scroll = false; // Disable auto-scroll when user manually scrolls
                                     }
                                     continue;
                                 }
                                 _ => {}
                             }
                         }

                         match key.code {
                            KeyCode::F(12) => {
                                if !app.errors.is_empty() {
                                    app.mode = AppMode::ErrorView;
                                }
                            }
                            KeyCode::PageUp => {
                                let i = app.list_state.selected().unwrap_or(0);
                                if i > 0 {
                                    app.list_state.select(Some(i.saturating_sub(10)));
                                    app.auto_scroll = false; // Disable auto-scroll when user manually scrolls
                                }
                            }
                            KeyCode::PageDown => {
                                let i = app.list_state.selected().unwrap_or(0);
                                if i < app.messages.len().saturating_sub(1) {
                                    app.list_state.select(Some((i + 10).min(app.messages.len().saturating_sub(1))));
                                    app.auto_scroll = false; // Disable auto-scroll when user manually scrolls
                                }
                            }
                            KeyCode::Esc => {
                                // If loading/thinking, cancel the operation
                                if app.is_loading {
                                    app.cancel_requested = true;
                                    app.is_loading = false;
                                    app.status_message = "Cancelled".to_string();
                                } else if app.input.lines().join("").is_empty() {
                                    break;
                                } else {
                                    app.reset_input();
                                }
                            }
                            KeyCode::Enter => {
                                // Multi-line input: Ctrl+Enter, Shift+Enter, or line ending with \
                                let content = app.input.lines().join("\n");
                                let ends_with_backslash = content.ends_with('\\');

                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    || key.modifiers.contains(KeyModifiers::SHIFT) {
                                    app.input.insert_newline();
                                } else if ends_with_backslash {
                                    // Remove trailing backslash and add newline
                                    app.input.delete_char();
                                    app.input.insert_newline();
                                } else {
                                    app.submit_message();
                                }
                            }
                            _ => {
                                app.input.input(event);
                                app.update_autocomplete();
                            }
                        }
                     }
                     AppMode::Planning(state) => {
                         match key.code {
                             KeyCode::Up => {
                                 let i = state.list_state.selected().unwrap_or(0);
                                 if i > 0 { state.list_state.select(Some(i - 1)); }
                             }
                             KeyCode::Down => {
                                 let i = state.list_state.selected().unwrap_or(0);
                                 if i < state.options.len() - 1 { state.list_state.select(Some(i + 1)); }
                             }
                             KeyCode::Char(' ') => {
                                 if let Some(i) = state.list_state.selected() {
                                     state.selected[i] = !state.selected[i];
                                 }
                             }
                             KeyCode::Enter => {
                                 app.handle_planning_selection();
                             }
                             _ => {}
                         }
                     }
                     AppMode::ErrorView => {
                         // F12 or Esc to exit error view
                         match key.code {
                             KeyCode::F(12) | KeyCode::Esc | KeyCode::Enter => {
                                 app.mode = AppMode::Chat;
                             }
                             KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                 // Clear errors
                                 app.errors.clear();
                                 app.mode = AppMode::Chat;
                             }
                             _ => {}
                         }
                     }
                     AppMode::Settings(ref mut state) => {
                         match key.code {
                             KeyCode::Up => {
                                 state.move_up();
                             }
                             KeyCode::Down => {
                                 state.move_down();
                             }
                             KeyCode::Char(' ') => {
                                 // Toggle the current setting
                                 if let Some(key) = state.current_setting_key() {
                                     match key {
                                         "rate_limiter_enabled" => {
                                             app.config.settings.rate_limiter_enabled = !app.config.settings.rate_limiter_enabled;
                                             save_config(&app.config).ok();
                                         }
                                         _ => {}
                                     }
                                 }
                             }
                             KeyCode::Esc | KeyCode::Enter => {
                                 app.mode = AppMode::Chat;
                             }
                             _ => {}
                         }
                     }
                 }
            }
        }

        while let Ok(event) = app.rx.try_recv() {
            // If cancel was requested, drain the queue but ignore results
            if app.cancel_requested {
                // Reset cancel flag after draining
                if matches!(event, AppEvent::Finished | AppEvent::Error(_)) {
                    app.cancel_requested = false;
                }
                continue;
            }

            match event {
                AppEvent::NewMessage(msg) => {
                    if let Some(last) = app.messages.last_mut() {
                        if last.role == "assistant" && msg.role == "assistant" {
                             *last = msg.clone();
                        } else {
                            app.messages.push(msg.clone());
                        }
                    } else {
                        app.messages.push(msg.clone());
                    }
                    app.api_messages.push(msg);
                    save_context(&app.api_messages, DEFAULT_CONTEXT_FILE).ok();
                    app.auto_scroll = true; // Enable auto-scroll for new messages
                    app.list_state.select(Some(app.messages.len().saturating_sub(1)));
                },
                AppEvent::Token(s) => {
                    if let Some(last) = app.messages.last_mut() {
                        if last.role == "assistant" && last.tool_calls.is_none() { 
                             if let Some(content) = &mut last.content {
                                 content.push_str(&s);
                             } else {
                                 last.content = Some(s);
                             }
                        } else {
                            app.messages.push(Message {
                                role: "assistant".to_string(),
                                content: Some(s),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                    } else {
                         app.messages.push(Message {
                            role: "assistant".to_string(),
                            content: Some(s),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                    app.list_state.select(Some(app.messages.len().saturating_sub(1)));
                },
                AppEvent::ThinkingToken(s) => {
                    // Update thinking preview for status bar
                    if let Some(ref mut preview) = app.thinking_preview {
                        preview.push_str(&s);
                        // Keep only last ~100 chars for display
                        if preview.len() > 150 {
                            let start = preview.len() - 100;
                            // Find a safe char boundary
                            let start = preview.char_indices()
                                .find(|(i, _)| *i >= start)
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            *preview = preview[start..].to_string();
                        }
                    } else {
                        app.thinking_preview = Some(s.clone());
                    }

                    if let Some(last) = app.messages.last_mut() {
                        if last.role == "thought" {
                             if let Some(content) = &mut last.content {
                                 content.push_str(&s);
                             } else {
                                 last.content = Some(s);
                             }
                        } else {
                            app.messages.push(Message {
                                role: "thought".to_string(),
                                content: Some(s),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                    } else {
                         app.messages.push(Message {
                            role: "thought".to_string(),
                            content: Some(s),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                    app.list_state.select(Some(app.messages.len().saturating_sub(1)));
                },
                AppEvent::StatusUpdate(s) => {
                    app.status_message = s;
                },
                AppEvent::Error(e) => {
                    // Store error in hidden list (accessible via F12)
                    app.errors.push(e.clone());
                    app.status_message = "Error occurred".to_string();
                    app.is_loading = false;
                    // Log to file (no terminal spam)
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("grok-cli-errors.log") {
                        use std::io::Write;
                        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                        writeln!(file, "[{}] {}", timestamp, e).ok();
                    }
                },
                AppEvent::Finished => {
                    // Capture task duration
                    if let Some(start) = app.task_start.take() {
                        app.last_task_duration = Some(start.elapsed());
                    }
                    app.is_loading = false;
                    app.status_message = "Ready".to_string();
                    app.thinking_preview = None; // Clear thinking preview
                    save_history(&app.messages, DEFAULT_HISTORY_FILE).ok();
                }
                AppEvent::UsageUpdate(input_tokens, output_tokens) => {
                    // Update to current call's usage (not cumulative)
                    // Input tokens = current context window usage
                    // Output tokens = tokens generated this call
                    app.total_input_tokens = input_tokens;
                    app.total_output_tokens = output_tokens;

                    // Update rate limit tracking
                    if app.rate_limit_window_start.is_none() {
                        app.rate_limit_window_start = Some(std::time::Instant::now());
                    }
                    // Add total tokens (input + output) to the minute window
                    app.tokens_used_this_minute += input_tokens + output_tokens;
                    app.requests_this_minute += 1;
                }
                AppEvent::PlanningRequest(q, opts, id, tool_call_cmd) => {
                    let len = opts.len();
                    app.mode = AppMode::Planning(PlanningState {
                        question: q,
                        options: opts,
                        selected: vec![false; len],
                        tool_call_id: id,
                        tool_call_cmd,
                        list_state: ratatui::widgets::ListState::default(),
                    });
                    if let AppMode::Planning(ref mut s) = app.mode {
                        s.list_state.select(Some(0));
                    }
                    app.is_loading = false;
                    app.status_message = "Selection Required".to_string();
                }
                AppEvent::ConfirmationRequest(plan, id) => {
                    // Store plan for display without adding to message history
                    // (adding assistant message here would break API flow)
                    app.pending_plan = Some(plan);
                    app.pending_confirmation = Some(id);
                    app.is_loading = false;
                    app.status_message = "Confirm plan: y/n or provide feedback".to_string();
                }
                AppEvent::BashApprovalRequest(tc, cmd) => {
                    // Use Planning modal for bash command approval
                    let truncated_cmd = if cmd.len() > 60 {
                        format!("{}...", &cmd[..60])
                    } else {
                        cmd.clone()
                    };
                    app.mode = AppMode::Planning(PlanningState {
                        question: format!("Execute command?\n$ {}", truncated_cmd),
                        options: vec![
                            "Approve".to_string(),
                            "Always Approve (save to config)".to_string(),
                            "Reject".to_string(),
                        ],
                        selected: vec![false, false, false],
                        tool_call_id: tc.id.clone(),
                        tool_call_cmd: Some((tc, cmd)),
                        list_state: ratatui::widgets::ListState::default(),
                    });
                    if let AppMode::Planning(ref mut s) = app.mode {
                        s.list_state.select(Some(0));
                    }
                    app.is_loading = false;
                    app.status_message = "Command approval required".to_string();
                }
                AppEvent::WebSearchApprovalRequest(tc, query) => {
                    // Use Planning modal for web search approval
                    let truncated_query = if query.len() > 50 {
                        format!("{}...", &query[..50])
                    } else {
                        query.clone()
                    };
                    app.mode = AppMode::Planning(PlanningState {
                        question: format!("Web search?\n🔍 {}", truncated_query),
                        options: vec![
                            "Approve".to_string(),
                            "Reject".to_string(),
                        ],
                        selected: vec![false, false],
                        tool_call_id: tc.id.clone(),
                        tool_call_cmd: Some((tc, query)),
                        list_state: ratatui::widgets::ListState::default(),
                    });
                    if let AppMode::Planning(ref mut s) = app.mode {
                        s.list_state.select(Some(0));
                    }
                    app.is_loading = false;
                    app.status_message = "Web search approval required".to_string();
                }
                AppEvent::RoleSwitch(from, to) => {
                    // Update status to show role switch
                    app.status_message = format!("Switching @{} → @{}", from, to);
                }
                AppEvent::TodoUpdate(new_todos) => {
                    // Smart merge: preserve completed items from old list that might have been dropped
                    let mut merged_todos = new_todos.clone();

                    // Find completed items from old list that aren't in new list
                    for old_todo in &app.todos {
                        if old_todo.status == crate::app::TodoStatus::Completed {
                            // Check if this completed item exists in new list
                            let exists = merged_todos.iter().any(|t| t.content == old_todo.content);
                            if !exists {
                                // Add the completed item back (at the position where it would logically be)
                                // Find first pending/in_progress item and insert before it
                                let insert_pos = merged_todos.iter()
                                    .position(|t| t.status != crate::app::TodoStatus::Completed)
                                    .unwrap_or(merged_todos.len());
                                merged_todos.insert(insert_pos, old_todo.clone());
                            }
                        }
                    }

                    app.todos = merged_todos;
                }
                AppEvent::MegamindToken(agent, token) => {
                    // Streaming token from megamind agent
                    if app.megamind_current_agent.as_ref() != Some(&agent) {
                        // New agent - flush buffer and start new message
                        if !app.megamind_buffer.is_empty() {
                            // The previous agent's response is complete
                        }
                        app.megamind_current_agent = Some(agent.clone());
                        app.megamind_buffer.clear();

                        // Add header for new agent
                        app.messages.push(Message {
                            role: "assistant".to_string(),
                            content: Some(format!("[{}]", agent)),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                    // Append token to current message
                    app.megamind_buffer.push_str(&token);
                    if let Some(last) = app.messages.last_mut() {
                        if last.role == "assistant" {
                            last.content = Some(format!("[{}] {}", agent, app.megamind_buffer));
                        }
                    }
                    app.auto_scroll = true;
                }
                AppEvent::MegamindAgentDone(agent, response) => {
                    // Agent completed - ensure message is finalized
                    if let Some(last) = app.messages.last_mut() {
                        if last.role == "assistant" && last.content.as_ref().map(|c| c.starts_with(&format!("[{}]", agent))).unwrap_or(false) {
                            last.content = Some(format!("[{}] {}", agent, response));
                        }
                    }
                    app.megamind_buffer.clear();
                    app.auto_scroll = true;
                }
                AppEvent::MegamindComplete(synthesis) => {
                    // Synthesis complete - add final message
                    app.messages.push(Message {
                        role: "assistant".to_string(),
                        content: Some(format!("=== Synthesis ===\n{}", synthesis)),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    app.megamind_active = false;
                    app.megamind_current_agent = None;
                    app.megamind_buffer.clear();
                    app.auto_scroll = true;
                    save_history(&app.messages, DEFAULT_HISTORY_FILE).ok();
                }
                AppEvent::RateLimitPause(seconds) => {
                    app.rate_limit_paused = true;
                    app.rate_limit_resume_at = Some(std::time::Instant::now() + Duration::from_secs(seconds));
                    app.status_message = format!("⏸ Rate limit - pausing {}s", seconds);
                }
                AppEvent::RateLimitResume => {
                    app.rate_limit_paused = false;
                    app.rate_limit_resume_at = None;
                    // Reset the rate limit counters after a pause
                    app.rate_limit_window_start = Some(std::time::Instant::now());
                    app.tokens_used_this_minute = 0;
                    app.requests_this_minute = 0;
                }
            }
        }

        // Update rate limit window tracking
        if let Some(start) = app.rate_limit_window_start {
            if start.elapsed() >= Duration::from_secs(60) {
                // Reset counters every minute
                app.rate_limit_window_start = Some(std::time::Instant::now());
                app.tokens_used_this_minute = 0;
                app.requests_this_minute = 0;
            }
        }
    }
    Ok(())
}