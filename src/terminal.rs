use std::io::{self, Stdout, Write};

use anyhow::{Context, Result};
use crossterm::{
    SynchronizedUpdate,
    cursor::EnableBlinking,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Position, Rect, Size},
};

use crate::custom_terminal::Terminal;

pub const VIEWPORT_HEIGHT: u16 = 4;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn setup() -> Result<Tui> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnableBlinking).context("setup terminal")?;

    let mut backend = CrosstermBackend::new(stdout);
    let screen_size = backend.size().context("read terminal size")?;
    let cursor_pos = backend.get_cursor_position().unwrap_or(Position { x: 0, y: 0 });

    let mut terminal = Terminal::with_options_and_cursor_position(backend, cursor_pos)
        .context("create terminal")?;

    let viewport = bottom_anchored_viewport(screen_size, cursor_pos.y, VIEWPORT_HEIGHT);
    make_room_for_viewport(&mut terminal, viewport, cursor_pos)?;
    terminal.set_viewport_area(viewport);

    Ok(terminal)
}

/// Compute the bottom-anchored inline viewport rect. If the cursor is high
/// enough on the screen we anchor the viewport `height` rows below it;
/// otherwise we pin to the screen bottom and let the caller scroll content
/// upward to make room.
fn bottom_anchored_viewport(screen_size: Size, cursor_y: u16, height: u16) -> Rect {
    let height = height.min(screen_size.height.max(1));
    let preferred_y = cursor_y.saturating_add(1);
    let bottom_y = screen_size.height.saturating_sub(height);
    let y = preferred_y.min(bottom_y);
    Rect::new(0, y, screen_size.width, height)
}

/// Resize the inline viewport to `desired_height`, keeping it bottom-anchored.
///
/// Grow: scroll the rows above the viewport up via DECSTBM so the new top
/// rows are blank; viewport contents stay put. Shrink: scroll the rows above
/// the viewport down by the same amount so scrollback slides back into the
/// freed rows instead of leaving a blank gap above the input box. The bottom
/// rows of the scroll region are overwritten — fine, since the new (smaller)
/// viewport is repainted on top of them.
///
/// No-op while a fullscreen view (model picker / auth wizard) owns the
/// viewport.
pub fn reshape_viewport(terminal: &mut Tui, desired_height: u16) -> Result<()> {
    let screen = terminal.size().context("read terminal size for reshape")?;
    let current = terminal.viewport_area;

    if screen.height == 0 || screen.width == 0 {
        return Ok(());
    }
    if current.height >= screen.height {
        return Ok(());
    }

    let new_height = desired_height.max(VIEWPORT_HEIGHT).min(screen.height);
    if new_height == current.height && current.width == screen.width {
        return Ok(());
    }

    let new_top = screen.height - new_height;
    let new_rect = Rect::new(0, new_top, screen.width, new_height);

    if new_top < current.top() {
        let scroll_by = current.top() - new_top;
        terminal
            .backend_mut()
            .scroll_region_up(0..current.top(), scroll_by)
            .context("scroll_region_up for reshape grow")?;
    } else if new_top > current.top() {
        let scroll_by = new_top - current.top();
        terminal
            .backend_mut()
            .scroll_region_down(0..new_top, scroll_by)
            .context("scroll_region_down for reshape shrink")?;
    }

    terminal.set_viewport_area(new_rect);
    terminal.invalidate_viewport();
    Ok(())
}

/// If the inline viewport sits below the current cursor we need to scroll the
/// terminal up so that existing content is preserved above the viewport.
fn make_room_for_viewport(
    terminal: &mut Tui,
    viewport: Rect,
    cursor_pos: Position,
) -> Result<()> {
    let needed_top = viewport.top();
    if cursor_pos.y >= needed_top {
        let scroll_by = cursor_pos.y - needed_top;
        if scroll_by > 0 {
            let backend = terminal.backend_mut();
            for _ in 0..scroll_by {
                backend.write_all(b"\n").context("scroll for viewport")?;
            }
            io::Write::flush(backend).context("flush scroll")?;
        }
    }
    Ok(())
}

pub fn restore(terminal: &mut Tui) {
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    disable_raw_mode().ok();
    let viewport_bottom = terminal.viewport_area.bottom();
    execute!(
        terminal.backend_mut(),
        crossterm::cursor::MoveTo(0, viewport_bottom),
        crossterm::cursor::Show
    )
    .ok();
    println!();
}

/// Enter the alternate screen and grow the inline viewport to fill the
/// terminal. Returns the prior viewport `Rect` so the caller can restore it.
pub fn enter_fullscreen(terminal: &mut Tui) -> Result<Rect> {
    let saved = terminal.viewport_area;
    execute!(terminal.backend_mut(), EnterAlternateScreen)
        .context("enter alternate screen")?;
    let size = terminal.size().context("read terminal size")?;
    let full = Rect::new(0, 0, size.width, size.height);
    terminal.set_viewport_area(full);
    terminal.clear().context("clear fullscreen viewport")?;
    Ok(saved)
}

/// Wrap a block of terminal writes in a DEC mode 2026 synchronized update so
/// supporting emulators commit the frame as one atomic flush. No-op on
/// terminals that don't recognize the escapes.
pub fn with_sync_update<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut stdout = io::stdout();
    stdout.sync_update(|_| f())?
}

/// Leave the alternate screen and restore the previously-saved inline viewport.
pub fn leave_fullscreen(terminal: &mut Tui, saved: Rect) -> Result<()> {
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("leave alternate screen")?;
    terminal.set_viewport_area(saved);
    terminal.clear().context("clear restored viewport")?;
    Ok(())
}
