# Grok CLI

A powerful, interactive terminal-based chat client for xAI's Grok models, built in Rust. Inspired by Claude Code's clean UI/UX.

## Features

- **Interactive TUI**: Built with `ratatui` for a rich terminal user interface
- **Streaming Responses**: Real-time token streaming for chat completion
- **Tool Use**: Autonomous capability to run shell commands, read/write files
- **Sandbox Mode**: Restrict tool execution to the current working directory
- **Interactive Planning**: Special mode (`/plan`) for complex multi-step tasks
- **Thinking Display**: Inline visibility into the model's reasoning process
- **Config Persistence**: Model selection saved across sessions
- **Command Autocomplete**: Tab-completion for slash commands

## Installation

1. **Prerequisites**: Ensure you have Rust and Cargo installed
2. **Clone the repository**:
   ```bash
   git clone https://github.com/PanicIsReal/grok-cli.git
   cd grok-cli
   ```
3. **Build**:
   ```bash
   cargo build --release
   ```

## Configuration

Set your xAI API key in a `.env` file or export it:

```bash
export XAI_API_KEY="your_api_key_here"
```

Settings are stored in `~/.config/grok-cli/config.json`.

## Usage

```bash
cargo run --release -- [OPTIONS]
```

Or after building:
```bash
./target/release/grok-cli
```

### Options

- `-r, --resume`: Resume the previous chat session
- `-m, --model <MODEL>`: Select the model (default: `grok-3`)
- `-s, --system <PROMPT>`: Set a custom system prompt

### Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/model <name>` | Switch models (persisted) |
| `/sandbox` | Toggle sandbox mode (restrict to cwd) |
| `/plan` | Enter interactive planning mode |
| `/clear` | Clear chat history |
| `/exit` | Quit the application |

### Available Models

- `grok-3` (default)
- `grok-3-mini`
- `grok-4-1-fast-reasoning`
- `grok-4-1-fast-non-reasoning`
- `grok-code-fast-1`
- `grok-2-vision-1212`

## Key Bindings

| Key | Action |
|-----|--------|
| Enter | Send message / Confirm |
| Ctrl+Enter | Insert new line |
| Ctrl+O | Toggle fullscreen output |
| Esc | Clear input / Exit |
| PageUp/PageDown | Scroll history |
| Tab | Accept autocomplete |
| Space | Toggle checkbox (in planning mode) |

## UI Overview

```
  Welcome to Grok CLI
  Type a message to get started, or use /help for commands.

> Hello!

  ◐ Thinking...
    Analyzing the user's greeting...

  ⏺ run_shell_command
    {"command": "echo 'Hello!'"}
    Hello!

  Hi there! How can I help you today?

                                                          Ready
───────────────────────────────────────────────────────────────
Type a message... ( / for commands )
```

## Development

Built with:
- `ratatui` - Terminal UI framework
- `tokio` - Async runtime
- `reqwest` - HTTP client
- `crossterm` - Terminal handling
- `clap` - CLI argument parsing

### Project Structure

```
src/
├── main.rs       # Entry point, event loop
├── app.rs        # Application state, message handling
├── ui.rs         # UI rendering
├── api.rs        # Grok API client
├── tools.rs      # Tool definitions & execution
├── config.rs     # Config persistence
├── markdown.rs   # Markdown rendering
└── persistence.rs # Chat history
```

## License

MIT
