use crate::app::App;

impl<'a> App<'a> {
    pub fn update_autocomplete(&mut self) {
        let content = self.input.lines().join("\n");
        if content.starts_with('/') {
            self.autocomplete_active = true;
            if let Some(query) = content.strip_prefix("/model ") {
                self.autocomplete_filtered = self
                    .available_models
                    .iter()
                    .filter(|m| m.name.starts_with(query))
                    .map(|m| format!("/model {}", m.name))
                    .collect();
            } else if !content.contains(' ') {
                let query = &content;
                self.autocomplete_filtered = self
                    .autocomplete_options
                    .iter()
                    .filter(|opt| opt.starts_with(query))
                    .map(|s| s.to_string())
                    .collect();
            } else {
                self.autocomplete_active = false;
            }

            if self.autocomplete_active {
                if self.autocomplete_index >= self.autocomplete_filtered.len() {
                    self.autocomplete_index = 0;
                }
                if self.autocomplete_filtered.is_empty() {
                    self.autocomplete_active = false;
                }
            }
        } else if content.starts_with('@') {
            // Role autocomplete - show available roles with their models
            let after_at = &content[1..];

            // Check if role is already complete (has colon followed by content)
            if let Some(colon_pos) = after_at.find(':') {
                let after_colon = &after_at[colon_pos + 1..];
                if !after_colon.trim().is_empty() {
                    // Role is complete and user is typing message - don't show autocomplete
                    self.autocomplete_active = false;
                    return;
                }
            }

            self.autocomplete_active = true;

            // Extract the role query (everything after @ before any space or colon)
            let query_end = after_at.find(|c| c == ' ' || c == ':').unwrap_or(after_at.len());
            let query = &after_at[..query_end].to_lowercase();

            // Filter and format roles - format: "@role: (model)"
            // The model part will be stripped on selection
            self.autocomplete_filtered = self.config.roles
                .iter()
                .filter(|(name, _)| name.starts_with(query))
                .map(|(name, role)| format!("@{}:  ({})", name, role.model))
                .collect();

            // Sort alphabetically
            self.autocomplete_filtered.sort();

            if self.autocomplete_index >= self.autocomplete_filtered.len() {
                self.autocomplete_index = 0;
            }
            if self.autocomplete_filtered.is_empty() {
                self.autocomplete_active = false;
            }
        } else {
            self.autocomplete_active = false;
        }
    }
}