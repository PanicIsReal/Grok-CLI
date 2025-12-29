use glob::glob;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use crate::tool_plugins;

/// Default patterns to ignore (used when no .grokignore exists)
const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    "*.log",
    ".*",  // Hidden files/dirs
];

/// Read ignore patterns from .grokignore file, or return defaults
pub fn read_grokignore() -> Vec<String> {
    let grokignore_path = Path::new(".grokignore");

    if grokignore_path.exists() {
        match fs::read_to_string(grokignore_path) {
            Ok(content) => {
                content
                    .lines()
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(|s| s.to_string())
                    .collect()
            }
            Err(_) => default_ignore_patterns(),
        }
    } else {
        default_ignore_patterns()
    }
}

/// Get default ignore patterns
fn default_ignore_patterns() -> Vec<String> {
    DEFAULT_IGNORE_PATTERNS.iter().map(|s| s.to_string()).collect()
}

/// Create a default .grokignore file in the current working directory
/// Returns Ok(true) if created, Ok(false) if already exists, Err on failure
pub fn create_default_grokignore() -> Result<bool, std::io::Error> {
    let grokignore_path = Path::new(".grokignore");

    if grokignore_path.exists() {
        return Ok(false);
    }

    let default_content = vec![
        "# Grok CLI ignore patterns",
        "# Lines starting with # are comments",
        "",
        "# Hidden files/directories",
        ".*",
        "",
        "# Build directories",
        "target",
        "node_modules",
        "__pycache__",
        ".venv",
        "venv",
        "",
        "# Log files",
        "*.log",
    ];

    fs::write(grokignore_path, default_content.join("\n"))?;
    Ok(true)
}

/// Check if a path should be ignored based on patterns
fn should_ignore(path: &str, patterns: &[String]) -> bool {
    let path_obj = Path::new(path);

    for pattern in patterns {
        // Check each component of the path
        for component in path_obj.components() {
            if let std::path::Component::Normal(name) = component {
                let name_str = name.to_string_lossy();

                // Direct match
                if name_str == *pattern {
                    return true;
                }

                // Glob match for file patterns (e.g., *.log)
                if pattern.contains('*') || pattern.contains('?') {
                    if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                        if glob_pattern.matches(&name_str) {
                            return true;
                        }
                    }
                }

                // Hidden file check (pattern ".*")
                if pattern == ".*" && name_str.starts_with('.') {
                    return true;
                }
            }
        }
    }

    false
}

