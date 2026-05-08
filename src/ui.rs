use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    api::ModelInfo,
    app::{App, StatusKind, ViewMode},
    commands::parse_slash_command,
    custom_terminal::Frame,
};

pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Display width of the prompt prefix (" › " or " … "), which is also the
/// indent used on wrapped continuation rows so input text aligns vertically.
pub const PROMPT_PREFIX_WIDTH: u16 = 3;
/// Status row + 2 input borders. Added to wrapped input row count to get
/// the total viewport height.
pub const INPUT_CHROME_ROWS: u16 = 3;

mod palette {
    use ratatui::style::Color;
    pub const ACCENT: Color = Color::Rgb(244, 162, 89);
    pub const ACCENT_DIM: Color = Color::Rgb(155, 105, 60);
    pub const USER: Color = Color::Rgb(122, 192, 252);
    pub const BOT: Color = Color::Rgb(167, 224, 175);
    pub const TEXT: Color = Color::Rgb(228, 226, 219);
    pub const MUTED: Color = Color::Rgb(135, 132, 145);
    pub const FAINT: Color = Color::Rgb(78, 76, 92);
    pub const DANGER: Color = Color::Rgb(238, 102, 102);
    pub const SELECTED_BG: Color = Color::Rgb(54, 42, 36);
}

pub fn render(app: &App, f: &mut Frame) {
    let area = f.area();
    if area.height < 4 || area.width < 12 {
        return;
    }

    if matches!(app.mode, ViewMode::ModelPicker) {
        render_model_picker(app, f, area);
        return;
    }

    let text_width = input_text_width(area.width);
    let input_rows = input_visual_rows(&app.input, text_width);
    let input_height = input_rows.saturating_add(2);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(input_height), Constraint::Length(1)])
        .split(area);

    render_input(app, f, chunks[0]);
    render_status(app, f, chunks[1]);
}

/// Render lines for a user message about to be pushed to scrollback.
pub fn render_user_message(content: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "▎ ",
            Style::default()
                .fg(palette::USER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "YOU",
            Style::default()
                .fg(palette::USER)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    for content_line in content.split('\n') {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                content_line.to_string(),
                Style::default().fg(palette::TEXT),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines
}

/// Render lines for one chunk of streamed assistant output. `chunk` ends with
/// `\n` for committed chunks, or has no trailing `\n` for the final flush.
/// `with_prefix` prepends the BOT label as the first line of the assistant turn.
pub fn render_assistant_chunk(chunk: &str, with_prefix: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if with_prefix {
        lines.push(Line::from(vec![
            Span::styled(
                "▎ ",
                Style::default()
                    .fg(palette::BOT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "BOT",
                Style::default()
                    .fg(palette::BOT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    let body = chunk.strip_suffix('\n').unwrap_or(chunk);
    for content_line in body.split('\n') {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                content_line.to_string(),
                Style::default().fg(palette::TEXT),
            ),
        ]));
    }
    lines
}

/// Render the assistant-turn closing blank line when streaming has ended.
pub fn render_assistant_trailer() -> Vec<Line<'static>> {
    vec![Line::from("")]
}

/// Render an error message as a scrollback entry.
pub fn render_error(message: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            format!("  ⚠ {message}"),
            Style::default()
                .fg(palette::DANGER)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ]
}

/// Render a single status/info line for scrollback.
pub fn render_session_banner(text: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled(
                "  ✦ ",
                Style::default()
                    .fg(palette::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text.to_string(), Style::default().fg(palette::MUTED)),
        ]),
        Line::from(""),
    ]
}

fn render_input(app: &App, f: &mut Frame, area: Rect) {
    let prompt_glyph = if app.streaming { "…" } else { "›" };
    let prompt_color = if app.streaming {
        palette::MUTED
    } else {
        palette::ACCENT
    };
    let valid_command = is_valid_pending_command(app);
    let input_style = if valid_command {
        Style::default()
            .fg(palette::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette::TEXT)
    };
    let prefix = format!(" {prompt_glyph} ");
    let border_color = if app.streaming {
        palette::FAINT
    } else if valid_command {
        palette::ACCENT
    } else {
        palette::ACCENT_DIM
    };

    let text_width = input_text_width(area.width);
    let layout = wrap_input(&app.input, app.cursor, text_width);
    let prompt_style = Style::default()
        .fg(prompt_color)
        .add_modifier(Modifier::BOLD);
    let indent: String = " ".repeat(PROMPT_PREFIX_WIDTH as usize);
    let lines: Vec<Line> = layout
        .rows
        .iter()
        .enumerate()
        .map(|(row_idx, row_text)| {
            let leading = if row_idx == 0 {
                Span::styled(prefix.clone(), prompt_style)
            } else {
                Span::raw(indent.clone())
            };
            Line::from(vec![leading, Span::styled(row_text.clone(), input_style)])
        })
        .collect();

    let input = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color)),
    );
    f.render_widget(input, area);

    if !app.streaming && matches!(app.mode, ViewMode::Chat) {
        let cursor_x = area
            .x
            .saturating_add(1)
            .saturating_add(PROMPT_PREFIX_WIDTH)
            .saturating_add(layout.cursor_col);
        let cursor_y = area.y.saturating_add(1).saturating_add(layout.cursor_row);
        f.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }
}

