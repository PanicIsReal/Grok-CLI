use crate::api::Message;
use crate::persistence::{save_history, save_context, DEFAULT_HISTORY_FILE, DEFAULT_CONTEXT_FILE};
use crate::tools::{execute_tool, ToolCall};
use crate::config::Config;
use crate::app::{App, AppMode, PlanningState, AppEvent};

// Re-export
pub use crate::app::AppEvent;

impl<'a> App<'a> {
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
                        crate::config::save_config(&self.config).ok();
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
                tokio::spawn(async move {
                    crate::app::process_conversation(client, history, tx, sandbox, debug, allowed_commands, max_context, roles, None, converse).await;
                });
                return;
            }

            // Regular planning selection
            let response = format!("User selected: {:?}", selections);
            let id = state.tool_call_id.clone();
            self.respond_with_tool_output(id, response);
        }
    }
}