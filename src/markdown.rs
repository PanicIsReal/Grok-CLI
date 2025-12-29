use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

#[allow(dead_code)]
pub fn render_markdown(text: &str, width: usize) -> Vec<ListItem<'static>> {
    render_markdown_lines(text, width)
        .into_iter()
        .map(ListItem::new)
        .collect()
}

/// Returns Lines instead of ListItems for more flexible composition
pub fn render_markdown_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;

    for line in text.lines() {
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
            // Render the marker itself
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Yellow)
            )));
            continue;
        }

        if in_code_block {
            let wrapped = wrap_code(line, width);
            for w in wrapped {
                lines.push(Line::from(Span::styled(
                    format!("  {}", w), // Indent code
                    Style::default().fg(Color::Cyan)
                )));
            }
        } else {
            // Normal text processing
            if line.starts_with("# ") {
                 let content = line.trim_start_matches("# ").trim();
                 let wrapped = wrap_text(content, width);
                 for w in wrapped {
                     lines.push(Line::from(Span::styled(
                         w,
                         Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
                     )));
                 }
            } else if line.starts_with("## ") {
                 let content = line.trim_start_matches("## ").trim();
                 let wrapped = wrap_text(content, width);
                 for w in wrapped {
                     lines.push(Line::from(Span::styled(
                         w,
                         Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
                     )));
                 }
            } else if line.starts_with("### ") {
                 let content = line.trim_start_matches("### ").trim();
                 let wrapped = wrap_text(content, width);
                 for w in wrapped {
                     lines.push(Line::from(Span::styled(
                         w,
                         Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                     )));
                 }
            } else if line.starts_with("- ") {
                 let content = line.trim_start_matches("- ").trim();
                 let wrapped = wrap_text(content, width.saturating_sub(2));
                 for w in wrapped {
                     lines.push(Line::from(Span::raw(format!("• {}", w))));
                 }
            } else {
                let wrapped = wrap_text(line, width);
                for w in wrapped {
                    // Check for inline code `...` (very basic)
                    lines.push(parse_inline(w));
                }
            }
        }
    }
    lines
}

pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if text.is_empty() { return lines; }
    
    // Naive word wrapping
    let mut current_line = String::new();
    for word in text.split_whitespace() {
        if current_line.len() + word.len() + 1 > width {
            lines.push(current_line);
            current_line = word.to_string();
        } else {
            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(word);
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines
}

fn wrap_code(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(width) {
        lines.push(chunk.iter().collect());
    }
    if lines.is_empty() { lines.push("".to_string()); }
    lines
}

fn parse_inline(text: String) -> Line<'static> {
    // Basic detection of `code`
    if text.contains('`') {
        let mut spans = Vec::new();
        
        // Let's do a manual scan
        let mut current_segment = String::new();
        let mut in_code = false;
        
        // If it starts with backtick, the loop below will catch the *second* backtick closing it,
        // or we need to handle the first char carefully.
        // Actually, if I just iterate chars, I can toggle.
        
        for c in text.chars() {
            if c == '`' {
                if !current_segment.is_empty() {
                    spans.push(if in_code {
                        Span::styled(current_segment.clone(), Style::default().fg(Color::Cyan))
                    } else {
                        Span::raw(current_segment.clone())
                    });
                    current_segment.clear();
                }
                in_code = !in_code;
            } else {
                current_segment.push(c);
            }
        }
        if !current_segment.is_empty() {
             spans.push(if in_code {
                Span::styled(current_segment, Style::default().fg(Color::Cyan))
            } else {
                Span::raw(current_segment)
            });
        }
        
        Line::from(spans)
    } else {
        Line::from(Span::raw(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_text() {
        let text = "hello world this is a test";
        let wrapped = wrap_text(text, 10);
        assert_eq!(wrapped[0], "hello");
        assert_eq!(wrapped[1], "world this");
    }

    #[test]
    fn test_parse_inline() {
        let text = "this is `code` inline";
        let line = parse_inline(text.to_string());
        // Verify spans exist
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[1].content, "code");
        assert_eq!(line.spans[1].style.fg, Some(Color::Cyan));
    }
}
