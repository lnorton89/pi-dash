//! The event loop.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::Backend, Terminal};

use crate::app::{App, Mode, Pane};
use crate::ui::draw;

/// How often the header clock is allowed to advance. Independent of the
/// sample interval so a 10-second sample cadence does not leave a clock that
/// looks stopped, and bounded so a 0.25-second one does not repaint the whole
/// frame four times a second for the sake of it.
const CLOCK_TICK: Duration = Duration::from_secs(1);

/// The longest the loop will block waiting for a key. Keeps quitting
/// responsive when the sample interval is long.
const MAX_POLL: Duration = Duration::from_millis(250);

pub(crate) fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> Result<()> {
    let start = Instant::now();
    app.sample(start);

    let mut next_sample = start + app.config.interval;
    let mut next_clock = start + CLOCK_TICK;
    // Redraw only when something changed. A dashboard that repaints on every
    // loop iteration is a dashboard that keeps a Pi's CPU warm doing nothing,
    // which on this box is not a neutral cost — it is one of the things that
    // brings on the thermal throttling the health pane is there to report.
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|frame| draw(frame, &mut app))?;
            dirty = false;
        }

        let now = Instant::now();
        let deadline = next_sample.min(next_clock);
        let timeout = deadline.saturating_duration_since(now).min(MAX_POLL);

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match handle_key(&mut app, key) {
                        Action::Quit => return Ok(()),
                        Action::Repaint => {
                            terminal.clear()?;
                            dirty = true;
                        }
                        Action::SampleNow => {
                            app.sample(Instant::now());
                            dirty = true;
                        }
                        Action::Redraw => dirty = true,
                        Action::Ignore => {}
                    }
                }
                Event::Resize(width, height) => {
                    // Resize from the event's own dimensions rather than
                    // waiting for the next draw to query them, and let
                    // Terminal::resize do the clear so cells the new layout
                    // does not cover cannot linger as on-screen garbage.
                    terminal.resize(ratatui::layout::Rect::new(0, 0, width, height))?;
                    dirty = true;
                }
                // Spelt out rather than caught by `_`, so a crossterm release
                // that adds an event kind fails to build here instead of
                // silently joining the pile this loop throws away.
                Event::Key(_)
                | Event::FocusGained
                | Event::FocusLost
                | Event::Mouse(_)
                | Event::Paste(_) => {}
            }
        }

        let now = Instant::now();
        if now >= next_sample {
            app.sample(now);
            // Drop missed samples rather than catching up: after the terminal
            // has been suspended, replaying a backlog of ticks would burn CPU
            // producing frames nobody can see, and every rate in them would be
            // computed over a near-zero interval.
            next_sample = now + app.config.interval;
            dirty = true;
        }
        if now >= next_clock {
            next_clock = now + CLOCK_TICK;
            dirty = true;
        }
        // The API poller runs on its own thread and its own clock; picking up
        // whatever it has posted is the only thing the loop does for it.
        if app.classg.drain() {
            dirty = true;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Ignore,
    Redraw,
    Repaint,
    SampleNow,
    Quit,
}

// Keys are dispatched by comparison rather than by one big `match` on
// `KeyCode`. `KeyCode` has thirty-odd variants, twenty-five of which this
// dashboard will never bind, so a `match` needs a `_` arm — and a wildcard over
// somebody else's enum is exactly what the lint table forbids, for the good
// reason that it hides a new variant instead of surfacing it. Spelling all
// thirty out to reach `Action::Ignore` would be worse than either. Comparing
// the handful we do bind says the same thing in the space it deserves.
fn handle_key(app: &mut App, key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        // The classic terminal fixer-upper, for when the display has drifted
        // out of sync with reality.
        if key.code == KeyCode::Char('l') {
            return Action::Repaint;
        }
        return Action::Ignore;
    }

    if app.mode == Mode::Help {
        // Any key closes help — it is a reference card, not a mode you work in.
        app.mode = Mode::Normal;
        return Action::Redraw;
    }

    // Every printable binding, resolved from the character itself. `char` is
    // not an enum, so the catch-all here costs nothing in future-proofing.
    if let KeyCode::Char(c) = key.code {
        return match c {
            'q' => Action::Quit,
            '?' => {
                app.mode = Mode::Help;
                Action::Redraw
            }
            'r' => Action::SampleNow,
            'l' => focus_next(app),
            'h' => focus_previous(app),
            '1'..='4' => {
                let index = (c as u8 - b'1') as usize;
                if let Some(pane) = Pane::ALL.get(index) {
                    app.focus = *pane;
                }
                Action::Redraw
            }
            _ => Action::Ignore,
        };
    }

    if key.code == KeyCode::Esc {
        return Action::Quit;
    }
    if matches!(key.code, KeyCode::Tab | KeyCode::Right) {
        return focus_next(app);
    }
    if matches!(key.code, KeyCode::BackTab | KeyCode::Left) {
        return focus_previous(app);
    }
    Action::Ignore
}

fn focus_next(app: &mut App) -> Action {
    app.focus = app.focus.next();
    Action::Redraw
}

fn focus_previous(app: &mut App) -> Action {
    app.focus = app.focus.previous();
    Action::Redraw
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn test_app() -> App {
        // Point at a port nothing listens on so the poller thread cannot
        // reach anything real from a test run.
        App::new(Config {
            api: "http://127.0.0.1:1".to_string(),
            ..Config::default()
        })
    }

    #[test]
    fn quit_keys_quit() {
        let mut app = test_app();
        assert_eq!(
            handle_key(&mut app, press(KeyCode::Char('q'))),
            Action::Quit
        );
        assert_eq!(handle_key(&mut app, press(KeyCode::Esc)), Action::Quit);
        assert_eq!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Action::Quit
        );
    }

    #[test]
    fn help_opens_and_any_key_closes_it() {
        let mut app = test_app();
        handle_key(&mut app, press(KeyCode::Char('?')));
        assert_eq!(app.mode, Mode::Help);
        // Even 'q' closes help rather than quitting, so a reflexive keypress
        // does not take the dashboard down with it.
        assert_eq!(
            handle_key(&mut app, press(KeyCode::Char('q'))),
            Action::Redraw
        );
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn ctrl_c_still_quits_from_the_help_overlay() {
        let mut app = test_app();
        app.mode = Mode::Help;
        assert_eq!(
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Action::Quit
        );
    }

    #[test]
    fn number_keys_and_tab_select_panes() {
        let mut app = test_app();
        handle_key(&mut app, press(KeyCode::Char('3')));
        assert_eq!(app.focus, Pane::Radios);
        handle_key(&mut app, press(KeyCode::Tab));
        assert_eq!(app.focus, Pane::Classg);
        handle_key(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.focus, Pane::Radios);
        // Out of range does nothing rather than panicking on the index.
        handle_key(&mut app, press(KeyCode::Char('9')));
        assert_eq!(app.focus, Pane::Radios);
    }
}
