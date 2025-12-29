use crate::api::{GrokClient, Message};
use crate::config::{save_config, Config};
use crate::persistence::{save_history, save_context, load_context, DEFAULT_HISTORY_FILE, DEFAULT_CONTEXT_FILE};
use crate::tools::{execute_tool, get_tool_definitions, ToolCall};
use ratatui::{
    style::{Color, Style},
    widgets::{Block, Borders, ListState},
};
use serde_json::{json, Value};
use std::sync::{mpsc, Arc};
use std::io::{BufWriter, Write};
use std::fs::OpenOptions;
use tui_textarea::TextArea;
use serde::{Deserialize, Serialize};
use std::time::Instant;

// Re-export from app.rs for now, will move later
pub use crate::app::{App, AppMode, PlanningState, AppEvent, ActiveRole};

pub impl<'a> App<'a> {
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
                if let Some(client) = Arc::get_mut(&mut self.client) {
                    client.update_model(new_model.clone());
                    // Save to config for persistence
                    self.config.model = new_model.clone();
                    save_config(&self.config).ok();
                    self.messages.push(Message {
                        role: "system".to_string(),
                        content: Some(format!("Model changed to: {}", new_model)),
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
            self.messages.push(Message {
                role: "system".to_string(),
                content: Some(format!(
                    "Commands:\n  /plan     - Toggle planning mode\n  /model    - Switch models\n  /sandbox  - Toggle sandbox ({})\n  /converse - Toggle converse mode ({})\n  /context  - Show context usage\n  /clear    - Clear history\n  /init     - Initialize config file with defaults\n  /exit     - Exit\n\nKeys: j/k scroll, Ctrl+↑↓, PageUp/Down, Ctrl+O expand output",
                    sandbox_status, converse_status
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
                (crate::app::total_context_tokens(&self.api_messages), 0)
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
            let default_config = Config::default();
            match save_config(&default_config) {
                Ok(_) => {
                    self.messages.push(Message {
                        role: "system".to_string(),
                        content: Some("Config initialized with defaults at ~/.config/grok-cli/config.json".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                Err(e) => {
                    self.messages.push(Message {
                        role: "system".to_string(),
                        content: Some(format!("Failed to initialize config: {}", e)),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
            self.reset_input();
            return;
        }

        // Standard message
        self.reset_input();

        // Check for @role: directive
        let (active_role, message_content) = if let Some(directive) = crate::app::parse_role_directive(&content) {
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
        self.task_start = Some(Instant::now());

        // Update status to show role if active
        if let Some(ref role) = active_role {
            self.status_message = format!("@{} thinking...", role.name);
        } else {
            self.status_message = "Thinking...".to_string();
        }

        // Auto-scroll to user message
        self.list_state
            .select(Some(self.messages.len().saturating_sub(1)));

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

        tokio::spawn(async move {
            crate::app::process_conversation(client, history, tx, sandbox, debug, allowed_commands, max_context, roles, active_role, converse).await;
        });
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
        self.task_start = Some(Instant::now());

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
        tokio::spawn(async move {
            crate::app::process_conversation(client, history, tx, sandbox, debug, allowed_commands, max_context, roles, None, converse).await;
        });
    }

    pub fn reset_input(&mut self) {
        self.input = TextArea::default();
        self.input.set_placeholder_text("Type a message... ( / commands, @ roles )");
        self.input.set_block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    }
}