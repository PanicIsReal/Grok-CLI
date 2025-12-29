use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, List, ListItem, ListState, Clear, BorderType},
    Frame,
};
use crate::app::{App, AppMode, total_context_tokens, TodoStatus};
use crate::markdown::{render_markdown_lines, wrap_text};

pub fn ui(f: &mut Frame, app: &mut App) {
    // Modern layout with padding and better spacing
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Top padding
            Constraint::Min(1),    // Main content
            Constraint::Length(1), // Bottom padding
        ])
        .split(f.area());

    let main_area = outer[1];

    // Main content: messages | status | input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),        // Messages area (with padding)
            Constraint::Length(2),     // Status bar (taller)
            Constraint::Length(4),     // Input area (taller)
        ])
        .split(main_area);

    let messages_area = chunks[0];
    let status_area = chunks[1];
    let input_area = chunks[2];

    // Split messages area if todos exist (with better proportions)
    let (messages_area, todos_area) = if !app.todos.is_empty() {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(50),      // Messages (more space)
                Constraint::Length(40),   // Todo panel (wider)
            ])
            .split(messages_area);
        (split[0], Some(split[1]))
    } else {
        (messages_area, None)
    };

    // === Messages Area ===
    let mut list_items = Vec::new();
    let max_width = messages_area.width.saturating_sub(2) as usize;

    // Welcome message with ASCII logo
    if app.messages.len() <= 1 {
        let logo_color = Color::White;

        // Official Grok logo ASCII art
        let grok_logo = [
"******************************************************==*************************************************************************************",
"****************************************************+ =**************************************************************************************",
"***********************************:   -***********  +***************************************************************************************",
"******************************               ****   *****************************************************************************************",
"***************************                 -**    -*****************************************************************************************",
"*************************+      .************      ************+           ****************************    **********************************",
"************************-     +************:       ***********               **************************    **********************************",
"************************     ************=         +*********    =*******    .****+=====****+.  .=*****    *****+===*************************",
"***********************     ***********+    **+    :********    -*********---+**=       -+          ***    ****     *************************",
"***********************    :**********   =*****     ********    *****=--------**   +****+    ****    =*    **+    ***************************",
"**********************+    -********   +*******     ********    *****         **   +****    ******    *    *    -****************************",
"***********************     ******  .**********     ********    =****+++++    **   +****    ******    *         *****************************",
"***********************-    =***+.+***********     +********+    ********-    **   +****    +*****    *    **    +***************************",
"************************     .***************     =**********+      =+=      ***   +*****     ++.    +*    ***     **************************",
"************************.    .*************      :*************-           =****   +******          ***    ****+    +************************",
"************************   -***-    .:.         *********************************************************************************************",
"**********************+  -***                 ***********************************************************************************************",
"*********************  =********+         ***************************************************************************************************",
"*******************- =***********************************************************************************************************************",
"******************.=*************************************************************************************************************************",
        ];

        for line in &grok_logo {
            list_items.push(ListItem::new(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(logo_color)
            ))));
        }

        list_items.push(ListItem::new(Line::from("")));
    }

    // Process messages
    let mut i = 0;
    while i < app.messages.len() {
        let msg = &app.messages[i];

        // System messages (info/help)
        if msg.role == "system" {
            if let Some(content) = &msg.content {
                if content.starts_with("You are") {
                    i += 1;
                    continue;
                }
                list_items.push(ListItem::new(Line::from("")));
                for line in wrap_text(content, max_width - 2) {
                    list_items.push(ListItem::new(Line::from(Span::styled(
                        format!(" {}", line),
                        Style::default().fg(Color::DarkGray)
                    ))));
                }
            }
            i += 1;
            continue;
        }

        // User messages
        if msg.role == "user" {
            list_items.push(ListItem::new(Line::from("")));
            if let Some(content) = &msg.content {
                let lines: Vec<&str> = content.lines().collect();
                for (idx, line) in lines.iter().enumerate() {
                    let prefix = if idx == 0 { "👤 " } else { "   " };
                    list_items.push(ListItem::new(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Blue)),
                        Span::raw(*line),
                    ])));
                }
            }
            i += 1;
            continue;
        }

        // Tool results - skip here, they're rendered under their tool calls
        if msg.role == "tool" {
            i += 1;
            continue;
        }

        // Thought + Assistant grouping
        let (thinking_content, assistant_msg, skip_extra) = if msg.role == "thought" {
            let thinking = msg.content.clone();
            if i + 1 < app.messages.len() && app.messages[i + 1].role == "assistant" {
                (thinking, Some(&app.messages[i + 1]), true)
            } else {
                (thinking, None, false)
            }
        } else if msg.role == "assistant" {
            (None, Some(msg), false)
        } else {
            (None, None, false)
        };

        if thinking_content.is_some() || assistant_msg.is_some() {
            let has_thinking = thinking_content.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
            let has_response = assistant_msg.as_ref().and_then(|m| m.content.as_ref()).map(|c| !c.is_empty()).unwrap_or(false);
            let has_tool_calls = assistant_msg.as_ref().and_then(|m| m.tool_calls.as_ref()).map(|tc: &Vec<_>| !tc.is_empty()).unwrap_or(false);

            if !has_thinking && !has_response && !has_tool_calls {
                if skip_extra { i += 2; } else { i += 1; }
                continue;
            }

            list_items.push(ListItem::new(Line::from("")));

            // Thinking indicator (minimal)
            if has_thinking {
                list_items.push(ListItem::new(Line::from(Span::styled(
                    " ◇ thinking...",
                    Style::default().fg(Color::DarkGray)
                ))));
            }

            // Tool calls with results underneath
            if let Some(a_msg) = assistant_msg {
                if let Some(tcs) = &a_msg.tool_calls {
                    for tc in tcs {
                        if !tc.function.name.is_empty() {
                            let is_expanded = app.is_tool_expanded(&tc.id);
                            let (icon, desc) = format_tool_call(&tc.function.name, &tc.function.arguments, max_width - 6);

                            // Tool call header with expand indicator
                            let expand_icon = if is_expanded { "▼" } else { "▶" };
                            list_items.push(ListItem::new(Line::from(vec![
                                Span::styled(format!(" {} ", icon), Style::default().fg(Color::Yellow)),
                                Span::styled(&tc.function.name, Style::default().fg(Color::Yellow)),
                                Span::styled(format!(" {}", desc), Style::default().fg(Color::DarkGray)),
                                Span::styled(format!(" {}", expand_icon), Style::default().fg(Color::DarkGray)),
                            ])));

                            // Find and render the corresponding tool result
                            if let Some(result_msg) = app.messages.iter().find(|m| {
                                m.role == "tool" && m.tool_call_id.as_ref() == Some(&tc.id)
                            }) {
                                if let Some(content) = &result_msg.content {
                                    let result_lines = if is_expanded {
                                        format_tool_result_full(content, max_width - 6)
                                    } else {
                                        format_tool_result(content, max_width - 6)
                                    };
                                    for line in result_lines {
                                        // Indent with vertical line connector
                                        let mut spans = vec![Span::styled("   │ ", Style::default().fg(Color::DarkGray))];
                                        spans.extend(line.spans);
                                        list_items.push(ListItem::new(Line::from(spans)));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Response content
            if let Some(a_msg) = assistant_msg {
                if let Some(content) = &a_msg.content {
                    if !content.is_empty() {
                        // Check for megamind agent messages and apply colors
                        let megamind_color = if content.starts_with("[Pragmatist]") {
                            Some(Color::Cyan)
                        } else if content.starts_with("[Innovator]") {
                            Some(Color::Green)
                        } else if content.starts_with("[Critic]") {
                            Some(Color::Yellow)
                        } else if content.starts_with("[Synthesis]") || content.starts_with("=== Synthesis ===") {
                            Some(Color::Magenta)
                        } else {
                            None
                        };

                        for line in render_markdown_lines(content, max_width - 1) {
                            let mut spans = vec![Span::styled("🤖 ", Style::default().fg(Color::Green))];
                            if let Some(color) = megamind_color {
                                // Apply megamind color to the line
                                for span in line.spans {
                                    spans.push(Span::styled(span.content, Style::default().fg(color)));
                                }
                            } else {
                                spans.extend(line.spans);
                            }
                            list_items.push(ListItem::new(Line::from(spans)));
                        }
                    }
                }
            }

            if skip_extra { i += 2; } else { i += 1; }
            continue;
        }

        i += 1;
    }

    // Pending plan confirmation
    if let Some(plan) = &app.pending_plan {
        list_items.push(ListItem::new(Line::from("")));
        list_items.push(ListItem::new(Line::from(Span::styled(
            " ─── Plan ───",
            Style::default().fg(Color::Yellow)
        ))));
        for line in plan.lines() {
            list_items.push(ListItem::new(Line::from(Span::styled(
                format!(" {}", line),
                Style::default().fg(Color::White)
            ))));
        }
        list_items.push(ListItem::new(Line::from(Span::styled(
            " [y/n/feedback]",
            Style::default().fg(Color::DarkGray)
        ))));
    }

    // Auto-scroll to bottom only if enabled
    let item_count = list_items.len();
    if item_count > 0 && app.auto_scroll {
        app.list_state.select(Some(item_count.saturating_sub(1)));
    }

    let messages_list = List::new(list_items)
        .block(Block::default()
            .title(Span::styled(" 💬 Chat ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray)))
        .highlight_style(Style::default().bg(Color::Rgb(50, 50, 70)).fg(Color::White))
        .highlight_symbol("▶ ");

    f.render_stateful_widget(messages_list, messages_area, &mut app.list_state);

    // === Todo Panel ===
    if let Some(todo_area) = todos_area {
        let todo_items: Vec<ListItem> = app.todos.iter().map(|todo| {
            let (icon, style) = match todo.status {
                TodoStatus::Pending => ("○", Style::default().fg(Color::DarkGray)),
                TodoStatus::InProgress => ("◉", Style::default().fg(Color::Yellow)),
                TodoStatus::Completed => ("✓", Style::default().fg(Color::Green)),
            };

            let text = match todo.status {
                TodoStatus::InProgress => &todo.active_form,
                _ => &todo.content,
            };

            // Truncate long text
            let max_text_len = todo_area.width.saturating_sub(6) as usize;
            let display_text = if text.len() > max_text_len && max_text_len > 3 {
                format!("{}…", &text[..max_text_len - 1])
            } else {
                text.clone()
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", icon), style),
                Span::styled(display_text, style),
            ]))
        }).collect();

        // Count stats
        let completed = app.todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
        let total = app.todos.len();

        let todo_list = List::new(todo_items)
            .block(Block::default()
                .title(Span::styled(
                    format!(" 📋 Tasks ({}/{}) ", completed, total),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                ))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray)));

        f.render_widget(todo_list, todo_area);
    }

    // === Status Bar ===
    let cwd = std::env::current_dir()
        .map(|p| {
            let path = p.to_string_lossy();
            // Shorten home directory
            if let Ok(home) = std::env::var("HOME") {
                if path.starts_with(&home) {
                    return format!("~{}", &path[home.len()..]);
                }
            }
            // Truncate long paths
            if path.len() > 30 {
                format!("...{}", &path[path.len()-27..])
            } else {
                path.to_string()
            }
        })
        .unwrap_or_else(|_| ".".to_string());

    // Use API-reported tokens if available, otherwise estimate
    let (input_tokens, output_tokens) = if app.total_input_tokens > 0 {
        (app.total_input_tokens, app.total_output_tokens)
    } else {
        (total_context_tokens(&app.api_messages), 0)
    };
    let total_tokens = input_tokens + output_tokens;
    let max_context = app.get_current_context();
    let pct = (total_tokens * 100) / max_context;
    let context_color = if pct > 80 { Color::Red } else if pct > 50 { Color::Yellow } else { Color::DarkGray };

    // Format token count (e.g., "12.5k" or "1.2M")
    let format_tokens = |t: usize| -> String {
        if t >= 1_000_000 {
            format!("{:.1}M", t as f64 / 1_000_000.0)
        } else if t >= 1_000 {
            format!("{:.1}k", t as f64 / 1_000.0)
        } else {
            format!("{}", t)
        }
    };
    let token_str = format!("{}↑ {}↓", format_tokens(input_tokens), format_tokens(output_tokens));

    // Format duration helper
    let format_duration = |d: std::time::Duration| -> String {
        let secs = d.as_secs_f64();
        if secs < 60.0 {
            format!("{:.1}s", secs)
        } else {
            let mins = secs / 60.0;
            format!("{:.1}m", mins)
        }
    };

    let status_spans = if app.is_loading {
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let s = spinner[app.spinner_index % spinner.len()];
        let elapsed = app.task_start
            .map(|start| format_duration(start.elapsed()))
            .unwrap_or_default();
        let mut spans = vec![
            Span::styled(format!(" {} ", cwd), Style::default().fg(Color::DarkGray)),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(format!(" {} ", app.client.get_model()), Style::default().fg(Color::Cyan)),
        ];
        if app.sandbox_enabled {
            spans.push(Span::styled("◆ ", Style::default().fg(Color::Green)));
        }
        if app.debug_mode {
            spans.push(Span::styled("◇ ", Style::default().fg(Color::Magenta)));
        }
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        // Create visual progress bar for context usage
        let bar_width = 10;
        let filled = (pct * bar_width / 100).min(bar_width);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_width - filled));
        spans.push(Span::styled(format!(" {} ", token_str), Style::default().fg(context_color)));
        spans.push(Span::styled(format!("{} ", bar), Style::default().fg(context_color)));
        spans.push(Span::styled(format!("{}%", pct), Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(format!(" {} ", s), Style::default().fg(Color::Yellow)));

        // Show thinking preview if available, otherwise status message
        if let Some(ref thinking) = app.thinking_preview {
            // Clean up thinking text: remove newlines, take last part
            let clean = thinking.replace('\n', " ").replace("  ", " ");
            let preview = if clean.len() > 60 {
                format!("{}...", &clean.chars().take(60).collect::<String>())
            } else {
                clean
            };
            spans.push(Span::styled("💭 ", Style::default().fg(Color::Magenta)));
            spans.push(Span::styled(preview, Style::default().fg(Color::Magenta).add_modifier(Modifier::ITALIC)));
        } else {
            spans.push(Span::styled(&app.status_message, Style::default().fg(Color::Yellow)));
        }

        spans.push(Span::styled(format!(" {} ", elapsed), Style::default().fg(Color::DarkGray)));
        // Show error count if any
        if !app.errors.is_empty() {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!(" {} Error(s): F12 to view ", app.errors.len()),
                Style::default().fg(Color::Red)
            ));
        }
        spans
    } else {
        let mut spans = vec![
            Span::styled(format!(" {} ", cwd), Style::default().fg(Color::DarkGray)),
            Span::styled("│", Style::default().fg(Color::DarkGray)),
            Span::styled(format!(" {} ", app.client.get_model()), Style::default().fg(Color::Cyan)),
        ];
        if app.sandbox_enabled {
            spans.push(Span::styled("◆ ", Style::default().fg(Color::Green)));
        }
        if app.debug_mode {
            spans.push(Span::styled("◇ ", Style::default().fg(Color::Magenta)));
        }
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        // Create visual progress bar for context usage
        let bar_width = 10;
        let filled = (pct * bar_width / 100).min(bar_width);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_width - filled));
        spans.push(Span::styled(format!(" {} ", token_str), Style::default().fg(context_color)));
        spans.push(Span::styled(format!("{} ", bar), Style::default().fg(context_color)));
        spans.push(Span::styled(format!("{}%", pct), Style::default().fg(Color::DarkGray)));
        // Show last task duration
        if let Some(duration) = app.last_task_duration {
            spans.push(Span::styled(format!(" {} ", format_duration(duration)), Style::default().fg(Color::DarkGray)));
        }
        // Show error count if any
        if !app.errors.is_empty() {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                format!(" {} Error(s): F12 to view ", app.errors.len()),
                Style::default().fg(Color::Red)
            ));
        }
        spans
    };

    f.render_widget(
        Paragraph::new(Line::from(status_spans))
            .block(Block::default()
                .borders(Borders::TOP)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))),
        status_area
    );

    // === Input Area ===
    f.render_widget(&app.input, input_area);

    // === Autocomplete Popup ===
    if app.autocomplete_active && !app.autocomplete_filtered.is_empty() {
        // Calculate desired height: items + header + borders
        let desired_height = (app.autocomplete_filtered.len() as u16 + 4).min(20);
        // Constrain to available space above input (leave 1 row gap minimum)
        let available_height = input_area.y.saturating_sub(1);
        let popup_height = desired_height.min(available_height).max(5); // At least 5 rows

        // Calculate width based on longest option (min 25, max 50)
        let max_option_len = app.autocomplete_filtered.iter()
            .map(|s| s.len())
            .max()
            .unwrap_or(20);
        let popup_width = (max_option_len as u16 + 4).clamp(25, 50);

        let area = Rect {
            x: input_area.x + 1,
            y: input_area.y.saturating_sub(popup_height),
            width: popup_width,
            height: popup_height,
        };

        f.render_widget(Clear, area);

        // Group autocomplete items by category
        let mut command_items = Vec::new();
        let mut model_items = Vec::new();
        let mut role_items = Vec::new();

        for item in &app.autocomplete_filtered {
            if item.starts_with('/') && !item.contains(' ') {
                command_items.push(item.clone());
            } else if item.starts_with("/model ") {
                model_items.push(item.clone());
            } else if item.starts_with('@') {
                role_items.push(item.clone());
            } else {
                command_items.push(item.clone()); // fallback
            }
        }

        let mut items = Vec::new();
        let mut selected_item_index: Option<usize> = None;
        let mut current_filtered_index = 0;

        if !command_items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                "─ Commands ─",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            ))));
            for cmd in &command_items {
                let is_selected = current_filtered_index == app.autocomplete_index;
                if is_selected {
                    selected_item_index = Some(items.len());
                }
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::raw(cmd.clone()),
                ])));
                current_filtered_index += 1;
            }
        }

        if !model_items.is_empty() {
            if !items.is_empty() {
                items.push(ListItem::new(""));
            }
            items.push(ListItem::new(Line::from(Span::styled(
                "─ Models ─",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            ))));
            for model in &model_items {
                let is_selected = current_filtered_index == app.autocomplete_index;
                if is_selected {
                    selected_item_index = Some(items.len());
                }
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::raw(model.clone()),
                ])));
                current_filtered_index += 1;
            }
        }

        if !role_items.is_empty() {
            if !items.is_empty() {
                items.push(ListItem::new(""));
            }
            items.push(ListItem::new(Line::from(Span::styled(
                "─ Roles ─",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            ))));
            for role in &role_items {
                let is_selected = current_filtered_index == app.autocomplete_index;
                if is_selected {
                    selected_item_index = Some(items.len());
                }
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::raw(role.clone()),
                ])));
                current_filtered_index += 1;
            }
        }

        let items_len = items.len();
        let list = List::new(items)
            .block(Block::default()
                .title(Span::styled(" Autocomplete ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan)))
            .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));

        let mut state = ListState::default();
        state.select(selected_item_index);

        // Calculate scroll offset to ensure selected item is visible
        let visible_rows = popup_height.saturating_sub(2) as usize; // Content rows (minus borders)
        if let Some(selected) = selected_item_index {
            if selected >= visible_rows {
                // Need to scroll down to show selected item
                *state.offset_mut() = selected.saturating_sub(visible_rows) + 1;
            }
            // Also ensure we don't scroll past the end
            let max_offset = items_len.saturating_sub(visible_rows);
            if *state.offset_mut() > max_offset {
                *state.offset_mut() = max_offset;
            }
        }

        f.render_stateful_widget(list, area, &mut state);
    }

    // === Planning Modal ===
    if let AppMode::Planning(state) = &mut app.mode {
        let area = centered_rect(60, 50, f.area());
        f.render_widget(Clear, area);

        let block = Block::default()
            .title(Span::styled(
                format!(" {} ", state.question),
                Style::default().add_modifier(Modifier::BOLD)
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));

        f.render_widget(block.clone(), area);

        let items: Vec<ListItem> = state.options.iter().enumerate().map(|(i, opt)| {
            let marker = if state.selected[i] { "☑" } else { "☐" };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", marker), Style::default().fg(if state.selected[i] { Color::Green } else { Color::DarkGray })),
                Span::raw(opt),
            ]))
        }).collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow))
            .highlight_symbol("▸ ");

        f.render_stateful_widget(list, block.inner(area), &mut state.list_state);
    }

    // === Error View Modal ===
    if let AppMode::ErrorView = &app.mode {
        let area = centered_rect(80, 70, f.area());
        f.render_widget(Clear, area);

        let block = Block::default()
            .title(Span::styled(
                format!(" Errors ({}) - Esc/F12 to close, Ctrl+C to clear ", app.errors.len()),
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Red)
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Red));

        f.render_widget(block.clone(), area);

        let inner = block.inner(area);
        let max_width = inner.width.saturating_sub(2) as usize;

        let items: Vec<ListItem> = app.errors.iter().enumerate().map(|(i, err)| {
            let lines: Vec<Line> = err.lines().enumerate().map(|(line_idx, line)| {
                if line_idx == 0 {
                    Line::from(vec![
                        Span::styled(format!("{:3}. ", i + 1), Style::default().fg(Color::Yellow)),
                        Span::styled(truncate(line, max_width.saturating_sub(5)), Style::default().fg(Color::White)),
                    ])
                } else {
                    Line::from(Span::styled(
                        format!("     {}", truncate(line, max_width.saturating_sub(5))),
                        Style::default().fg(Color::DarkGray)
                    ))
                }
            }).collect();
            ListItem::new(lines)
        }).collect();

        if items.is_empty() {
            let empty_msg = Paragraph::new(Span::styled(
                "No errors",
                Style::default().fg(Color::DarkGray)
            ));
            f.render_widget(empty_msg, inner);
        } else {
            let list = List::new(items)
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(list, inner);
        }
    }

    // === Settings Modal ===
    if let AppMode::Settings(ref state) = &app.mode {
        let area = centered_rect(60, 40, f.area());
        f.render_widget(Clear, area);

        let block = Block::default()
            .title(Span::styled(
                " ⚙️ Settings ",
                Style::default().add_modifier(Modifier::BOLD)
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));

        f.render_widget(block.clone(), area);

        let inner = block.inner(area);
        let max_width = inner.width.saturating_sub(4) as usize;

        // Create list items for each setting
        let items: Vec<ListItem> = state.settings_list.iter().enumerate().map(|(i, setting)| {
            // Get the current value for this setting
            let is_enabled = match setting.key {
                "rate_limiter_enabled" => app.config.settings.rate_limiter_enabled,
                _ => false,
            };

            let toggle_marker = if is_enabled { "◉" } else { "○" };
            let toggle_color = if is_enabled { Color::Green } else { Color::DarkGray };
            let is_selected = i == state.selected_index;

            // Create the main line with toggle
            let line1 = Line::from(vec![
                Span::styled(
                    format!(" {} ", toggle_marker),
                    Style::default().fg(toggle_color)
                ),
                Span::styled(
                    setting.name,
                    Style::default()
                        .fg(if is_selected { Color::Yellow } else { Color::White })
                        .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() })
                ),
                Span::styled(
                    format!(" [{}]", if is_enabled { "ON" } else { "OFF" }),
                    Style::default().fg(toggle_color)
                ),
            ]);

            // Description line (only for selected item)
            if is_selected {
                let desc = truncate(setting.description, max_width);
                let line2 = Line::from(vec![
                    Span::raw("   "),
                    Span::styled(desc, Style::default().fg(Color::DarkGray)),
                ]);
                ListItem::new(vec![line1, line2, Line::from("")])
            } else {
                ListItem::new(vec![line1])
            }
        }).collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE))
            .highlight_style(Style::default());

        let mut list_state = ListState::default();
        list_state.select(Some(state.selected_index));
        f.render_stateful_widget(list, inner, &mut list_state);

        // Instructions at bottom
        let footer_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(2),
            width: inner.width,
            height: 2,
        };
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow)),
            Span::raw("navigate  "),
            Span::styled(" Space ", Style::default().fg(Color::Yellow)),
            Span::raw("toggle  "),
            Span::styled(" Esc ", Style::default().fg(Color::Yellow)),
            Span::raw("close"),
        ]));
        f.render_widget(footer, footer_area);
    }
}

