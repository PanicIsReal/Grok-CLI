use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// User-toggleable settings for the CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Enable rate limiter - pauses execution when approaching API limits
    #[serde(default = "default_true")]
    pub rate_limiter_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            rate_limiter_enabled: true,
        }
    }
}

/// Setting metadata for display in the settings modal
#[derive(Debug, Clone)]
pub struct SettingInfo {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

/// Get all available settings with their metadata
pub fn get_settings_info() -> Vec<SettingInfo> {
    vec![
        SettingInfo {
            key: "rate_limiter_enabled",
            name: "Rate Limiter",
            description: "Pauses execution when approaching the API context/min rate limit until it clears",
        },
    ]
}

/// Rate limit configuration for a specific model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum context window size (tokens)
    pub max_context: usize,
    /// Tokens per minute limit
    pub tpm: usize,
    /// Requests per minute limit
    pub rpm: usize,
}

impl RateLimitConfig {
    pub fn new(max_context: usize, tpm: usize, rpm: usize) -> Self {
        RateLimitConfig { max_context, tpm, rpm }
    }
}

/// Default rate limits for known models
pub fn default_rate_limits() -> HashMap<String, RateLimitConfig> {
    let mut limits = HashMap::new();

    // grok-code-fast-1: 256K context, 2M TPM, 480 RPM
    limits.insert("grok-code-fast-1".to_string(), RateLimitConfig::new(256_000, 2_000_000, 480));

    // grok-3: 131K context, estimate 1M TPM, 300 RPM
    limits.insert("grok-3".to_string(), RateLimitConfig::new(131_072, 1_000_000, 300));

    // grok-3-mini: 131K context, estimate 1.5M TPM, 400 RPM
    limits.insert("grok-3-mini".to_string(), RateLimitConfig::new(131_072, 1_500_000, 400));

    // grok-4 variants: 2M context, estimate 3M TPM, 300 RPM
    limits.insert("grok-4-1-fast-reasoning".to_string(), RateLimitConfig::new(2_000_000, 3_000_000, 300));
    limits.insert("grok-4-1-fast-non-reasoning".to_string(), RateLimitConfig::new(2_000_000, 3_000_000, 300));
    limits.insert("grok-4-fast-reasoning".to_string(), RateLimitConfig::new(2_000_000, 3_000_000, 300));
    limits.insert("grok-4-fast-non-reasoning".to_string(), RateLimitConfig::new(2_000_000, 3_000_000, 300));
    limits.insert("grok-4-0709".to_string(), RateLimitConfig::new(256_000, 2_000_000, 480));

    // grok-2-vision: 32K context
    limits.insert("grok-2-vision-1212".to_string(), RateLimitConfig::new(32_768, 500_000, 200));

    limits
}

/// State for the settings modal
#[derive(Debug, Clone)]
pub struct SettingsModalState {
    pub selected_index: usize,
    pub settings_list: Vec<SettingInfo>,
}

impl SettingsModalState {
    pub fn new() -> Self {
        SettingsModalState {
            selected_index: 0,
            settings_list: get_settings_info(),
        }
    }

    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected_index < self.settings_list.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    pub fn current_setting_key(&self) -> Option<&'static str> {
        self.settings_list.get(self.selected_index).map(|s| s.key)
    }
}