/// Width available for the typed input text on a single visual row, after
/// accounting for the 1-col borders and the prompt prefix indent.
pub fn input_text_width(area_width: u16) -> u16 {
    area_width
        .saturating_sub(2)
        .saturating_sub(PROMPT_PREFIX_WIDTH)
        .max(1)
}

/// Number of visual rows the input text needs at the given inner text width.
/// Always at least 1 so the input box has a content row even when empty.
pub fn input_visual_rows(input: &str, text_width: u16) -> u16 {
    let rows = wrap_input(input, 0, text_width).rows.len();
    (rows.max(1)).min(u16::MAX as usize) as u16
}

struct WrapLayout {
    rows: Vec<String>,
    cursor_row: u16,
    cursor_col: u16,
}

fn wrap_input(input: &str, cursor: usize, text_width: u16) -> WrapLayout {
    let tw = text_width.max(1) as usize;
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut col: usize = 0;
    let mut cursor_pos: Option<(u16, u16)> = None;

    for (i, c) in input.char_indices() {
        if i == cursor && cursor_pos.is_none() {
            if col >= tw {
                rows.push(std::mem::take(&mut current));
                col = 0;
            }
            cursor_pos = Some((rows.len() as u16, col as u16));
        }
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if w > 0 && col + w > tw {
            rows.push(std::mem::take(&mut current));
            col = 0;
        }
        current.push(c);
        col += w;
    }

    if cursor_pos.is_none() {
        if col >= tw {
            rows.push(std::mem::take(&mut current));
            cursor_pos = Some((rows.len() as u16, 0));
        } else {
            cursor_pos = Some((rows.len() as u16, col as u16));
        }
    }

    rows.push(current);
    let (cursor_row, cursor_col) = cursor_pos.unwrap();
    WrapLayout {
        rows,
        cursor_row,
        cursor_col,
    }
}

fn is_valid_pending_command(app: &App) -> bool {
    !app.streaming
        && matches!(app.mode, ViewMode::Chat)
        && app
            .input
            .strip_prefix('/')
            .is_some_and(|command| parse_slash_command(command).is_ok())
}

fn render_status(app: &App, f: &mut Frame, area: Rect) {
    let (left_text, left_style) = if app.streaming {
        let spinner = SPINNER_FRAMES[app.spinner_idx];
        match &app.status {
            Some(status) => (
                format!("{spinner}  {}", status.text),
                style_for_status(&status.kind),
            ),
            None => (
                format!("{spinner}  streaming"),
                Style::default().fg(palette::ACCENT),
            ),
        }
    } else if app.model_picker.loading {
        let spinner = SPINNER_FRAMES[app.spinner_idx];
        (
            format!("{spinner}  loading models"),
            Style::default().fg(palette::ACCENT),
        )
    } else {
        match &app.status {
            Some(status) => (status.text.clone(), style_for_status(&status.kind)),
            None => (
                format!("model: {}", app.current_model),
                Style::default().fg(palette::FAINT),
            ),
        }
    };

    let hint = match app.mode {
        ViewMode::Chat => "esc quit  ·  /model  ·  /clear",
        ViewMode::ModelPicker => "↑↓ select  ·  enter pick  ·  esc back",
    };

    let left = Span::styled(format!(" {left_text}"), left_style);
    let right = Span::styled(format!("{hint} "), Style::default().fg(palette::FAINT));
    let pad_w = (area.width as usize).saturating_sub(left.width() + right.width());
    let pad = Span::raw(" ".repeat(pad_w));
    let line = Line::from(vec![left, pad, right]);
    f.render_widget(Paragraph::new(line), area);
}

