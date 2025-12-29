use crate::api::{GrokClient, Message};
use crate::app::AppEvent;
use serde_json::Value;
use std::sync::{mpsc, Arc};

/// Megamind agent definition
pub struct MegamindAgent {
    pub name: &'static str,
    pub model: &'static str,
    pub prompt: &'static str,
    pub icon: &'static str,
}

/// The 3 agents with different perspectives (token-efficient prompts)
pub const AGENTS: [MegamindAgent; 3] = [
    MegamindAgent {
        name: "Pragmatist",
        model: "grok-3-mini",
        prompt: "You are the Pragmatist. Focus on: feasibility, implementation cost, quick wins.\nRULES: MAX 3 bullet points. Each bullet: 1-2 sentences. Be concise.",
        icon: "P",
    },
    MegamindAgent {
        name: "Innovator",
        model: "grok-3-mini",
        prompt: "You are the Innovator. Focus on: creative solutions, novel approaches, 'what if' thinking.\nRULES: MAX 3 bullet points. Each bullet: 1-2 sentences. Build on previous ideas, don't repeat.",
        icon: "I",
    },
    MegamindAgent {
        name: "Critic",
        model: "grok-3-mini",
        prompt: "You are the Critic. Focus on: risks, edge cases, what could go wrong.\nRULES: MAX 3 bullet points. Each bullet: 1-2 sentences. Only raise NEW concerns.",
        icon: "C",
    },
];

/// Megamind brainstorming session state
#[derive(Clone)]
pub struct MegamindSession {
    pub topic: String,
    pub round: usize,
    pub max_rounds: usize,
    pub agent_responses: Vec<(String, String)>, // (agent_name, response)
}

impl MegamindSession {
    pub fn new(topic: String) -> Self {
        Self {
            topic,
            round: 1,
            max_rounds: 2,
            agent_responses: Vec::new(),
        }
    }

    /// Build minimal context for next agent (token-efficient)
    pub fn build_agent_context(&self, agent_idx: usize) -> String {
        let mut context = format!("TOPIC: {}", self.topic);

        // Add only current round's previous responses
        let round_start = (self.round - 1) * 3;
        for (name, response) in self.agent_responses.iter().skip(round_start).take(agent_idx) {
            context.push_str(&format!("\n\n[{}]: {}", &name[..1], response));
        }

        context
    }

    /// Check if we've reached consensus (critic has no major concerns)
    pub fn check_consensus(&self) -> bool {
        if let Some((name, response)) = self.agent_responses.last() {
            if name == "Critic" {
                let lower = response.to_lowercase();
                return lower.contains("no major concerns")
                    || lower.contains("looks good")
                    || lower.contains("agree with")
                    || lower.contains("solid approach");
            }
        }
        false
    }
}

/// Run the megamind brainstorming session
pub async fn run_megamind(
    client: Arc<GrokClient>,
    topic: String,
    tx: mpsc::Sender<AppEvent>,
) {
    let mut session = MegamindSession::new(topic);

    // Run rounds until max or consensus
    loop {
        let _ = tx.send(AppEvent::StatusUpdate(format!(
            "Megamind Round {}/{}...",
            session.round, session.max_rounds
        )));

        // Run each agent in sequence
        for (idx, agent) in AGENTS.iter().enumerate() {
            let _ = tx.send(AppEvent::StatusUpdate(format!(
                "Megamind: {} thinking...",
                agent.name
            )));

            let context = session.build_agent_context(idx);
            let messages = vec![
                Message {
                    role: "system".to_string(),
                    content: Some(agent.prompt.to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                Message {
                    role: "user".to_string(),
                    content: Some(format!("{}\n\nYour perspective:", context)),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ];

            // Call API with agent's model (no tools for brainstorming)
            match client
                .chat_completion_stream_with_model(messages, vec![], agent.model)
                .await
            {
                Ok(mut response) => {
                    let mut full_response = String::new();
                    let mut sse_buffer = String::new();

                    // Stream processing
                    while let Ok(Some(chunk)) = response.chunk().await {
                        let text = String::from_utf8_lossy(&chunk);
                        sse_buffer.push_str(&text);

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
                                break;
                            }

                            if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                    if let Some(choice) = choices.first() {
                                        if let Some(delta) = choice.get("delta") {
                                            if let Some(content) =
                                                delta.get("content").and_then(|c| c.as_str())
                                            {
                                                full_response.push_str(content);
                                                // Stream token to UI
                                                let _ = tx.send(AppEvent::MegamindToken(
                                                    agent.name.to_string(),
                                                    content.to_string(),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Trim and store response
                    let response_text = full_response.trim().to_string();
                    session
                        .agent_responses
                        .push((agent.name.to_string(), response_text.clone()));

                    // Send completed agent response
                    let _ = tx.send(AppEvent::MegamindAgentDone(
                        agent.name.to_string(),
                        response_text,
                    ));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Megamind error: {}", e)));
                    let _ = tx.send(AppEvent::Finished);
                    return;
                }
            }
        }

        // Check for consensus or max rounds
        if session.check_consensus() || session.round >= session.max_rounds {
            break;
        }

        session.round += 1;
    }

    // Synthesize final result
    let _ = tx.send(AppEvent::StatusUpdate("Megamind: Synthesizing...".to_string()));
    synthesize_megamind(&client, &session, &tx).await;

    let _ = tx.send(AppEvent::Finished);
}

/// Synthesize all ideas into actionable points
async fn synthesize_megamind(
    client: &Arc<GrokClient>,
    session: &MegamindSession,
    tx: &mpsc::Sender<AppEvent>,
) {
    let synthesis_prompt =
        "Synthesize the brainstorming into 3-5 actionable points. Be brief and practical.";

    // Collect all ideas
    let all_ideas = session
        .agent_responses
        .iter()
        .map(|(name, resp)| format!("[{}] {}", &name[..1], resp))
        .collect::<Vec<_>>()
        .join("\n\n");

    let messages = vec![
        Message {
            role: "system".to_string(),
            content: Some(synthesis_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        },
        Message {
            role: "user".to_string(),
            content: Some(format!("TOPIC: {}\n\nIDEAS:\n{}", session.topic, all_ideas)),
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    // Use fast model for synthesis
    match client
        .chat_completion_stream_with_model(messages, vec![], "grok-3-mini")
        .await
    {
        Ok(mut response) => {
            let mut synthesis = String::new();
            let mut sse_buffer = String::new();

            while let Ok(Some(chunk)) = response.chunk().await {
                let text = String::from_utf8_lossy(&chunk);
                sse_buffer.push_str(&text);

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
                        break;
                    }

                    if let Ok(val) = serde_json::from_str::<Value>(json_str) {
                        if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                            if let Some(choice) = choices.first() {
                                if let Some(delta) = choice.get("delta") {
                                    if let Some(content) =
                                        delta.get("content").and_then(|c| c.as_str())
                                    {
                                        synthesis.push_str(content);
                                        let _ = tx.send(AppEvent::MegamindToken(
                                            "Synthesis".to_string(),
                                            content.to_string(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let _ = tx.send(AppEvent::MegamindComplete(synthesis.trim().to_string()));
        }
        Err(e) => {
            let _ = tx.send(AppEvent::Error(format!("Synthesis error: {}", e)));
        }
    }
}
