use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, StatefulWidgetRef, Widget, Wrap},
};

use crate::{
    api::ModelInfo,
    app::{AUTH_PROVIDER_CHOICES, App, AuthStep, StatusKind, ViewMode},
    commands::{SlashCommand, parse_slash_command},
    custom_terminal::Frame,
};

pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Display width of the prompt prefix (" › " or " … "), which is also the
/// indent used on wrapped continuation rows so input text aligns vertically.
pub const PROMPT_PREFIX_WIDTH: u16 = 3;
/// Status row + 2 input borders. Added to wrapped input row count to get
/// the total viewport height.
pub const INPUT_CHROME_ROWS: u16 = 3;

pub(crate) mod palette {
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

pub fn render(app: &mut App, f: &mut Frame) {
    let area = f.area();
    if area.height < 4 || area.width < 12 {
        return;
    }

    if matches!(app.mode, ViewMode::ModelPicker) {
        render_model_picker(app, f, area);
        return;
    }

    if matches!(app.mode, ViewMode::AuthWizard) {
        render_auth_wizard(app, f, area);
        return;
    }

    let text_width = input_text_width(area.width);
    let input_rows = app.input.desired_height(text_width);
    let input_height = input_rows.saturating_add(2);

    let popup_height = app.command_popup.as_ref().map_or(0, |p| p.desired_height());
    let mut constraints: Vec<Constraint> = Vec::new();
    if popup_height > 0 {
        constraints.push(Constraint::Length(popup_height));
    }
    constraints.push(Constraint::Length(input_height));
    constraints.push(Constraint::Length(1));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    if let Some(popup) = app.command_popup.as_ref() {
        popup.render(chunks[idx], f.buffer_mut());
        idx += 1;
    }
    render_input(app, f, chunks[idx]);
    idx += 1;
    render_status(app, f, chunks[idx]);
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

fn render_input(app: &mut App, f: &mut Frame, area: Rect) {
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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    let prompt_style = Style::default()
        .fg(prompt_color)
        .add_modifier(Modifier::BOLD);
    let indent: String = " ".repeat(PROMPT_PREFIX_WIDTH as usize);
    let text_width = inner.width.saturating_sub(PROMPT_PREFIX_WIDTH).max(1);
    let row_count = app.input.desired_height(text_width);
    for r in 0..row_count.min(inner.height) {
        let (leading, style) = if r == 0 {
            (prefix.as_str(), prompt_style)
        } else {
            (indent.as_str(), Style::default())
        };
        f.buffer_mut()
            .set_string(inner.x, inner.y + r, leading, style);
    }

    let text_rect = Rect {
        x: inner.x + PROMPT_PREFIX_WIDTH,
        y: inner.y,
        width: inner.width.saturating_sub(PROMPT_PREFIX_WIDTH),
        height: inner.height,
    };

    f.buffer_mut().set_style(text_rect, input_style);
    (&app.input).render_ref(text_rect, f.buffer_mut(), &mut app.input_state);

    if !app.streaming && matches!(app.mode, ViewMode::Chat) {
        if let Some((cx, cy)) = app.input.cursor_pos_with_state(text_rect, app.input_state) {
            f.set_cursor_position(Position { x: cx, y: cy });
        }
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

fn is_valid_pending_command(app: &App) -> bool {
    !app.streaming
        && matches!(app.mode, ViewMode::Chat)
        && app
            .input
            .text()
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

    let chat_hint: String = {
        let cmds: Vec<String> = SlashCommand::all()
            .iter()
            .map(|c| format!("/{}", c.command()))
            .collect();
        format!("esc quit  ·  {}", cmds.join("  ·  "))
    };
    let hint: &str = match app.mode {
        ViewMode::Chat => &chat_hint,
        ViewMode::ModelPicker => "↑↓ select  ·  enter pick  ·  esc back",
        ViewMode::AuthWizard => "",
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
    let title = if app.model_picker.filter.is_empty() {
        " select model ".to_string()
    } else {
        format!(" select model · filter: {} ", app.model_picker.filter)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            title,
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    let hint = "type to filter · ↑↓ select · enter pick · esc back";
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

    let matches = app.model_picker.matches(&app.models);
    if matches.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            format!("  no models match \"{}\"", app.model_picker.filter),
            Style::default()
                .fg(palette::MUTED)
                .add_modifier(Modifier::ITALIC),
        )))
        .render(area, buf);
        return;
    }

    let model_rows = list_height.saturating_sub(1);
    let selected = app.model_picker.selected;
    let mut start = app.model_picker.scroll.min(selected);
    if model_rows > 0 && selected >= start.saturating_add(model_rows) {
        start = selected.saturating_sub(model_rows.saturating_sub(1));
    }

    let id_width = matches
        .iter()
        .skip(start)
        .take(model_rows)
        .map(|&i| app.models[i].id.chars().count())
        .max()
        .unwrap_or(0)
        .max("model".len());

    let mut lines: Vec<Line> = vec![build_header_line(id_width)];
    for (row, &model_idx) in matches.iter().enumerate().skip(start).take(model_rows) {
        let model = &app.models[model_idx];
        let is_selected = row == selected;
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
        let meta = format_model_meta_cols(model);
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

fn build_header_line(id_width: usize) -> Line<'static> {
    let style = Style::default().fg(palette::FAINT).add_modifier(Modifier::BOLD);
    let text = format!(
        "       {:<id_width$}  {:<8}{:<8}{:<6}{}",
        "model", "in", "out", "ctx", "max",
        id_width = id_width
    );
    Line::from(Span::styled(text, style))
}

fn format_model_meta_cols(model: &ModelInfo) -> String {
    let in_val  = model.input_price_per_mtok.map(|v| format!("${}", fmt_price(v))).unwrap_or_default();
    let out_val = model.output_price_per_mtok.map(|v| format!("${}", fmt_price(v))).unwrap_or_default();
    let ctx_val = model.context_length.map(short_count).unwrap_or_default();
    let max_val = model.max_output_tokens.map(short_count).unwrap_or_default();
    if in_val.is_empty() && out_val.is_empty() && ctx_val.is_empty() && max_val.is_empty() {
        return String::new();
    }
    format!("{:<8}{:<8}{:<6}{}", in_val, out_val, ctx_val, max_val)
}

fn fmt_price(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let s = format!("{:.4}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
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

pub fn mask_api_key(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    if n <= 4 {
        return "•".repeat(n);
    }
    let mut out = "•".repeat(n - 4);
    out.extend(&chars[n - 4..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_api_key_empty() {
        assert_eq!(mask_api_key(""), "");
    }

    #[test]
    fn mask_api_key_short_fully_masked() {
        assert_eq!(mask_api_key("abcd"), "••••");
        assert_eq!(mask_api_key("abc"), "•••");
    }

    #[test]
    fn mask_api_key_longer_shows_last_four() {
        assert_eq!(mask_api_key("sk-1234"), "•••1234");
    }

    #[test]
    fn mask_api_key_long_key() {
        assert_eq!(mask_api_key("sk-or-abcdefghABCD"), "••••••••••••••ABCD");
    }
}

fn render_auth_wizard(app: &App, f: &mut Frame, area: Rect) {
    let choice = AUTH_PROVIDER_CHOICES[app.auth_wizard.provider_idx];
    let title = match app.auth_wizard.step {
        AuthStep::SelectProvider => " setup auth ".to_string(),
        AuthStep::EnterOrigin    => format!(" setup auth  >  {} ", choice.label()),
        AuthStep::EnterApiKey    => format!(" setup auth  >  {} ", choice.label()),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            title,
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    let hint = match app.auth_wizard.step {
        AuthStep::SelectProvider => "↑↓ select  ·  enter continue  ·  esc cancel",
        AuthStep::EnterOrigin    => "enter continue  ·  esc cancel",
        AuthStep::EnterApiKey    => "enter save  ·  esc cancel",
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    match app.auth_wizard.step {
        AuthStep::SelectProvider => render_auth_provider_list(app, f.buffer_mut(), chunks[0]),
        AuthStep::EnterOrigin    => render_auth_text_field(
            "Origin (e.g. https://api.together.ai)",
            &app.auth_wizard.origin,
            app.auth_wizard.error.as_deref(),
            false,
            f.buffer_mut(),
            chunks[0],
        ),
        AuthStep::EnterApiKey    => render_auth_text_field(
            "API key",
            &app.auth_wizard.api_key,
            app.auth_wizard.error.as_deref(),
            true,
            f.buffer_mut(),
            chunks[0],
        ),
    }

    let hint_line = Line::from(Span::styled(hint, Style::default().fg(palette::FAINT)));
    Paragraph::new(hint_line).render(chunks[1], f.buffer_mut());
}

fn render_auth_provider_list(app: &App, buf: &mut Buffer, area: Rect) {
    let selected = app.auth_wizard.provider_idx;
    let label_width = AUTH_PROVIDER_CHOICES
        .iter()
        .map(|c| c.label().chars().count())
        .max()
        .unwrap_or(0);

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (idx, choice) in AUTH_PROVIDER_CHOICES.iter().enumerate() {
        let is_selected = idx == selected;
        let cursor = if is_selected { "▶" } else { " " };
        let pad = label_width.saturating_sub(choice.label().chars().count());
        let label_segment = format!("  {cursor}  {}{}", choice.label(), " ".repeat(pad));
        let (label_style, desc_style) = if is_selected {
            (
                Style::default().fg(palette::ACCENT).bg(palette::SELECTED_BG).add_modifier(Modifier::BOLD),
                Style::default().fg(palette::TEXT).bg(palette::SELECTED_BG),
            )
        } else {
            (
                Style::default().fg(palette::TEXT),
                Style::default().fg(palette::MUTED),
            )
        };
        lines.push(Line::from(vec![
            Span::styled(label_segment, label_style),
            Span::styled(format!("  {}", choice.description()), desc_style),
        ]));
    }
    Paragraph::new(lines).render(area, buf);
}

fn render_auth_text_field(
    label: &str,
    value: &str,
    error: Option<&str>,
    mask: bool,
    buf: &mut Buffer,
    area: Rect,
) {
    let display = if mask { mask_api_key(value) } else { value.to_string() };

    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {label}:"),
            Style::default().fg(palette::MUTED),
        )),
    ];

    let field_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::ACCENT_DIM));
    let field_area = Rect {
        x: area.x + 2,
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: 3,
    };

    if let Some(err) = error {
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ⚠  {err}"),
            Style::default().fg(palette::DANGER),
        )));
    }

    Paragraph::new(lines).render(area, buf);

    // Render the input box on top of the placeholder lines
    let inner_field = field_block.inner(field_area);
    field_block.render(field_area, buf);
    Paragraph::new(Line::from(Span::styled(
        display,
        Style::default().fg(palette::TEXT),
    )))
    .render(inner_field, buf);
}
