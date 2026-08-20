//! All ratatui drawing, one module per pane:
//!
//! - [`system`] — CPU, memory and the process table (the btop replacement)
//! - [`health`] — temperature, power, clock, throttle bits, disk and I/O
//! - [`radios`] — interfaces, monitor mode, USB radios
//! - [`classg`] — the ClassG API pane
//!
//! There is no animation anywhere in here, deliberately. The Bash version
//! redrew by homing the cursor and clearing each line as it was rewritten
//! rather than clearing the screen, because on a Pi over SSH a full clear per
//! tick flickers badly enough to be tiring to watch. Ratatui's renderer
//! already does better than that — it diffs the buffer and writes only the
//! cells that changed — but only if the frame is mostly the same as the last
//! one. A shimmering border would repaint every cell every tick and hand the
//! flicker straight back.

pub(crate) mod classg;
pub(crate) mod gauge;
pub(crate) mod health;
pub(crate) mod radios;
pub(crate) mod system;

use chrono::Local;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Mode, Pane};
use crate::config::{NARROW_COLS, READER_MAX_COLS, READER_MIN_COLS};
use crate::ui::gauge::Glyphs;

/// Green/amber/red, used identically by every pane so a colour means the same
/// thing wherever you see it.
pub(crate) const OK: Color = Color::Green;
pub(crate) const WARN: Color = Color::Yellow;
pub(crate) const BAD: Color = Color::Red;
pub(crate) const DIM: Color = Color::DarkGray;

pub(crate) fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    if area.width >= NARROW_COLS {
        draw_wide(frame, chunks[1], app);
    } else {
        draw_narrow(frame, chunks[1], app);
    }
    draw_footer(frame, chunks[2], app, area.width >= NARROW_COLS);

    if app.mode == Mode::Help {
        draw_help(frame, area, app);
    }
}

/// The two-column layout. The reader column is clamped rather than
/// proportional: the panes in it are built around a ~46-column body, and
/// wider than about 60 the right-hand side is mostly padding — on a large
/// monitor that means a third of the screen showing nothing. The system pane
/// gets every column the readers cannot use.
fn draw_wide(frame: &mut Frame, area: Rect, app: &mut App) {
    let reader = area
        .width
        .saturating_mul(42)
        .saturating_div(100)
        .clamp(READER_MIN_COLS, READER_MAX_COLS);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(reader)])
        .split(area);

    system::draw(frame, columns[0], app);

    // Both fixed-height panes write a known number of lines, so measure them
    // and hand every remaining row to the ClassG pane — it is the only one
    // with more to say when it has the room, and it scales its track and
    // detection lists to the height it is given. Splitting the column into
    // equal thirds instead gave the health pane 24 rows for its 7 lines.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(health::CONTENT_ROWS + 2),
            Constraint::Length(app.radios.content_rows() + 2),
            Constraint::Min(4),
        ])
        .split(columns[1]);

    health::draw(frame, rows[0], app);
    radios::draw(frame, rows[1], app);
    classg::draw(frame, rows[2], app);
}