/// Format tool call for display
fn format_tool_call(name: &str, args: &str, max_len: usize) -> (&'static str, String) {
    let parsed: serde_json::Value = serde_json::from_str(args).unwrap_or_default();

    match name {
        "Read" | "read_file" => {
            let path = parsed.get("file_path").or(parsed.get("path"))
                .and_then(|v| v.as_str()).unwrap_or("?");
            ("📖", truncate(path, max_len))
        }
        "Edit" | "edit_file" => {
            let path = parsed.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
            ("✏️", truncate(path, max_len))
        }
        "Write" | "write_file" => {
            let path = parsed.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
            ("📝", truncate(path, max_len))
        }
        "Glob" | "glob" => {
            let pattern = parsed.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
            ("🔍", truncate(pattern, max_len))
        }
        "Grep" | "grep" | "search" => {
            let pattern = parsed.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            ("🔎", truncate(&format!("/{}/", pattern), max_len))
        }
        "Bash" | "bash" | "shell" => {
            let cmd = parsed.get("command").and_then(|v| v.as_str()).unwrap_or("?");
            ("💻", truncate(cmd, max_len))
        }
        "List" | "list_dir" => {
            let path = parsed.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            ("📂", truncate(path, max_len))
        }
        "WebSearch" | "web_search" => {
            let query = parsed.get("query").and_then(|v| v.as_str()).unwrap_or("?");
            ("🌐", truncate(query, max_len))
        }
        _ => ("⚙️", truncate(args, max_len.min(30)))
    }
}

