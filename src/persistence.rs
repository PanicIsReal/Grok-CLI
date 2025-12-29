use crate::api::Message;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub const DEFAULT_HISTORY_FILE: &str = ".grok_history.json";
pub const DEFAULT_CONTEXT_FILE: &str = ".grok_context.json";

pub fn save_history(messages: &[Message], path: &str) -> Result<()> {
    let json = serde_json::to_string_pretty(messages)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn load_history(path: &str) -> Result<Vec<Message>> {
    if !Path::new(path).exists() {
        return Ok(Vec::new());
    }
    let json = fs::read_to_string(path)?;
    let messages: Vec<Message> = serde_json::from_str(&json)?;
    Ok(messages)
}

/// Save optimized API context (separate from display history)
pub fn save_context(messages: &[Message], path: &str) -> Result<()> {
    let json = serde_json::to_string(messages)?; // compact for context
    fs::write(path, json)?;
    Ok(())
}

/// Load optimized API context
pub fn load_context(path: &str) -> Result<Vec<Message>> {
    if !Path::new(path).exists() {
        return Ok(Vec::new());
    }
    let json = fs::read_to_string(path)?;
    let messages: Vec<Message> = serde_json::from_str(&json)?;
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_save_and_load_history() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap();
        
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                tool_calls: None,
                tool_call_id: None,
            }
        ];
        
        save_history(&messages, path).unwrap();
        
        let loaded = load_history(path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content.as_ref().unwrap(), "hello");
    }
}

