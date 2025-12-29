use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use anyhow::Result;
use crate::tools::ToolCall;

const API_URL: &str = "https://api.x.ai/v1/chat/completions";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

pub struct GrokClient {
    client: Client,
    api_key: String,
    model: String,
}

impl GrokClient {
    pub fn new(model: String) -> Result<Self> {
        let api_key = env::var("XAI_API_KEY").expect("XAI_API_KEY must be set");
        Ok(Self {
            client: Client::new(),
            api_key,
            model,
        })
    }

    pub fn update_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn get_model(&self) -> &str {
        &self.model
    }

    #[allow(dead_code)]
    pub async fn chat_completion(
        &self,
        messages: Vec<Message>,
        tools: Vec<Value>,
    ) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "stream": false
        });

        let res = self.client.post(API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
             let error_text = res.text().await?;
             return Err(anyhow::anyhow!("API Error: {}", error_text));
        }

        let json_res: Value = res.json().await?;
        Ok(json_res)
    }

    pub async fn chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Value>,
    ) -> Result<reqwest::Response> {
        self.chat_completion_stream_with_model(messages, tools, &self.model).await
    }

    /// Stream chat completion with explicit model override
    pub async fn chat_completion_stream_with_model(
        &self,
        messages: Vec<Message>,
        tools: Vec<Value>,
        model: &str,
    ) -> Result<reqwest::Response> {
        let body = json!({
            "model": model,
            "messages": messages,
            "tools": tools,
            "stream": true,
            "stream_options": {
                "include_usage": true
            }
        });

        let res = self.client.post(API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
             let error_text = res.text().await?;
             return Err(anyhow::anyhow!("API Error: {}", error_text));
        }

        Ok(res)
    }
}