/// Generate a git-style diff snippet showing removals and additions
fn generate_diff_snippet(old_string: &str, new_string: &str) -> String {
    let old_lines: Vec<&str> = old_string.lines().collect();
    let new_lines: Vec<&str> = new_string.lines().collect();

    let mut result = String::new();

    // Show line count change header
    let old_count = old_lines.len();
    let new_count = new_lines.len();
    result.push_str(&format!("@@ -{} lines +{} lines @@\n", old_count, new_count));

    // Show removed lines (with -)
    for line in &old_lines {
        result.push_str(&format!("-  {}\n", line));
    }

    // Show added lines (with +)
    for line in &new_lines {
        result.push_str(&format!("+  {}\n", line));
    }

    result.trim_end().to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

pub fn get_tool_definitions() -> Vec<Value> {
    let mut tools = vec![
        // === Bash ===
        json!({
            "type": "function",
            "function": {
                "name": "Bash",
                "description": "Executes a shell command. Use for git, build commands, running programs, installing packages, etc. Do NOT use for file reading/writing - use the dedicated tools instead.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute"
                        },
                        "description": {
                            "type": "string",
                            "description": "Brief description of what this command does (5-10 words)"
                        }
                    },
                    "required": ["command"]
                }
            }
        }),
        // === Read ===
        json!({
            "type": "function",
            "function": {
                "name": "Read",
                "description": "Reads a file from the filesystem. Returns content with line numbers. You MUST read a file before editing it. For large files, use offset and limit to read specific sections.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "The absolute or relative path to the file to read"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Line number to start reading from (1-indexed). Only use for large files."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of lines to read. Only use for large files."
                        }
                    },
                    "required": ["file_path"]
                }
            }
        }),
        // === Edit ===
        json!({
            "type": "function",
            "function": {
                "name": "Edit",
                "description": "Performs exact string replacement in a file. You MUST Read the file first before editing. The old_string must match EXACTLY including all whitespace and indentation. The edit will FAIL if old_string is not found or is not unique in the file. To make old_string unique, include more surrounding context. Use replace_all only for renaming variables/functions across the file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "The path to the file to edit"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "The exact text to find and replace. Must match the file content exactly, including whitespace and indentation. Copy this directly from the Read output."
                        },
                        "new_string": {
                            "type": "string",
                            "description": "The new text to replace old_string with. Must be different from old_string."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace all occurrences instead of requiring uniqueness. Use for renaming variables/functions. Default: false"
                        }
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }
            }
        }),
        // === Write ===
        json!({
            "type": "function",
            "function": {
                "name": "Write",
                "description": "Writes content to a file, replacing existing content or creating a new file. Creates parent directories if needed. For editing existing files, prefer Edit instead. Use Write for creating new files or complete rewrites.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "The path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "The complete content to write to the file"
                        }
                    },
                    "required": ["file_path", "content"]
                }
            }
        }),
        // === Glob ===
        json!({
            "type": "function",
            "function": {
                "name": "Glob",
                "description": "Fast file pattern matching. Use to find files by name patterns. Examples: '**/*.rs' (all Rust files), 'src/**/*.ts' (TypeScript in src), 'test_*.py' (Python test files).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern to match files against"
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search in. Defaults to current directory."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        // === Grep ===
        json!({
            "type": "function",
            "function": {
                "name": "Grep",
                "description": "Search for text patterns in files. Returns matching lines with file paths and line numbers. Use for finding code, function definitions, usages, etc.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Text or regex pattern to search for"
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory to search in. Defaults to current directory."
                        },
                        "include": {
                            "type": "string",
                            "description": "Only search files matching this glob pattern (e.g., '*.rs', '*.py')"
                        },
                        "context_lines": {
                            "type": "integer",
                            "description": "Number of context lines to show before and after each match"
                        }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        // === List ===
        json!({
            "type": "function",
            "function": {
                "name": "List",
                "description": "Lists files and directories at the specified path. Shows directories first (with trailing /), then files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory to list. Defaults to current directory."
                        }
                    },
                    "required": []
                }
            }
        }),
        // === FileInfo ===
        json!({
            "type": "function",
            "function": {
                "name": "FileInfo",
                "description": "Get metadata about a file or directory, including size, permissions, modification time, and whether it exists.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "The path to check"
                        }
                    },
                    "required": ["path"]
                }
            }
        }),
        // === Planning Tools ===
        json!({
            "type": "function",
            "function": {
                "name": "AskUser",
                "description": "Ask the user a multiple choice question to clarify requirements or get input during planning.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question to ask the user"
                        },
                        "options": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The available options for the user to choose from"
                        }
                    },
                    "required": ["question", "options"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "ConfirmPlan",
                "description": "Present a plan to the user for confirmation before executing. Use after gathering requirements.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "plan": {
                            "type": "string",
                            "description": "The detailed plan to present to the user"
                        }
                    },
                    "required": ["plan"]
                }
            }
        }),
        // === WebSearch ===
        json!({
            "type": "function",
            "function": {
                "name": "WebSearch",
                "description": "Search the web for information. Use when you need current information, facts, documentation, or anything not in your training data. Requires user approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        // === TodoWrite ===
        json!({
            "type": "function",
            "function": {
                "name": "TodoWrite",
                "description": "Update task list to track progress. IMPORTANT: Always include ALL existing tasks when updating - mark completed ones as 'completed', don't remove them. Only ONE task should be 'in_progress' at a time. Update status as you complete tasks rather than creating new lists.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "todos": {
                            "type": "array",
                            "description": "The full todo list including completed items. Preserve completed tasks, update statuses.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "content": {
                                        "type": "string",
                                        "description": "What needs to be done (imperative form, e.g., 'Fix the bug')"
                                    },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "completed"],
                                        "description": "Current status of the task"
                                    },
                                    "activeForm": {
                                        "type": "string",
                                        "description": "Present continuous form shown during execution (e.g., 'Fixing the bug')"
                                    }
                                },
                                "required": ["content", "status", "activeForm"]
                            }
                        }
                    },
                    "required": ["todos"]
                }
            }
        }),
    ];

    // Add plugin tools from YAML files
    let plugin_defs = tool_plugins::get_plugin_definitions();
    tools.extend(plugin_defs);

    tools
}