/// Format tool result for display with diff coloring
fn format_tool_result(content: &str, max_width: usize) -> Vec<Line<'static>> {
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return vec![Line::from(Span::styled("✓", Style::default().fg(Color::Green)))];
    }

    // Check if this looks like a diff output (has @@ header or +/- lines)
    let is_diff = lines.iter().any(|l| l.starts_with("@@") || l.starts_with("-  ") || l.starts_with("+  "));

    if is_diff {
        // Render diff with colors - show tail if too long
        let visible_lines = 8;
        let lines_to_show: Vec<&str> = if lines.len() > visible_lines {
            lines.iter().skip(lines.len() - visible_lines).copied().collect()
        } else {
            lines.clone()
        };

        let mut result = Vec::new();

        // Add (+N lines) indicator if truncated
        if lines.len() > visible_lines {
            result.push(Line::from(Span::styled(
                format!("(+{} lines)", lines.len() - visible_lines),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
            )));
        }

        for line in &lines_to_show {
            let styled_line = if line.starts_with("@@") {
                // Diff header - cyan
                Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Cyan)))
            } else if line.starts_with("-  ") {
                // Removed line - red
                Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Red)))
            } else if line.starts_with("+  ") {
                // Added line - green
                Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Green)))
            } else if line.starts_with("✓") {
                // Success marker - green
                Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Green)))
            } else if line.is_empty() {
                Line::from("")
            } else {
                // File path or other content - white
                Line::from(Span::styled(line.to_string(), Style::default().fg(Color::White)))
            };
            result.push(styled_line);
        }
        return result;
    }

    // Check for error output - show in red with compact format
    if content.starts_with("Error:") || content.starts_with("error:") {
        let first_line = lines.first().unwrap_or(&"Error");
        let error_text = truncate(first_line, max_width);
        return vec![Line::from(Span::styled(
            format!("✗ {}", error_text),
            Style::default().fg(Color::Red)
        ))];
    }

    // Non-diff content
    let non_empty: Vec<&str> = lines.iter().filter(|l| !l.trim().is_empty()).copied().collect();

    if non_empty.is_empty() {
        return vec![Line::from(Span::styled("✓", Style::default().fg(Color::Green)))];
    }

    // Single short line
    if non_empty.len() == 1 && non_empty[0].len() <= max_width {
        return vec![Line::from(Span::styled(
            truncate(non_empty[0], max_width),
            Style::default().fg(Color::DarkGray)
        ))];
    }

    // Multiple lines - show TAIL (last 6 lines) with (+N lines) indicator
    let visible_lines = 6;
    if non_empty.len() > visible_lines {
        let hidden_count = non_empty.len() - visible_lines;
        let mut result = vec![Line::from(Span::styled(
            format!("(+{} lines)", hidden_count),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
        ))];
        // Show the tail (last N lines)
        for line in non_empty.iter().skip(hidden_count) {
            result.push(Line::from(Span::styled(
                truncate(line, max_width),
                Style::default().fg(Color::DarkGray)
            )));
        }
        return result;
    }

    non_empty.iter().map(|l| Line::from(Span::styled(
        truncate(l, max_width),
        Style::default().fg(Color::DarkGray)
    ))).collect()
}

/// Format tool result for display - FULL version (expanded)
fn format_tool_result_full(content: &str, max_width: usize) -> Vec<Line<'static>> {
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return vec![Line::from(Span::styled("✓", Style::default().fg(Color::Green)))];
    }

    // Check if this is an error - show all in red
    let is_error = content.starts_with("Error:") || content.starts_with("error:");

    let mut result = Vec::new();
    for line in &lines {
        let styled_line = if is_error {
            Line::from(Span::styled(
                truncate(line, max_width),
                Style::default().fg(Color::Red)
            ))
        } else if line.starts_with("@@") {
            Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Cyan)))
        } else if line.starts_with("-  ") {
            Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Red)))
        } else if line.starts_with("+  ") {
            Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Green)))
        } else if line.starts_with("✓") {
            Line::from(Span::styled(line.to_string(), Style::default().fg(Color::Green)))
        } else if line.is_empty() {
            Line::from("")
        } else {
            Line::from(Span::styled(
                truncate(line, max_width),
                Style::default().fg(Color::DarkGray)
            ))
        };
        result.push(styled_line);
    }
    result
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("{}…", &s[..max_len - 1])
    } else {
        s[..max_len].to_string()
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