fn style_for_status(kind: &StatusKind) -> Style {
    match kind {
        StatusKind::Info => Style::default().fg(palette::MUTED),
        StatusKind::Error => Style::default()
            .fg(palette::DANGER)
            .add_modifier(Modifier::BOLD),
    }
}

fn render_model_picker(app: &App, f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            " select model ",
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    let hint = match app.mode {
        ViewMode::ModelPicker => "↑↓ select · enter pick · esc back",
        _ => "",
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    render_picker_list(app, f.buffer_mut(), chunks[0]);

    let hint_line = Line::from(Span::styled(hint, Style::default().fg(palette::FAINT)));
    Paragraph::new(hint_line).render(chunks[1], f.buffer_mut());
}

fn render_picker_list(app: &App, buf: &mut Buffer, area: Rect) {
    let list_height = area.height as usize;
    if list_height == 0 {
        return;
    }

    if app.model_picker.loading {
        Paragraph::new(Line::from(Span::styled(
            "  loading…",
            Style::default()
                .fg(palette::MUTED)
                .add_modifier(Modifier::ITALIC),
        )))
        .render(area, buf);
        return;
    }
    if let Some(error) = &app.model_picker.error {
        Paragraph::new(Line::from(Span::styled(
            format!("  failed to load models: {error}"),
            Style::default().fg(palette::DANGER),
        )))
        .wrap(Wrap { trim: false })
        .render(area, buf);
        return;
    }
    if app.models.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "  no chat models returned by the api.",
            Style::default().fg(palette::MUTED),
        )))
        .render(area, buf);
        return;
    }

    let selected = app.model_picker.selected;
    let mut start = app.model_picker.scroll.min(selected);
    if list_height > 0 && selected >= start.saturating_add(list_height) {
        start = selected.saturating_sub(list_height.saturating_sub(1));
    }

    let id_width = app
        .models
        .iter()
        .skip(start)
        .take(list_height)
        .map(|model| model.id.chars().count())
        .max()
        .unwrap_or(0);

    let mut lines: Vec<Line> = Vec::new();
    for (idx, model) in app.models.iter().enumerate().skip(start).take(list_height) {
        let is_selected = idx == selected;
        let is_current = model.id == app.current_model;
        let cursor = if is_selected { "▶" } else { " " };
        let dot = if is_current { "●" } else { "·" };
        let id_pad = id_width.saturating_sub(model.id.chars().count());
        let id_segment = format!("  {cursor} {dot}  {}{}", model.id, " ".repeat(id_pad));
        let id_style = if is_selected {
            Style::default()
                .fg(palette::ACCENT)
                .bg(palette::SELECTED_BG)
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default()
                .fg(palette::USER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette::TEXT)
        };
        let mut spans = vec![Span::styled(id_segment, id_style)];
        let meta = format_model_meta(model);
        if !meta.is_empty() {
            let meta_style = if is_selected {
                Style::default().fg(palette::TEXT).bg(palette::SELECTED_BG)
            } else {
                Style::default().fg(palette::MUTED)
            };
            spans.push(Span::styled(format!("  {meta}"), meta_style));
        }
        lines.push(Line::from(spans));
    }

    Paragraph::new(lines).render(area, buf);
}

fn format_model_meta(model: &ModelInfo) -> String {
    let mut parts = Vec::new();
    if let Some(name) = model.name.as_deref().filter(|s| !s.is_empty()) {
        parts.push(name.to_string());
    }
    if let Some(ctx) = model.context_length {
        parts.push(format!("ctx: {}", short_count(ctx)));
    }
    if let Some(max) = model.max_output_tokens {
        parts.push(format!("max-out: {}", short_count(max)));
    }
    parts.join(" · ")
}

fn short_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}