/// Get a list of all available tool names (builtin + plugins)
pub fn get_all_tool_names() -> Vec<String> {
    let mut names = vec![
        "Bash".to_string(),
        "Read".to_string(),
        "Edit".to_string(),
        "Write".to_string(),
        "Glob".to_string(),
        "Grep".to_string(),
        "List".to_string(),
        "FileInfo".to_string(),
        "AskUser".to_string(),
        "ConfirmPlan".to_string(),
        "WebSearch".to_string(),
        "TodoWrite".to_string(),
    ];

    // Add plugin tool names
    for (name, _, _) in tool_plugins::get_plugin_tool_descriptions() {
        names.push(name);
    }

    names
}

fn is_path_in_sandbox(path: &str, sandbox_cwd: &str) -> bool {
    let path = Path::new(path);
    let sandbox = Path::new(sandbox_cwd);

    let canonical_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // Path doesn't exist yet - check parent
            if let Some(parent) = path.parent() {
                if parent.as_os_str().is_empty() {
                    // Relative path with no parent - allow in sandbox
                    return true;
                }
                match parent.canonicalize() {
                    Ok(p) => p.join(path.file_name().unwrap_or_default()),
                    Err(_) => return false,
                }
            } else {
                return false;
            }
        }
    };

    let canonical_sandbox = match sandbox.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };

    canonical_path.starts_with(&canonical_sandbox)
}

