# Grok CLI

A powerful, interactive terminal-based chat client for xAI's Grok models, built in Rust. Inspired by Claude Code's clean UI/UX, Grok CLI provides a sophisticated TUI for interacting with Grok models through streaming responses, tool execution, and intelligent context management.

## Features

- **Interactive TUI**: Rich terminal user interface built with `ratatui`
- **Streaming Responses**: Real-time token streaming for chat completions
- **Tool Use**: Autonomous capability to execute shell commands, read/write files, search code, and run custom tools
- **Sandbox Mode**: Restrict tool execution to the current working directory for safety
- **Interactive Planning**: Special `/plan` mode for complex multi-step tasks with checkbox selection
- **Thinking Display**: Inline visibility into the model's reasoning process
- **Model Roles**: Role-based model orchestration with `@planner`, `@coder`, `@reviewer` directives
- **Rate Limiting**: Built-in rate limiter to prevent API quota violations
- **Context Compression**: Intelligent context management to handle long conversations
- **Custom Tool Plugins**: YAML-based system for defining custom tools
- **Config Persistence**: Model selection and settings saved across sessions
- **Command Autocomplete**: Tab-completion for slash commands and role names
- **Chat History**: Persistent session history for resuming conversations
- **File Transactions**: Snapshot and rollback system for safe file editing

## Installation

### Prerequisites

- Rust 1.70+ and Cargo
- xAI API key

### Build from Source

```bash
git clone https://github.com/PanicIsReal/grok-cli.git
cd grok-cli
cargo build --release
```

The binary will be available at `./target/release/grok-cli`.

## Configuration

### API Key

Set your xAI API key via environment variable or `.env` file:

```bash
# Environment variable
export XAI_API_KEY="your_api_key_here"

# Or create a .env file in the project directory
echo 'XAI_API_KEY=your_api_key_here' > .env
```

### Configuration File

Settings are stored in `~/.config/grok-cli/config.json`. Run `/init` to create default configuration.

```json
{
  "model": "grok-3",
  "sandbox_enabled": false,
  "allowed_commands": {},
  "settings": {
    "rate_limiter_enabled": true
  },
  "model_roles": {
    "@planner": "grok-4.1-fast-reasoning",
    "@coder": "grok-code-fast-1",
    "@reviewer": "grok-3-mini"
  },
  "rate_limits": {
    "grok-code-fast-1": {
      "context_window": 262144,
      "tokens_per_minute": 2000000,
      "requests_per_minute": 480
    },
    "grok-3": {
      "context_window": 131072,
      "tokens_per_minute": 1000000,
      "requests_per_minute": 300
    }
  }
}
```

### Files Created

| File | Location | Purpose |
|------|----------|---------|
| `config.json` | `~/.config/grok-cli/` | Global configuration |
| `tools/*.yaml` | `~/.config/grok-cli/` | Custom tool plugins |
| `.grok_history.json` | Current directory | Chat history |
| `.grok_context.json` | Current directory | API context cache |
| `.grokignore` | Current directory | Ignore patterns (optional) |

## Usage

```bash
# Run directly
./target/release/grok-cli

# Or with cargo
cargo run --release -- [OPTIONS]
```

### Command-Line Options

| Option | Description |
|--------|-------------|
| `-r, --resume` | Resume the previous chat session |
| `-m, --model <MODEL>` | Select the model (default: `grok-3`) |
| `-s, --system <PROMPT>` | Set a custom system prompt |
| `--debug` | Run in debug mode |

### In-App Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/model <name>` | Switch models (persisted) |
| `/sandbox` | Toggle sandbox mode |
| `/plan` | Enter interactive planning mode |
| `/clear` | Clear chat history |
| `/converse` | Toggle conversation mode |
| `/context` | Show context usage |
| `/settings` | Open settings menu |
| `/init` | Initialize config with defaults |
| `/exit` | Quit the application |

### Model Roles

Use `@role:` prefix to invoke specialized models:

