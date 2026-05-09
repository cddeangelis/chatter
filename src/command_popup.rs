use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::commands::SlashCommand;
use crate::ui::palette;

pub struct CommandPopup {
    filter: String,
    selected: usize,
}

impl CommandPopup {
    pub fn new() -> Self {
        Self {
            filter: String::new(),
            selected: 0,
        }
    }

    pub fn on_text_change(&mut self, first_line: &str) {
        let after_slash = first_line.strip_prefix('/').unwrap_or("");
        let token = after_slash.split_whitespace().next().unwrap_or("");
        self.filter = token.to_lowercase();
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub fn filtered(&self) -> Vec<SlashCommand> {
        if self.filter.is_empty() {
            return SlashCommand::all().to_vec();
        }
        SlashCommand::all()
            .iter()
            .filter(|cmd| cmd.command().starts_with(self.filter.as_str()))
            .copied()
            .collect()
    }

    pub fn selected(&self) -> Option<SlashCommand> {
        self.filtered().get(self.selected).copied()
    }

    pub fn move_up(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1) % len;
    }

    pub fn desired_height(&self) -> u16 {
        let rows = self.filtered().len().max(1); // at least 1 for "no matches"
        rows as u16 + 2 // borders
    }
}

impl Widget for &CommandPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette::ACCENT_DIM));
        let inner = block.inner(area);
        block.render(area, buf);

        let items = self.filtered();
        if items.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                "  no matches",
                Style::default()
                    .fg(palette::MUTED)
                    .add_modifier(Modifier::ITALIC),
            )))
            .render(inner, buf);
            return;
        }

        let name_width = items
            .iter()
            .map(|cmd| cmd.command().chars().count() + 1) // +1 for '/'
            .max()
            .unwrap_or(0);

        let mut lines: Vec<Line> = Vec::new();
        for (idx, cmd) in items.iter().enumerate() {
            let is_selected = idx == self.selected;
            let cursor = if is_selected { "▶" } else { " " };
            let name = format!("/{}", cmd.command());
            let pad = name_width.saturating_sub(name.chars().count());
            let name_segment = format!(" {cursor} {name}{}", " ".repeat(pad));

            let (name_style, desc_style) = if is_selected {
                (
                    Style::default()
                        .fg(palette::ACCENT)
                        .bg(palette::SELECTED_BG)
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(palette::TEXT).bg(palette::SELECTED_BG),
                )
            } else {
                (
                    Style::default().fg(palette::TEXT),
                    Style::default().fg(palette::MUTED),
                )
            };

            lines.push(Line::from(vec![
                Span::styled(name_segment, name_style),
                Span::styled(
                    format!("  {}", cmd.description()),
                    desc_style,
                ),
            ]));
        }

        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_shows_all() {
        let p = CommandPopup::new();
        assert_eq!(p.filtered().len(), 4);
    }

    #[test]
    fn prefix_filter_narrows_list() {
        let mut p = CommandPopup::new();
        p.on_text_change("/cl");
        assert_eq!(p.filtered(), vec![SlashCommand::Clear]);
    }

    #[test]
    fn no_match_returns_empty() {
        let mut p = CommandPopup::new();
        p.on_text_change("/foo");
        assert!(p.filtered().is_empty());
    }

    #[test]
    fn move_down_wraps() {
        let mut p = CommandPopup::new();
        // 4 items; wrap after last
        for _ in 0..4 {
            p.move_down();
        }
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn move_up_wraps() {
        let mut p = CommandPopup::new();
        p.move_up();
        assert_eq!(p.selected, 3); // wraps to last of 4
    }
}
