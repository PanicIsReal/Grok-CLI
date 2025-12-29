//! YAML-based tool plugin system
//!
//! Allows users to define custom tools via YAML files in ~/.config/grok-cli/tools/
//! Each tool is a separate YAML file that defines:
//! - Tool metadata (name, description)
//! - Parameters with types and descriptions
//! - The command to execute (shell command with parameter substitution)

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const TOOLS_DIR: &str = ".config/grok-cli/tools";

/// Parameter definition for a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    /// Parameter name
    pub name: String,
    /// Parameter type (string, integer, boolean, array)
    #[serde(rename = "type")]
    pub param_type: String,
    /// Human-readable description
    pub description: String,
    /// Whether this parameter is required
    #[serde(default)]
    pub required: bool,
    /// Default value (optional)
    #[serde(default)]
    pub default: Option<String>,
}

/// YAML tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YamlTool {
    /// Tool name (used as function name)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Parameters the tool accepts
    #[serde(default)]
    pub parameters: Vec<ToolParameter>,
    /// Shell command to execute. Use {{param_name}} for parameter substitution
    pub command: String,
    /// Working directory for the command (optional, defaults to cwd)
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Whether to respect sandbox mode (default: true)
    #[serde(default = "default_true")]
    pub sandbox_aware: bool,
    /// Category for organization (optional)
    #[serde(default)]
    pub category: Option<String>,
    /// Icon/emoji for display (optional)
    #[serde(default)]
    pub icon: Option<String>,
}

fn default_true() -> bool {
    true
}

impl YamlTool {
    /// Convert to OpenAI-compatible function definition
    pub fn to_api_definition(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for param in &self.parameters {
            let mut prop = serde_json::Map::new();
            prop.insert("type".to_string(), json!(param.param_type));
            prop.insert("description".to_string(), json!(param.description));
            properties.insert(param.name.clone(), Value::Object(prop));

            if param.required {
                required.push(json!(param.name));
            }
        }

        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required
                }
            }
        })
    }

    /// Execute the tool with given arguments
    pub fn execute(&self, args: &Value, sandbox_cwd: Option<&str>) -> String {
        // Build the command with parameter substitution
        let mut command = self.command.clone();

        // Substitute parameters
        for param in &self.parameters {
            let placeholder = format!("{{{{{}}}}}", param.name);
            let value = args.get(&param.name)
                .and_then(|v| {
                    match v {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        Value::Array(arr) => Some(arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                            .join(" ")),
                        _ => None,
                    }
                })
                .or_else(|| param.default.clone())
                .unwrap_or_default();

            command = command.replace(&placeholder, &shell_escape(&value));
        }

        // Determine working directory
        let work_dir = self.working_dir.as_deref().unwrap_or(".");

        // Check sandbox constraints
        if self.sandbox_aware {
            if let Some(cwd) = sandbox_cwd {
                // Verify we're working within sandbox
                let work_path = std::path::Path::new(work_dir);
                let cwd_path = std::path::Path::new(cwd);

                if work_path.is_absolute() {
                    if let (Ok(work_canonical), Ok(cwd_canonical)) =
                        (work_path.canonicalize(), cwd_path.canonicalize())
                    {
                        if !work_canonical.starts_with(&cwd_canonical) {
                            return format!("Error: Tool working directory is outside sandbox: {}", work_dir);
                        }
                    }
                }
            }
        }

        // Execute the command
        let output = if let Some(cwd) = sandbox_cwd {
            Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(cwd)
                .output()
        } else {
            Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(work_dir)
                .output()
        };

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);

                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    "(no output)".to_string()
                } else {
                    result
                }
            }
            Err(e) => format!("Error executing command: {}", e),
        }
    }
}

/// Escape special shell characters
fn shell_escape(s: &str) -> String {
    // Simple escaping - wrap in single quotes and escape any single quotes
    format!("'{}'", s.replace("'", "'\"'\"'"))
}

/// Tool plugin manager
pub struct ToolPluginManager {
    /// Loaded tools from YAML files
    tools: HashMap<String, YamlTool>,
}

impl ToolPluginManager {
    /// Create a new plugin manager and load tools from the tools directory
    pub fn new() -> Self {
        let mut manager = ToolPluginManager {
            tools: HashMap::new(),
        };
        manager.load_tools();
        manager
    }