```
@planner: Design the architecture for a REST API
@coder: Implement the user authentication module
@reviewer: Review this pull request for issues
```

### Available Models

| Model | Description |
|-------|-------------|
| `grok-3` | Default model, balanced performance |
| `grok-3-mini` | Faster, lighter model |
| `grok-4-1-fast-reasoning` | Advanced reasoning capabilities |
| `grok-4-1-fast-non-reasoning` | Fast non-reasoning tasks |
| `grok-code-fast-1` | Optimized for code generation |
| `grok-2-vision-1212` | Vision capabilities |

## Key Bindings

| Key | Action |
|-----|--------|
| Enter | Send message / Confirm |
| Ctrl+Enter | Insert new line |
| Ctrl+O | Toggle fullscreen output |
| Esc | Clear input / Exit mode |
| PageUp/PageDown | Scroll history |
| Tab | Accept autocomplete |
| Space | Toggle checkbox (in planning mode) |
| j/k | Scroll messages |
| Ctrl+↑/↓ | Navigate input history |

## Built-in Tools

Grok CLI includes several built-in tools that the model can use autonomously:

| Tool | Description |
|------|-------------|
| **Read** | Read file contents with line numbers |
| **Edit** | Exact string replacement in files |
| **Write** | Create or overwrite files |
| **Glob** | Find files by pattern (e.g., `**/*.rs`) |
| **Grep** | Search file contents with regex |
| **Bash** | Execute shell commands |
| **WebSearch** | Search the web for information |

Tool execution requires user approval unless the command has been whitelisted.

## Custom Tool Plugins

Define custom tools via YAML files in `~/.config/grok-cli/tools/`:

```yaml
name: "GitStatus"
description: "Show the current git repository status"
icon: "git"
category: "version_control"
parameters:
  - name: "verbose"
    type: "boolean"
    description: "Show verbose output"
    required: false
command: "git status {{#if verbose}}-v{{/if}}"
sandbox_aware: true
```

Run `/init` to create example tool plugins.

## Safety Features

- **Sandbox Mode**: Restricts file operations to the current working directory
- **Tool Approval**: User confirmation required for tool execution
- **Command Whitelisting**: Save trusted commands for automatic approval
- **Rate Limiting**: Prevents API quota violations by monitoring TPM/RPM
- **Context Compression**: Automatically compresses old conversations at 70% capacity
- **File Transactions**: Snapshot and restore functionality for safe editing

## UI Overview

```
  Welcome to Grok CLI
  Type a message to get started, or use /help for commands.

> Help me refactor this function

  ◐ Thinking...
    Analyzing the code structure...

  ⏺ Read
    {"file_path": "src/main.rs"}

  Here's my analysis of the function...

                                              grok-3 | Ready
───────────────────────────────────────────────────────────────
Type a message... ( / for commands )
```

## Project Structure

```
src/
├── main.rs           # Entry point, CLI parsing, event loop
├── app.rs            # Application state, message handling
├── ui.rs             # UI rendering with ratatui
├── api.rs            # Grok API client
├── tools.rs          # Built-in tool definitions & execution
├── tool_plugins.rs   # YAML-based custom tool system
├── config.rs         # Configuration persistence
├── settings.rs       # User-toggleable settings
├── message_handling.rs # Message submission, command processing
├── persistence.rs    # Chat history save/load
├── compression.rs    # Context window compression
├── transactions.rs   # File snapshot and rollback
├── markdown.rs       # Markdown rendering
├── planning.rs       # Planning mode logic
├── megamind.rs       # Advanced orchestration
└── autocomplete.rs   # Command autocomplete
```

## Dependencies

**Core**:
- `ratatui` - Terminal UI framework
- `tokio` - Async runtime
- `reqwest` - HTTP client
- `crossterm` - Terminal handling
- `clap` - CLI argument parsing

**Utilities**:
- `serde` / `serde_json` / `serde_yaml` - Serialization
- `regex` - Pattern matching
- `glob` - File pattern matching
- `chrono` - Date/time handling
- `anyhow` - Error handling

## License

MIT