pub fn execute_tool(name: &str, arguments: &str, sandbox_cwd: Option<&str>) -> String {
    // First check if this is a plugin tool
    if tool_plugins::is_plugin_tool(name) {
        return tool_plugins::execute_plugin_tool(name, arguments, sandbox_cwd)
            .unwrap_or_else(|| format!("Error: Plugin tool '{}' failed to execute", name));
    }

    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));

    match name {
        "Bash" | "run_shell_command" => {
            let command = args["command"].as_str().unwrap_or("");

            if command.is_empty() {
                return "Error: command is required".to_string();
            }

            let output = if let Some(cwd) = sandbox_cwd {
                Command::new("sh")
                    .arg("-c")
                    .arg(format!("cd {} && {}", cwd, command))
                    .output()
            } else {
                Command::new("sh").arg("-c").arg(command).output()
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

        "Read" | "read_file" | "read_lines" => {
            let file_path = args["file_path"].as_str().unwrap_or("");
            let offset = args["offset"].as_u64().map(|n| n as usize);
            let limit = args["limit"].as_u64().map(|n| n as usize);
            // Also support start_line/end_line for backwards compatibility
            let start_line = args["start_line"].as_u64().map(|n| n as usize);
            let end_line = args["end_line"].as_u64().map(|n| n as usize);

            if file_path.is_empty() {
                return "Error: file_path is required".to_string();
            }

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(file_path, cwd) {
                    return format!("Error: Cannot read files outside of {}", cwd);
                }
            }

            match fs::read_to_string(file_path) {
                Ok(content) => {
                    if content.len() > 10_000_000 {
                        return format!("Error: File too large ({} bytes). Use offset and limit for large files.", content.len());
                    }
                    let lines: Vec<&str> = content.lines().collect();
                    let total_lines = lines.len();

                    if total_lines == 0 {
                        return "(empty file)".to_string();
                    }

                    // Determine range
                    let start = offset.or(start_line).unwrap_or(1).max(1);
                    let end = if let Some(lim) = limit {
                        (start + lim - 1).min(total_lines)
                    } else if let Some(el) = end_line {
                        el.min(total_lines)
                    } else {
                        total_lines
                    };

                    if start > total_lines {
                        return format!(
                            "Error: offset {} exceeds file length ({} lines)",
                            start, total_lines
                        );
                    }

                    // Build output with line numbers (cat -n style)
                    let mut result = String::new();
                    let width = total_lines.to_string().len().max(4);

                    for (idx, line) in lines.iter().enumerate() {
                        let line_num = idx + 1;
                        if line_num >= start && line_num <= end {
                            // Format: "   1\t" style like cat -n
                            result.push_str(&format!("{:>width$}\t{}\n", line_num, line, width = width));
                        }
                    }

                    // Add indicator if showing partial file
                    if start > 1 || end < total_lines {
                        result.push_str(&format!(
                            "\n[Showing lines {}-{} of {}. Use offset/limit to see more.]",
                            start, end, total_lines
                        ));
                    }

                    result
                }
                Err(e) => format!("Error reading file: {}", e),
            }
        }

        "Edit" | "edit_file" => {
            let file_path = args["file_path"].as_str().unwrap_or("");
            let old_string = args["old_string"].as_str().unwrap_or("");
            let new_string = args["new_string"].as_str().unwrap_or("");
            let replace_all = args["replace_all"].as_bool().unwrap_or(false);

            if file_path.is_empty() {
                return "Error: file_path is required".to_string();
            }

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(file_path, cwd) {
                    return format!("Error: Cannot edit files outside of {}", cwd);
                }
            }

            if old_string.is_empty() {
                return "Error: old_string cannot be empty".to_string();
            }

            if old_string == new_string {
                return "✓ No changes needed - strings are identical".to_string();
            }

            // Use transactional file operation
            match crate::transactions::execute_file_operation(file_path, || {
                // Read the file
                let content = fs::read_to_string(file_path)?;

                // Count occurrences
                let count = content.matches(old_string).count();

                if count == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Error: old_string not found in {}\n\n\
                             The text must match EXACTLY, including:\n\
                             - All whitespace and indentation\n\
                             - Line endings\n\
                             - Any special characters\n\n\
                             Tip: Copy the exact text from the Read output.",
                            file_path
                        )
                    ));
                }

                if count > 1 && !replace_all {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "Error: old_string appears {} times in the file.\n\n\
                            To fix:\n\
                            - Include more surrounding context to make old_string unique, OR\n\
                            - Use replace_all: true to replace all occurrences",
                            count
                        )
                    ));
                }

                // Perform the replacement
                let new_content = if replace_all {
                    content.replace(old_string, new_string)
                } else {
                    content.replacen(old_string, new_string, 1)
                };

                // Write the file
                fs::write(file_path, &new_content)?;

                Ok(count)
            }) {
                Ok(count) => {
                    let diff = generate_diff_snippet(old_string, new_string);
                    if replace_all && count > 1 {
                        format!("{}\n\n{}\n\n✓ Replaced {} occurrences in {}", file_path, diff, count, file_path)
                    } else {
                        format!("{}\n\n{}\n\n✓ Successfully edited", file_path, diff)
                    }
                }
                Err(e) => e.to_string(),
            }
        }

        "Write" | "write_file" => {
            let file_path = args["file_path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");

            if file_path.is_empty() {
                return "Error: file_path is required".to_string();
            }

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(file_path, cwd) {
                    return format!("Error: Cannot write files outside of {}", cwd);
                }
            }

            // Use transactional file operation
            match crate::transactions::execute_file_operation(file_path, || {
                // Create parent directories if needed
                if let Some(parent) = Path::new(file_path).parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        fs::create_dir_all(parent)?;
                    }
                }

                fs::write(file_path, content)?;
                Ok(())
            }) {
                Ok(_) => format!("Successfully wrote to {}", file_path),
                Err(e) => format!("Error writing file: {}", e),
            }
        }

        "Glob" | "glob_files" => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let base_path = args["path"].as_str().unwrap_or(".");

            if pattern.is_empty() {
                return "Error: pattern is required".to_string();
            }

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(base_path, cwd) {
                    return format!("Error: Cannot search outside of {}", cwd);
                }
            }

            // Read ignore patterns from .grokignore
            let ignore_patterns = read_grokignore();

            let full_pattern = if base_path == "." {
                pattern.to_string()
            } else {
                format!("{}/{}", base_path.trim_end_matches('/'), pattern)
            };

            match glob(&full_pattern) {
                Ok(paths) => {
                    let mut results: Vec<String> = Vec::new();
                    for entry in paths.flatten() {
                        if let Some(path_str) = entry.to_str() {
                            // Check sandbox
                            if let Some(cwd) = sandbox_cwd {
                                if !is_path_in_sandbox(path_str, cwd) {
                                    continue;
                                }
                            }
                            // Check ignore patterns
                            if should_ignore(path_str, &ignore_patterns) {
                                continue;
                            }
                            results.push(path_str.to_string());
                        }
                    }

                    results.sort();

                    if results.is_empty() {
                        "No matching files found".to_string()
                    } else {
                        results.join("\n")
                    }
                }
                Err(e) => format!("Error in glob pattern: {}", e),
            }
        }

        "Grep" | "grep" | "search_files" | "search_content" => {
            let pattern = args["pattern"].as_str()
                .or_else(|| args["query"].as_str())
                .unwrap_or("");
            let path = args["path"].as_str().unwrap_or(".");
            let include = args["include"].as_str();
            let context = args["context_lines"].as_u64().map(|n| n as usize);

            if pattern.is_empty() {
                return "Error: pattern is required".to_string();
            }

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(path, cwd) {
                    return format!("Error: Cannot search outside of {}", cwd);
                }
            }

            // Read ignore patterns from .grokignore
            let ignore_patterns = read_grokignore();

            let mut grep_args = vec!["-rn", "--color=never"];

            // Build exclusion args from .grokignore patterns
            let mut exclude_strings: Vec<String> = Vec::new();
            for pat in &ignore_patterns {
                if pat.contains('*') || pat.contains('?') {
                    // File pattern like *.log
                    exclude_strings.push(format!("--exclude={}", pat));
                } else if pat == ".*" {
                    // Hidden files/dirs
                    exclude_strings.push("--exclude=.*".to_string());
                    exclude_strings.push("--exclude-dir=.*".to_string());
                } else {
                    // Directory pattern
                    exclude_strings.push(format!("--exclude-dir={}", pat));
                }
            }

            // Convert to &str for grep args
            for excl in &exclude_strings {
                grep_args.push(excl.as_str());
            }

            // Add context lines if specified
            if let Some(ctx) = context {
                grep_args.push("-C");
                // We need to convert to string and keep it alive
                let ctx_str = ctx.to_string();
                grep_args.push(Box::leak(ctx_str.into_boxed_str()));
            }

            if let Some(inc) = include {
                grep_args.push("--include");
                grep_args.push(inc);
            }

            grep_args.push(pattern);
            grep_args.push(path);

            match Command::new("grep").args(&grep_args).output() {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if stdout.is_empty() {
                        "No matches found".to_string()
                    } else {
                        // Limit output to prevent overwhelming responses
                        let lines: Vec<&str> = stdout.lines().collect();
                        let total = lines.len();

                        if total > 100 {
                            let truncated: Vec<&str> = lines.into_iter().take(100).collect();
                            format!(
                                "{}\n\n... {} more lines (showing first 100)",
                                truncated.join("\n"),
                                total - 100
                            )
                        } else {
                            stdout.trim_end().to_string()
                        }
                    }
                }
                Err(e) => format!("Error running grep: {}", e),
            }
        }

        "List" | "list_directory" => {
            let path = args["path"].as_str().unwrap_or(".");

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(path, cwd) {
                    return format!("Error: Cannot access directories outside of {}", cwd);
                }
            }

            match fs::read_dir(path) {
                Ok(entries) => {
                    let mut dirs = Vec::new();
                    let mut files = Vec::new();

                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if entry.path().is_dir() {
                            dirs.push(format!("{}/", name));
                        } else {
                            files.push(name);
                        }
                    }

                    dirs.sort();
                    files.sort();

                    let mut result = String::new();
                    for d in dirs {
                        result.push_str(&d);
                        result.push('\n');
                    }
                    for f in files {
                        result.push_str(&f);
                        result.push('\n');
                    }

                    if result.is_empty() {
                        "(empty directory)".to_string()
                    } else {
                        result.trim_end().to_string()
                    }
                }
                Err(e) => format!("Error listing directory: {}", e),
            }
        }

        "FileInfo" | "file_info" => {
            let path = args["path"].as_str().unwrap_or("");

            if path.is_empty() {
                return "Error: path is required".to_string();
            }

            if let Some(cwd) = sandbox_cwd {
                if !is_path_in_sandbox(path, cwd) {
                    return format!("Error: Cannot access paths outside of {}", cwd);
                }
            }

            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(e) => return format!("Error getting metadata for {}: {}", path, e),
            };

            let file_type = if metadata.is_file() {
                "file"
            } else if metadata.is_dir() {
                "directory"
            } else {
                "other"
            };

            let size = metadata.len();
            let modified = metadata.modified()
                .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())
                .unwrap_or(0);

            let permissions = metadata.permissions();
            let readonly = permissions.readonly();

            format!(
                "Path: {}\nType: {}\nSize: {} bytes\nModified: {} (Unix timestamp)\nRead-only: {}",
                path, file_type, size, modified, readonly
            )
        }

        "AskUser" | "ask_multiple_choice" => {
            "Tool handled by application".to_string()
        }

        "ConfirmPlan" | "confirm_plan" => {
            "Tool handled by application".to_string()
        }

        "WebSearch" | "web_search" => {
            // This is handled specially in process_conversation for approval
            // But if called directly, execute the search
            let query = args["query"].as_str().unwrap_or("");
            if query.is_empty() {
                return "Error: query is required".to_string();
            }
            execute_web_search(query)
        }

        _ => format!("Unknown tool: {}", name),
    }
}

