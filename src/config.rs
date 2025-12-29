use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::settings::{Settings, RateLimitConfig, default_rate_limits};

const CONFIG_DIR: &str = ".config/grok-cli";
const CONFIG_FILE: &str = "config.json";

/// Configuration for a model role (e.g., planner, coder, reviewer)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelRole {
    /// The model to use for this role
    pub model: String,
    /// Optional custom system prompt for this role
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    pub model: String,
    #[serde(default)]
    pub sandbox_enabled: bool,
    #[serde(default)]
    pub allowed_commands: HashMap<String, Vec<String>>,
    /// Model roles for multi-model orchestration (e.g., @planner, @coder)
    #[serde(default)]
    pub roles: HashMap<String, ModelRole>,
    /// User-toggleable settings
    #[serde(default)]
    pub settings: Settings,
    /// Rate limits per model (can be customized by user)
    #[serde(default = "default_rate_limits")]
    pub rate_limits: HashMap<String, RateLimitConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut roles = HashMap::new();

        // Default planner role - good for reasoning and planning
        roles.insert("planner".to_string(), ModelRole {
            model: "grok-4.1-fast-reasoning".to_string(),
            prompt: Some("You are a planning assistant. Analyze requests carefully, break them into steps, and create detailed implementation plans. Focus on the 'what' and 'why', not the 'how'. When your plan is complete, hand off to @coder for implementation.".to_string()),
        });

        // Default coder role - optimized for code
        roles.insert("coder".to_string(), ModelRole {
            model: "grok-code-fast-1".to_string(),
            prompt: Some("You are a code execution assistant. Implement the plan given to you efficiently. Use tools to read, edit, and test code. Be concise and focus on execution.".to_string()),
        });

        // Default reviewer role - quick verification
        roles.insert("reviewer".to_string(), ModelRole {
            model: "grok-3-mini".to_string(),
            prompt: Some("You are a code reviewer. Check the implementation for bugs, edge cases, and improvements. Be concise.".to_string()),
        });

        Config {
            model: "grok-3".to_string(),
            sandbox_enabled: false,
            allowed_commands: HashMap::new(),
            roles,
            settings: Settings::default(),
            rate_limits: default_rate_limits(),
        }
    }
}

impl Config {
    /// Check if a command is allowed for the given directory
    #[allow(dead_code)]
    pub fn is_command_allowed(&self, command: &str, cwd: &str) -> bool {
        if let Some(commands) = self.allowed_commands.get(cwd) {
            commands.contains(&command.to_string())
        } else {
            false
        }
    }

    /// Add a command to the allowed list for a directory
    pub fn allow_command(&mut self, command: String, cwd: String) {
        self.allowed_commands
            .entry(cwd)
            .or_insert_with(Vec::new)
            .push(command);
    }

    /// Get a model role by name
    pub fn get_role(&self, name: &str) -> Option<&ModelRole> {
        self.roles.get(name)
    }

    /// Get rate limit config for a model (returns None if model not in config)
    pub fn get_rate_limit(&self, model: &str) -> Option<&RateLimitConfig> {
        self.rate_limits.get(model)
    }
}

fn get_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(CONFIG_DIR).join(CONFIG_FILE)
}

pub fn load_config() -> Config {
    let path = get_config_path();

    let mut config = if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => {
                serde_json::from_str(&content).unwrap_or_default()
            }
            Err(_) => Config::default(),
        }
    } else {
        Config::default()
    };

    // Ensure default roles exist if config file didn't have them
    if config.roles.is_empty() {
        let defaults = Config::default();
        config.roles = defaults.roles;
    }

    config
}

pub fn save_config(config: &Config) -> Result<(), std::io::Error> {
    let path = get_config_path();

    // Create config directory if it doesn't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(config)?;
    fs::write(&path, content)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.model, "grok-3");
        assert!(!config.sandbox_enabled);
        assert!(config.allowed_commands.is_empty());
    }

    #[test]
    fn test_allow_command() {
        let mut config = Config::default();
        config.allow_command("ls".to_string(), "/home".to_string());
        assert!(config.is_command_allowed("ls", "/home"));
        assert!(!config.is_command_allowed("pwd", "/home"));
        assert!(!config.is_command_allowed("ls", "/tmp"));
    }
}
