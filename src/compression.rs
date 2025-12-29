use crate::api::Message;
use crate::app::{App, TodoItem, TodoStatus};

// Re-export functions from app.rs
pub use crate::app::{
    estimate_tokens, message_tokens, total_context_tokens, safe_truncate,
    parse_role_directive, find_handoff_directive, compress_history_if_needed,
    summarize_tool_result, filter_valid_messages,
};

impl<'a> App<'a> {
    /// Compress old context when exceeding threshold
    fn compress_context_if_needed(&mut self) {
        let tokens = total_context_tokens(&self.api_messages);
        let max_context = self.get_current_context();
        let target_threshold = max_context * 5 / 10; // Target 50% after compression
        let trigger_threshold = max_context * 7 / 10; // Trigger at 70%

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
}