    /// Get the tools directory path
    fn get_tools_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(TOOLS_DIR)
    }

    /// Load all tools from YAML files in the tools directory
    pub fn load_tools(&mut self) {
        let tools_dir = Self::get_tools_dir();

        if !tools_dir.exists() {
            // Create the tools directory if it doesn't exist
            if let Err(e) = fs::create_dir_all(&tools_dir) {
                eprintln!("Warning: Could not create tools directory: {}", e);
                return;
            }
        }

        // Find all YAML files
        let pattern = tools_dir.join("*.yaml");
        let pattern_str = pattern.to_string_lossy();

        if let Ok(entries) = glob::glob(&pattern_str) {
            for entry in entries.flatten() {
                if let Err(e) = self.load_tool_file(&entry) {
                    eprintln!("Warning: Failed to load tool {}: {}", entry.display(), e);
                }
            }
        }

        // Also check .yml extension
        let pattern = tools_dir.join("*.yml");
        let pattern_str = pattern.to_string_lossy();

        if let Ok(entries) = glob::glob(&pattern_str) {
            for entry in entries.flatten() {
                if let Err(e) = self.load_tool_file(&entry) {
                    eprintln!("Warning: Failed to load tool {}: {}", entry.display(), e);
                }
            }
        }
    }

    /// Load a single tool from a YAML file
    fn load_tool_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read file: {}", e))?;

        let tool: YamlTool = serde_yaml::from_str(&content)
            .map_err(|e| format!("Failed to parse YAML: {}", e))?;

        // Validate required fields
        if tool.name.is_empty() {
            return Err("Tool name is required".to_string());
        }
        if tool.description.is_empty() {
            return Err("Tool description is required".to_string());
        }
        if tool.command.is_empty() {
            return Err("Tool command is required".to_string());
        }

        self.tools.insert(tool.name.clone(), tool);
        Ok(())
    }

    /// Get all loaded tools
    pub fn get_tools(&self) -> &HashMap<String, YamlTool> {
        &self.tools
    }

    /// Get a specific tool by name
    pub fn get_tool(&self, name: &str) -> Option<&YamlTool> {
        self.tools.get(name)
    }

    /// Get API definitions for all plugin tools
    pub fn get_api_definitions(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.to_api_definition()).collect()
    }

    /// Execute a plugin tool
    pub fn execute_tool(&self, name: &str, args: &str, sandbox_cwd: Option<&str>) -> Option<String> {
        if let Some(tool) = self.tools.get(name) {
            let args_value: Value = serde_json::from_str(args).unwrap_or(json!({}));
            Some(tool.execute(&args_value, sandbox_cwd))
        } else {
            None
        }
    }

    /// Get a list of tool names with their descriptions (for system prompt)
    pub fn get_tool_descriptions(&self) -> Vec<(String, String, Option<String>)> {
        self.tools.values()
            .map(|t| (t.name.clone(), t.description.clone(), t.icon.clone()))
            .collect()
    }
}

/// Global plugin manager instance
lazy_static::lazy_static! {
    pub static ref TOOL_PLUGINS: std::sync::Mutex<ToolPluginManager> =
        std::sync::Mutex::new(ToolPluginManager::new());
}

/// Reload plugins from disk
pub fn reload_plugins() {
    if let Ok(mut manager) = TOOL_PLUGINS.lock() {
        manager.tools.clear();
        manager.load_tools();
    }
}

/// Get API definitions from plugins
pub fn get_plugin_definitions() -> Vec<Value> {
    TOOL_PLUGINS.lock()
        .map(|m| m.get_api_definitions())
        .unwrap_or_default()
}

/// Execute a plugin tool
pub fn execute_plugin_tool(name: &str, args: &str, sandbox_cwd: Option<&str>) -> Option<String> {
    TOOL_PLUGINS.lock().ok()?.execute_tool(name, args, sandbox_cwd)
}

/// Check if a tool name is a plugin
pub fn is_plugin_tool(name: &str) -> bool {
    TOOL_PLUGINS.lock()
        .map(|m| m.tools.contains_key(name))
        .unwrap_or(false)
}

/// Get plugin tool descriptions for system prompt
pub fn get_plugin_tool_descriptions() -> Vec<(String, String, Option<String>)> {
    TOOL_PLUGINS.lock()
        .map(|m| m.get_tool_descriptions())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yaml_tool_definition() {
        let yaml = r#"
name: test_tool
description: A test tool
parameters:
  - name: query
    type: string
    description: The search query
    required: true
command: echo "{{query}}"
"#;

        let tool: YamlTool = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.parameters.len(), 1);
        assert!(tool.parameters[0].required);
    }

    #[test]
    fn test_tool_execution() {
        let tool = YamlTool {
            name: "echo_tool".to_string(),
            description: "Echo input".to_string(),
            parameters: vec![
                ToolParameter {
                    name: "message".to_string(),
                    param_type: "string".to_string(),
                    description: "Message to echo".to_string(),
                    required: true,
                    default: None,
                }
            ],
            command: "echo {{message}}".to_string(),
            working_dir: None,
            sandbox_aware: true,
            category: None,
            icon: None,
        };

        let args = json!({"message": "hello world"});
        let result = tool.execute(&args, None);
        assert!(result.contains("hello world"));
    }
}
