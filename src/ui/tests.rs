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
use crate::panes::system::{MemInfo, ProcRow};

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

/// Fills the system pane with a plausible sample so the meters and the graph
/// have something to draw. `App::sample` reads /proc, which on a dev machine
/// that is not Linux yields nothing at all.
fn with_load(app: &mut App) {
    app.system.unavailable = None;
    app.system.cpu_pct = Some(87.0);
    app.system.core_pct = vec![Some(100.0), Some(99.5), Some(72.0), Some(3.0)];
    app.system.mem = MemInfo {
        total_kb: 3_800_000,
        available_kb: 2_400_000,
        cached_kb: 1_200_000,
        buffers_kb: 90_000,
        swap_total_kb: 524_288,
        swap_free_kb: 524_288,
    };
    app.system.load = [15.67, 5.21, 2.24];
    app.system.uptime_secs = 26_700;
    app.system.task_count = 431;
    app.system.runnable = 4;
    // A ramp, so the graph has a shape rather than a flat line.
    app.system.cpu_history = (0..200).map(|i| (i % 100) as f64 / 99.0).collect();
    app.system.procs = vec![
        ProcRow {
            pid: 91_997,
            name: "npm ci".to_string(),
            state: 'R',
            cpu_pct: 88.0,
            rss_kb: 482_000,
            cmdline: "/usr/bin/node /usr/lib/node_modules/npm/bin/npm-cli.js ci".to_string(),
        },
        ProcRow {
            pid: 68_260,
            name: "dump1090-mutability".to_string(),
            state: 'D',
            cpu_pct: 47.0,
            rss_kb: 6_000,
            cmdline: "/usr/bin/dump1090-mutability --net --quiet".to_string(),
        },
    ];
}

fn has_braille(rows: &[String]) -> bool {
    rows.iter().any(|row| {
        row.chars()
            // U+2800 is blank braille, which is not proof of a graph.
            .any(|c| ('\u{2801}'..='\u{28FF}').contains(&c))
    })
}

#[test]
fn the_system_pane_draws_gradient_meters_and_a_history_graph() {
    let mut app = test_app();
    with_load(&mut app);
    let rows = render(&mut app, 140, 44);
    let screen = rows.join("\n");

    assert!(
        screen.contains('\u{2588}'),
        "the meters must be block-filled, not ASCII:\n{screen}"
    );
    assert!(
        has_braille(&rows),
        "the CPU history graph is missing:\n{screen}"
    );
    // The btop-shaped additions, each of which is a number this pane already
    // sampled and then threw away.
    assert!(
        contains(&rows, "reclaimable"),
        "cache meter missing:\n{screen}"
    );
    assert!(
        contains(&rows, "431 tasks, 4 running"),
        "task counts missing"
    );
    assert!(
        contains(&rows, "up 0d7h25m"),
        "uptime chip missing:\n{screen}"
    );
    assert!(contains(&rows, "npm ci"), "process table missing");
}

#[test]
fn ascii_mode_leaves_no_drawing_glyph_anywhere_on_screen() {
    // The whole point of the mode: on the Pi's framebuffer console every one
    // of these renders as a replacement character. The frame counts too —
    // drawing ASCII meters inside a Unicode box is what this used to do.
    let mut app = App::new(Config {
        api: "http://127.0.0.1:1".to_string(),
        glyphs: "ascii".to_string(),
        ..Config::default()
    });
    with_load(&mut app);

    for (width, height) in [(140, 44), (70, 24), (24, 6)] {
        for row in render(&mut app, width, height) {
            for ch in row.chars() {
                let drawing = matches!(ch,
                    '\u{2500}'..='\u{257F}'     // box drawing
                    | '\u{2580}'..='\u{259F}'   // block elements and shades
                    | '\u{2800}'..='\u{28FF}'   // braille
                );
                assert!(
                    !drawing,
                    "U+{:04X} survived ascii mode at {width}x{height} in: {row}",
                    ch as u32
                );
            }
        }
    }
}