/// Execute a web search using DuckDuckGo
pub fn execute_web_search(query: &str) -> String {
    let encoded_query = query.replace(' ', "+");
    let url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);

    // Use curl for simplicity (available on most systems)
    let output = Command::new("curl")
        .args(["-s", "-L", "-A", "Mozilla/5.0", &url])
        .output();

    match output {
        Ok(out) => {
            let html = String::from_utf8_lossy(&out.stdout);
            parse_duckduckgo_results(&html)
        }
        Err(e) => format!("Error performing search: {}", e),
    }
}

/// Parse DuckDuckGo HTML results
fn parse_duckduckgo_results(html: &str) -> String {
    let mut results = Vec::new();

    // Simple parsing - look for result links and snippets
    // DuckDuckGo HTML has results in <a class="result__a"> tags
    for line in html.lines() {
        if line.contains("result__a") && line.contains("href=") {
            // Extract title from the link text
            if let Some(start) = line.find('>') {
                if let Some(end) = line[start..].find("</a>") {
                    let title = &line[start + 1..start + end];
                    let title = title.replace("&amp;", "&")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&quot;", "\"");
                    if !title.trim().is_empty() && !title.contains('<') {
                        results.push(format!("• {}", title.trim()));
                    }
                }
            }
        }
        if line.contains("result__snippet") {
            // Extract snippet text
            if let Some(start) = line.find('>') {
                if let Some(end) = line[start..].find("</") {
                    let snippet = &line[start + 1..start + end];
                    let snippet = snippet.replace("&amp;", "&")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&quot;", "\"")
                        .replace("<b>", "")
                        .replace("</b>", "");
                    if !snippet.trim().is_empty() && results.len() > 0 {
                        results.push(format!("  {}", snippet.trim()));
                        results.push(String::new());
                    }
                }
            }
        }

        if results.len() >= 15 { // Limit to ~5 results with snippets
            break;
        }
    }

    if results.is_empty() {
        "No results found or unable to parse search results.".to_string()
    } else {
        results.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_with_line_numbers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let args = json!({
            "file_path": file_path.to_str().unwrap()
        }).to_string();

        let result = execute_tool("Read", &args, None);
        assert!(result.contains("1\t"));
        assert!(result.contains("line 1"));
        assert!(result.contains("2\t"));
        assert!(result.contains("line 2"));
    }

    #[test]
    fn test_read_with_offset_limit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let args = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 2,
            "limit": 2
        }).to_string();

        let result = execute_tool("Read", &args, None);
        assert!(result.contains("line 2"));
        assert!(result.contains("line 3"));
        assert!(!result.contains("line 1"));
        assert!(!result.contains("line 4"));
    }

    #[test]
    fn test_edit_exact_match() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "hello world\nfoo bar\n").unwrap();

        let args = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "foo bar",
            "new_string": "baz qux"
        }).to_string();

        let result = execute_tool("Edit", &args, None);
        assert!(result.contains("Successfully"));

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("baz qux"));
        assert!(!content.contains("foo bar"));
    }

    #[test]
    fn test_edit_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "hello world\n").unwrap();

        let args = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "not here",
            "new_string": "replacement"
        }).to_string();

        let result = execute_tool("Edit", &args, None);
        assert!(result.contains("not found"));
    }

    #[test]
    fn test_edit_multiple_occurrences_fails() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "foo\nfoo\nfoo\n").unwrap();

        let args = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar"
        }).to_string();

        let result = execute_tool("Edit", &args, None);
        assert!(result.contains("appears 3 times"));
    }

    #[test]
    fn test_edit_replace_all() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "foo\nfoo\nfoo\n").unwrap();

        let args = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar",
            "replace_all": true
        }).to_string();

        let result = execute_tool("Edit", &args, None);
        assert!(result.contains("Replaced 3 occurrences"));
        assert!(result.contains("-  foo"));
        assert!(result.contains("+  bar"));

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "bar\nbar\nbar\n");
    }

    #[test]
    fn test_write_creates_directories() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("subdir/nested/test.txt");

        let args = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello"
        }).to_string();

        let result = execute_tool("Write", &args, None);
        assert!(result.contains("Successfully"));
        assert!(file_path.exists());
    }

    #[test]
    fn test_file_info() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "hello world").unwrap();

        let args = json!({
            "path": file_path.to_str().unwrap()
        }).to_string();

        let result = execute_tool("FileInfo", &args, None);
        assert!(result.contains("Path:"));
        assert!(result.contains("Type: file"));
        assert!(result.contains("Size: 11 bytes"));
        assert!(result.contains("Modified:"));
        assert!(result.contains("Read-only:"));
    }

    #[test]
    fn test_file_info_directory() {
        let temp_dir = tempfile::tempdir().unwrap();

        let args = json!({
            "path": temp_dir.path().to_str().unwrap()
        }).to_string();

        let result = execute_tool("FileInfo", &args, None);
        assert!(result.contains("Type: directory"));
    }

    #[test]
    fn test_file_info_nonexistent() {
        let args = json!({
            "path": "/nonexistent/path"
        }).to_string();

        let result = execute_tool("FileInfo", &args, None);
        assert!(result.contains("Error getting metadata"));
    }
}
