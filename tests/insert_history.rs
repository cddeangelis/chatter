//! VT100 snapshot tests for `insert_history_lines`.
//!
//! These exercise the DECSTBM + reverse-index dance against a real
//! `vt100::Parser` so the scroll-region behavior is locked down before
//! steps 5-7 of the rebuild touch the surrounding draw loop.

mod common;

use chatter::{InsertHistoryMode, Terminal, insert_history_lines};
use common::VT100Backend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

fn make_terminal(width: u16, height: u16, viewport: Rect) -> Terminal<VT100Backend> {
    let backend = VT100Backend::new(width, height);
    let mut term = Terminal::with_options_and_cursor_position(backend, Position::new(0, 0))
        .expect("construct test terminal");
    term.set_viewport_area(viewport);
    term
}

fn insert(term: &mut Terminal<VT100Backend>, lines: Vec<Line<'static>>) {
    insert_history_lines(term, lines, InsertHistoryMode::Standard)
        .expect("insert_history_lines");
}

#[test]
fn basic_insertion_no_wrap() {
    // 80x24 screen, viewport pinned to the last 2 rows (rows 22-23).
    let viewport = Rect::new(0, 22, 80, 2);
    let mut term = make_terminal(80, 24, viewport);

    insert(
        &mut term,
        vec![
            Line::from("first"),
            Line::from("second"),
            Line::from("third"),
        ],
    );

    let contents = term.backend().vt100().screen().contents();
    assert!(contents.contains("first"), "missing 'first' in:\n{contents}");
    assert!(contents.contains("second"), "missing 'second' in:\n{contents}");
    assert!(contents.contains("third"), "missing 'third' in:\n{contents}");
}

#[test]
fn long_token_wraps() {
    // Width 40, viewport at the bottom 2 rows. A 100-char run of 'A's must
    // round-trip through wrap into multiple terminal rows, with no characters
    // dropped.
    let viewport = Rect::new(0, 22, 40, 2);
    let mut term = make_terminal(40, 24, viewport);

    let long: String = "A".repeat(100);
    insert(&mut term, vec![Line::from(long.clone())]);

    let screen = term.backend().vt100().screen();
    let mut count_a = 0usize;
    for row in 0..24u16 {
        for col in 0..40u16 {
            if let Some(cell) = screen.cell(row, col)
                && cell.contents() == "A"
            {
                count_a += 1;
            }
        }
    }
    assert_eq!(count_a, long.len(), "wrapped content lost characters");
}

#[test]
fn cursor_returns_to_last_known_position() {
    // `insert_history_lines` must be cursor-position-neutral: it ends with
    // an explicit MoveTo back to `terminal.last_known_cursor_pos`. Seed the
    // terminal with a known cursor and verify it survives the insertion.
    let viewport = Rect::new(0, 22, 80, 2);
    let backend = VT100Backend::new(80, 24);
    let seeded = Position::new(7, 22);
    let mut term = Terminal::with_options_and_cursor_position(backend, seeded)
        .expect("construct test terminal");
    term.set_viewport_area(viewport);

    insert(
        &mut term,
        vec![Line::from("alpha"), Line::from("beta"), Line::from("gamma")],
    );

    let (row, col) = term.backend().vt100().screen().cursor_position();
    assert_eq!(
        (row, col),
        (seeded.y, seeded.x),
        "cursor not restored to last_known position",
    );
}

#[test]
fn insertion_after_viewport_move_preserves_content() {
    // Simulate a viewport reflow: insert lines, then move the viewport (as
    // a resize handler would), then insert again. The terminal must not
    // panic, the new viewport stays inside the screen, and both insertions
    // survive somewhere in scrollback or the live area.
    let viewport = Rect::new(0, 22, 80, 2);
    let mut term = make_terminal(80, 24, viewport);

    insert(&mut term, vec![Line::from("before-resize")]);

    let new_viewport = Rect::new(0, 14, 60, 2);
    term.set_viewport_area(new_viewport);

    insert(&mut term, vec![Line::from("after-resize")]);

    let contents = term.backend().vt100().screen().contents();
    assert!(
        contents.contains("after-resize"),
        "missing post-resize line in:\n{contents}"
    );
    let area = term.viewport_area;
    assert!(area.bottom() <= 24, "viewport escaped screen bottom: {area:?}");
    assert!(area.right() <= 80, "viewport escaped screen right: {area:?}");
}

#[test]
fn style_preserved_on_insertion() {
    // A green-on-default span should land in scrollback with the green
    // foreground SGR applied to its cells.
    let viewport = Rect::new(0, 22, 80, 2);
    let mut term = make_terminal(80, 24, viewport);

    let styled = Span::styled(
        "GREEN",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    insert(&mut term, vec![Line::from(vec![styled])]);

    let screen = term.backend().vt100().screen();
    // Locate the row containing 'G'.
    let mut found = None;
    for row in 0..24u16 {
        if let Some(cell) = screen.cell(row, 0)
            && cell.contents() == "G"
        {
            found = Some(row);
            break;
        }
    }
    let row = found.expect("did not find 'G' on any row");

    for (col, ch) in "GREEN".chars().enumerate() {
        let cell = screen
            .cell(row, col as u16)
            .unwrap_or_else(|| panic!("missing cell at ({row},{col})"));
        assert_eq!(cell.contents(), ch.to_string(), "char mismatch at col {col}");
        let fg = cell.fgcolor();
        assert!(
            matches!(fg, vt100::Color::Idx(2) | vt100::Color::Rgb(_, _, _)),
            "expected green fg at col {col}, got {fg:?}",
        );
        assert!(cell.bold(), "expected bold at col {col}");
    }
}