#[test]
fn a_short_pane_drops_the_graph_rather_than_the_process_table() {
    let mut app = test_app();
    with_load(&mut app);
    // Narrow layout, so the system pane is the only thing on screen and gets
    // the terminal's whole height — which here is too little for a graph
    // worth looking at.
    app.focus = Pane::System;
    let rows = render(&mut app, 90, 16);
    let screen = rows.join("\n");
    assert!(
        !has_braille(&rows),
        "there is no room for a graph here:\n{screen}"
    );
    assert!(
        contains(&rows, "npm ci"),
        "the table must survive:\n{screen}"
    );
}

#[test]
fn no_system_pane_label_is_ever_sliced_by_the_pane_edge() {
    // The pane draws without wrapping, so anything too long for the line is
    // cut wherever the pane ends rather than wrapped. That produced
    // `1.9G reclaimab` and a load average truncated after the word `load` at
    // the width the two-column layout actually gives this pane.
    let mut app = test_app();
    with_load(&mut app);
    app.focus = Pane::System;

    /// The rest of a field's line, up to the pane's right border. From 100
    /// columns the other three panes share these rows, so the row alone is
    /// not the field.
    fn cell<'a>(row: &'a str, label: &str) -> Option<&'a str> {
        let tail = row.split(label).nth(1)?;
        Some(tail.split(['\u{2502}', '|']).next().unwrap_or(tail))
    }

    for width in 30..150u16 {
        for row in render(&mut app, width, 30) {
            if let Some(cache) = cell(&row, "  cache ") {
                assert!(
                    !cache.contains("reclaimab") || cache.contains("reclaimable"),
                    "sliced at width {width}: {row}"
                );
            }
            if let Some(swap) = cell(&row, "  swap ") {
                assert!(
                    !swap.contains("task") || swap.contains(" tasks"),
                    "sliced at width {width}: {row}"
                );
                assert!(
                    !swap.contains("runnin") || swap.contains(" running"),
                    "sliced at width {width}: {row}"
                );
            }
            if let Some(cpu) = cell(&row, "  cpu ") {
                assert!(
                    !cpu.contains(" core") || cpu.contains(" cores"),
                    "sliced at width {width}: {row}"
                );
                if let Some(load) = cpu.split(" load ").nth(1) {
                    let numbers: Vec<&str> = load.split_whitespace().collect();
                    assert_eq!(numbers.len(), 3, "load cut short at width {width}: {row}");
                    for n in numbers {
                        assert!(
                            n.parse::<f64>().is_ok(),
                            "partial number {n:?} at width {width}: {row}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn adjacent_process_columns_never_run_into_each_other() {
    // A name that exactly fills its column used to touch the command line
    // beside it: `dump1090-mutabilit/usr/bin/dump1090-mutability --net`.
    let mut app = test_app();
    with_load(&mut app);
    // Filler chosen so the pair cannot occur anywhere else on screen — `ab`
    // matched the footer's own "tab/1-4 pane".
    app.system.procs[0].name = "W".repeat(40);
    app.system.procs[0].cmdline = "Z".repeat(60);
    app.focus = Pane::System;

    for width in 40..200u16 {
        for row in render(&mut app, width, 30) {
            assert!(!row.contains("WZ"), "columns touch at width {width}: {row}");
        }
    }
}

#[test]
fn a_kernel_thread_is_bracketed_rather_than_left_blank() {
    let mut app = test_app();
    with_load(&mut app);
    app.system.procs[1].name = "kworker/0:1".to_string();
    app.system.procs[1].cmdline = String::new();
    app.focus = Pane::System;
    let rows = render(&mut app, 160, 30);
    assert!(
        contains(&rows, "[kworker/0:1]"),
        "an empty cmdline must read as a kernel thread, not a failed read:\n{}",
        rows.join("\n")
    );
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