/// One pane at a time. Stacking four panes into fewer than 100 columns leaves
/// every one of them unreadable; the Bash version put btop in its own tmux
/// window at this point, which is the same trade made without tmux.
fn draw_narrow(frame: &mut Frame, area: Rect, app: &mut App) {
    match app.focus {
        Pane::System => system::draw(frame, area, app),
        Pane::Health => health::draw(frame, area, app),
        Pane::Radios => radios::draw(frame, area, app),
        Pane::Classg => classg::draw(frame, area, app),
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let left = Line::from(vec![
        Span::styled(
            " pi-dash ",
            Style::default()
                .bg(app.accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(app.host.clone(), Style::default().fg(Color::White)),
        Span::styled("  ", Style::default()),
        Span::styled(
            app.config.api.trim_start_matches("http://").to_string(),
            Style::default().fg(DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{} ", Local::now().format("%H:%M:%S")),
            Style::default().fg(DIM),
        )))
        .alignment(Alignment::Right),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, wide: bool) {
    let keys = if wide {
        if app.system.filter_editing {
            // While the filter owns the keyboard, the footer has to say so:
            // every other binding on it is a lie until Enter or Esc.
            " typing a filter · enter keep · esc clear ".to_string()
        } else {
            format!(
                " q quit · r refresh · s sort by {} · f filter · ? help ",
                app.system.sort.next().label().to_ascii_lowercase()
            )
        }
    } else {
        format!(
            " tab/1-4 pane ({}) · q quit · r refresh · s sort · f filter · ? help ",
            app.focus.title()
        )
    };
    frame.render_widget(
        Paragraph::new(keys)
            .alignment(Alignment::Center)
            .style(Style::default().fg(DIM)),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.min(64);
    let height = area.height.min(23);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let source = app
        .config
        .source
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "built-in defaults".to_string());

    let lines = vec![
        Line::from(Span::styled(
            " Keys",
            Style::default().fg(app.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from("  q / Esc / Ctrl-C   quit"),
        Line::from("  r                  sample now, don't wait for the tick"),
        Line::from("  s                  sort processes by CPU or by memory"),
        Line::from("  f                  filter processes by name or command line"),
        Line::from("  up/down/pgup/pgdn  scroll the process table, home for the top"),
        Line::from("  tab / 1-4          focus a pane; its number is in its title"),
        Line::from("  Ctrl-L             force a full repaint"),
        Line::from("  ?                  close this"),
        Line::default(),
        Line::from(Span::styled(
            " Config",
            Style::default().fg(app.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("  from      {source}")),
        Line::from(format!("  api       {}", app.config.api)),
        Line::from(format!(
            "  interval  {:.1}s local, {:.1}s api",
            app.config.interval.as_secs_f64(),
            app.config.api_interval.as_secs_f64()
        )),
        // Whether a session is configured, never the token. This overlay is
        // the thing somebody screenshots when asking why a pane is empty.
        Line::from(format!(
            "  credential {}",
            match (&app.config.session, &app.config.local_token) {
                (Some(_), _) => "set — sent as the classg_session cookie",
                (None, Some(_)) =>
                    "this unit's own local agent token — sent as Authorization: Bearer",
                (None, None) => "none — only /health and /auth/me are public",
            }
        )),
        Line::default(),
        Line::from(Span::styled(
            "  env CLASSG_API, CLASSG_SESSION and CLASSG_DASH_INTERVAL",
            Style::default().fg(DIM),
        )),
        Line::from(Span::styled(
            "  override the file.  pi-dash --print-config says which won.",
            Style::default().fg(DIM),
        )),
    ];

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(pane_block(" Help ", app.accent, app.glyphs)),
        popup,
    );
}

/// The standard bordered pane.
///
/// The frame follows the same glyph set as the meters inside it. Drawing an
/// ASCII meter inside a Unicode box, which is what this did before, solves
/// half of the framebuffer-console problem and leaves the frame in
/// replacement characters.
pub(crate) fn pane_block<'a>(title: &'a str, border: Color, glyphs: Glyphs) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(glyphs.border_set())
        .border_style(Style::default().fg(border))
        .title(title)
        .title_style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )
        .title_alignment(Alignment::Left)
}

/// The same, with btop's hotkey tab: the pane's own number, accented, ahead of
/// its name.
///
/// `Tab` and `1`-`4` have switched panes since the rewrite and nothing on
/// screen has ever said which number belongs to which pane -- a binding you
/// have to read the help card to discover is one nobody uses. btop puts its
/// hotkeys in the box titles for exactly this reason.
///
/// Numbered on every terminal, not only the narrow ones that need the keys.
/// A title that gains and loses a digit as the window resizes is a title you
/// stop reading.
pub(crate) fn numbered_pane_block<'a>(
    pane: Pane,
    title: &'a str,
    accent: Color,
    glyphs: Glyphs,
    focused: bool,
) -> Block<'a> {
    let index = Pane::ALL.iter().position(|p| *p == pane).unwrap_or(0) + 1;
    // The focused pane keeps the accent; the rest recede. Without this the
    // numbers would advertise keys that do nothing: `focus` only ever chose
    // which pane to draw on a narrow terminal, so on a wide one 1-4 and Tab
    // moved something invisible. btop highlights its focused box for exactly
    // this reason.
    let border = if focused { accent } else { DIM };
    // Built as one title rather than layered on top of pane_block's, because
    // two left-aligned titles are laid out in the order they were added and
    // the number has to come first to read as a key.
    Block::default()
        .borders(Borders::ALL)
        .border_set(glyphs.border_set())
        .border_style(Style::default().fg(border))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                index.to_string(),
                Style::default().fg(border).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", title.trim()),
                Style::default()
                    .fg(if focused { Color::Gray } else { DIM })
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_alignment(Alignment::Left)
}

/// A label/value line, with the label in the fixed-width gutter every pane
/// shares so values line up down the whole column.
pub(crate) fn field<'a>(label: &str, value: Vec<Span<'a>>) -> Line<'a> {
    let mut spans = vec![Span::styled(
        format!("  {label:<7}"),
        Style::default().fg(DIM),
    )];
    spans.extend(value);
    Line::from(spans)
}

/// A column-header row for a table inside a pane.
///
/// Every table on screen uses this, so a heading always looks like a heading
/// and never like the first row of data — which is what a plain bold line
/// looked like next to a process name.
pub(crate) fn table_header<'a>(text: String) -> Line<'a> {
    Line::from(Span::styled(text, header_style()))
}

/// The style [`table_header`] applies, for the one table that builds its
/// heading span by span because it colours the column it is sorted by. Shared
/// so that heading cannot drift into looking like something else.
pub(crate) fn header_style() -> Style {
    Style::default().fg(DIM).add_modifier(Modifier::BOLD)
}

/// Appends a dim suffix, but only if the line still has room for all of it.
///
/// The panes render without wrapping, so a tail that does not fit is not
/// wrapped — it is sliced wherever the pane ends, which is how a 45-column
/// pane produced `1.9G reclaimab` and a load average cut off after the word
/// `load`. Returns whether it fitted, so a caller can offer a shorter form.
pub(crate) fn push_if_fits<'a>(
    spans: &mut Vec<Span<'a>>,
    used: &mut usize,
    width: usize,
    text: String,
) -> bool {
    let len = text.chars().count();
    if *used + len > width {
        return false;
    }
    *used += len;
    spans.push(Span::styled(text, Style::default().fg(DIM)));
    true
}

/// The left gutter [`field`] establishes: two spaces plus a seven-column
/// label. Tables line their first column up with it.
pub(crate) const GUTTER: usize = 9;

/// Green below `warn`, amber below `bad`, red above.
pub(crate) fn threshold_color(value: f64, warn: f64, bad: f64) -> Color {
    if value >= bad {
        BAD
    } else if value >= warn {
        WARN
    } else {
        OK
    }
}

#[cfg(test)]
mod tests;
