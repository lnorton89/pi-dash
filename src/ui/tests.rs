//! Rendering tests. These drive the real drawing code through ratatui's
//! `TestBackend`, so they catch the failures that a parser test cannot: a pane
//! whose title falls off the edge, a layout that gives the ClassG pane no
//! rows, a throttle state that renders as the wrong words.

use ratatui::{backend::TestBackend, Terminal};

use super::draw;
use crate::app::{App, Mode, Pane};
use crate::config::{Config, READER_MAX_COLS};
use crate::panes::classg::{HealthResponse, SensorHealth, Snapshot};
use crate::panes::health::Throttle;

/// Points the API at a port nothing can be listening on, so the poller thread
/// a test app spawns cannot reach anything real.
fn test_app() -> App {
    App::new(Config {
        api: "http://127.0.0.1:1".to_string(),
        ..Config::default()
    })
}

fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    terminal
        .draw(|frame| draw(frame, app))
        .expect("draw must not fail");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn contains(rows: &[String], needle: &str) -> bool {
    rows.iter().any(|row| row.contains(needle))
}

#[test]
fn the_wide_layout_shows_every_pane_at_once() {
    let mut app = test_app();
    let rows = render(&mut app, 140, 44);
    for title in ["System", "Pi health", "Radios & network", "ClassG"] {
        assert!(
            contains(&rows, title),
            "missing {title} in:\n{}",
            rows.join("\n")
        );
    }
}

#[test]
fn the_reader_column_is_clamped_not_proportional() {
    let mut app = test_app();
    // On a very wide terminal 42% would be 126 columns of mostly padding.
    let rows = render(&mut app, 300, 44);
    let health_row = rows
        .iter()
        .find(|row| row.contains("Pi health"))
        .expect("health pane title");
    let start = health_row.find("Pi health").expect("title offset");
    assert!(
        start as u16 >= 300 - READER_MAX_COLS - 2,
        "reader column started at {start}, so it is wider than the clamp"
    );
}

#[test]
fn a_narrow_terminal_shows_one_pane_at_a_time() {
    let mut app = test_app();
    app.focus = Pane::Radios;
    let rows = render(&mut app, 70, 24);
    assert!(contains(&rows, "Radios & network"));
    assert!(!contains(&rows, "Pi health"), "panes must not be stacked");
    assert!(
        contains(&rows, "tab/1-4 pane"),
        "the footer must say how to switch"
    );
}

#[test]
fn every_pane_still_draws_in_a_tiny_terminal() {
    let mut app = test_app();
    for pane in Pane::ALL {
        app.focus = pane;
        // Small enough that several panes have no room for their content at
        // all. Nothing here may panic on a subtraction or an index.
        render(&mut app, 24, 6);
        render(&mut app, 20, 3);
    }
}

#[test]
fn a_missing_vcgencmd_reads_as_unknown_not_as_ok() {
    let mut app = test_app();
    app.health.throttle = None;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "no vcgencmd here"), "{}", rows.join("\n"));
    assert!(
        !contains(&rows, "clean since boot"),
        "'cannot tell' must never render as 'clean'"
    );
}

#[test]
fn a_clean_register_and_a_sticky_one_render_differently() {
    let mut app = test_app();
    app.health.throttle = Some(Throttle::decode(0));
    let clean = render(&mut app, 140, 44);
    assert!(contains(&clean, "clean since boot"));

    app.health.throttle = Some(Throttle::decode(0x50000));
    let sticky = render(&mut app, 140, 44);
    assert!(
        contains(&sticky, "nothing right now"),
        "{}",
        sticky.join("\n")
    );
    assert!(contains(&sticky, "under-voltage, throttled"));
    assert!(
        contains(&sticky, "0x50000"),
        "the raw code must still be readable"
    );
    assert!(
        !contains(&sticky, "UNDER-VOLTAGE NOW"),
        "a sticky bit must not be reported as live"
    );

    // Both halves set: neither may be truncated away by the other, which is
    // what a single combined line did.
    app.health.throttle = Some(Throttle::decode(0x50005));
    let live = render(&mut app, 140, 44);
    assert!(
        contains(&live, "UNDER-VOLTAGE NOW, throttled"),
        "{}",
        live.join("\n")
    );
    assert!(contains(&live, "under-voltage, throttled"));
    assert!(contains(&live, "0x50005"));
}

#[test]
fn a_dead_api_degrades_into_a_hint_and_leaves_the_rest_alone() {
    let mut app = test_app();
    app.classg.snapshot = Snapshot {
        error: Some("connection refused".to_string()),
        ..Snapshot::default()
    };
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "not reachable"));
    assert!(contains(&rows, "make dev"));
    // The other three panes are unaffected — that is the whole contract.
    assert!(contains(&rows, "Pi health"));
    assert!(contains(&rows, "Radios & network"));
    assert!(contains(&rows, "System"));
}

#[test]
fn a_down_sensor_gets_its_reason_on_its_own_line() {
    let mut app = test_app();
    app.classg.snapshot = Snapshot {
        health: Some(HealthResponse {
            status: "degraded".to_string(),
            uptime_s: 4000,
            version: "0.4.1".to_string(),
            sensors: vec![SensorHealth {
                sensor_id: "sdr-1".to_string(),
                healthy: false,
                reason: Some("rtl_sdr: device not found".to_string()),
                ..SensorHealth::default()
            }],
            fusion: None,
        }),
        ..Snapshot::default()
    };
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "degraded"));
    assert!(contains(&rows, "DOWN"));
    assert!(
        contains(&rows, "rtl_sdr: device not found"),
        "{}",
        rows.join("\n")
    );
    assert!(
        contains(&rows, "not configured"),
        "fusion state must still show"
    );
}

#[test]
fn the_help_overlay_covers_the_screen_and_names_the_config_source() {
    let mut app = test_app();
    app.mode = Mode::Help;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "Help"));
    assert!(contains(&rows, "CLASSG_API"));
    assert!(contains(&rows, "built-in defaults"));
}
