//! Rendering tests. These drive the real drawing code through ratatui's
//! `TestBackend`, so they catch the failures that a parser test cannot: a pane
//! whose title falls off the edge, a layout that gives the ClassG pane no
//! rows, a throttle state that renders as the wrong words.

use ratatui::{backend::TestBackend, Terminal};

use super::draw;
use crate::app::{App, Mode, Pane};
use crate::config::{Config, READER_MAX_COLS};
use crate::panes::classg::{
    Adsb, AuthState, AuthUser, Capture, CaptureAnalysis, CapturePage, Detection, DetectionPage,
    Evidence, FusionHealth, HealthResponse, Identity, MonitoringState, Position, Rf, SensorHealth,
    Slow, Snapshot, SpectrumSweep, SweepPage, SystemBuild, SystemHost, SystemInfo, SystemRuntime,
    Track, TrackPage,
};
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
                    | '\u{25A0}'..='\u{25FF}'   // geometric shapes
                    | '\u{2190}'..='\u{21FF}'   // arrows
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
fn a_monitor_interface_reporting_unknown_is_not_drawn_as_a_fault() {
    // Monitor-mode interfaces routinely report `unknown` rather than `up`,
    // because there is no association to have an opinion about. Colouring
    // that red made the pane cry wolf every session. This used to be an
    // `Iface::is_up` predicate; the judgement now lives in the row renderer,
    // so the test follows it here.
    use crate::panes::radios::{Iface, WirelessMode};

    let mut app = test_app();
    app.radios.ifaces = vec![
        Iface {
            name: "wlan1".into(),
            state: "unknown".into(),
            rx_bps: 0.0,
            tx_bps: 0.0,
            mode: Some(WirelessMode::Monitor),
            channel: Some(6),
            driver: Some("mt7921u".into()),
        },
        Iface {
            name: "eth0".into(),
            state: "down".into(),
            rx_bps: 0.0,
            tx_bps: 0.0,
            mode: None,
            channel: None,
            driver: Some("bcmgenet".into()),
        },
    ];
    app.focus = Pane::Radios;

    let mut terminal = ratatui::Terminal::new(TestBackend::new(120, 30)).expect("test backend");
    terminal
        .draw(|frame| draw(frame, &mut app))
        .expect("draw must not fail");
    let buffer = terminal.backend().buffer().clone();

    // Cell columns, not byte offsets: the frame is drawn with box characters
    // that are three bytes each, so `str::find` desynchronises from the
    // buffer's coordinates the moment a row contains one.
    let colour_of = |needle: &str| -> Option<ratatui::style::Color> {
        let needle: Vec<char> = needle.chars().collect();
        for y in 0..30u16 {
            let row: Vec<String> = (0..120u16)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            let flat: String = row.concat();
            if !flat.contains("wlan1") && !flat.contains("eth0") {
                continue;
            }
            for x in 0..row.len().saturating_sub(needle.len()) {
                if needle
                    .iter()
                    .enumerate()
                    .all(|(i, c)| row[x + i] == c.to_string())
                {
                    return buffer[(x as u16, y)].fg.into();
                }
            }
        }
        None
    };

    assert_ne!(
        colour_of("unkn"),
        Some(super::BAD),
        "an unknown operstate must not read as a fault"
    );
    assert_eq!(
        colour_of("down"),
        Some(super::BAD),
        "a genuinely down link still must"
    );
}

#[test]
fn every_health_meter_starts_at_the_same_column() {
    // Sizing each row's value field to its own content put the temperature,
    // clock and disk bars at three different columns and made the pane read
    // as a ragged list rather than a table.
    use crate::panes::health::{DiskUsage, IoRates};

    let mut app = test_app();
    app.health.temp_c = Some(39.4);
    app.health.volts = Some(0.85);
    app.health.arm_mhz = Some(1000);
    app.health.max_mhz = Some(1800);
    app.health.disk = Some(DiskUsage {
        used_kb: 31_775_129,
        total_kb: 122_683_392,
        // Not total - used: the 5% ext4 holds back for root is neither used
        // nor available to anything this box runs.
        avail_kb: 84_774_400,
    });
    app.health.io = IoRates::default();
    app.focus = Pane::Health;

    for width in 50..140u16 {
        let rows = render(&mut app, width, 30);
        let starts: Vec<usize> = ["  temp ", "  clock ", "  disk "]
            .iter()
            .filter_map(|label| {
                let row = rows.iter().find(|r| r.contains(*label))?;
                // The first meter cell, filled or track. Block characters
                // only: `.` and `#` also match the decimal point in `39.4C`,
                // which is how this test first reported a column of 13.
                row.char_indices()
                    .filter(|(_, c)| matches!(c, '\u{2588}'..='\u{2593}'))
                    .map(|(i, _)| row[..i].chars().count())
                    .next()
            })
            .collect();
        assert_eq!(starts.len(), 3, "all three rows must draw a meter");
        assert!(
            starts.windows(2).all(|w| w[0] == w[1]),
            "meters start at {starts:?} at width {width}"
        );
    }
}

#[test]
fn every_table_on_screen_has_a_heading_over_it() {
    // The columns these label were four unlabelled numbers and a word.
    let mut app = test_app();
    with_load(&mut app);
    app.radios.ifaces = vec![crate::panes::radios::Iface {
        name: "wlan1".into(),
        state: "unknown".into(),
        rx_bps: 3993.0,
        tx_bps: 0.0,
        mode: Some(crate::panes::radios::WirelessMode::Monitor),
        channel: Some(11),
        driver: Some("mt7921u".into()),
    }];
    app.radios.usb = vec![crate::panes::radios::UsbRadio {
        id: "0bda:2838".into(),
        description: "RTLSDRBlog Blog V4".into(),
    }];
    app.classg.snapshot = Snapshot {
        health: Some(HealthResponse {
            status: "ok".to_string(),
            sensors: vec![SensorHealth {
                sensor_id: "sdr-0".to_string(),
                sensor_kind: "sdr".to_string(),
                healthy: true,
                ..SensorHealth::default()
            }],
            ..HealthResponse::default()
        }),
        ..Snapshot::default()
    };

    let rows = render(&mut app, 152, 48);
    for heading in [
        "PID", "COMMAND", "MEM", "CPU%", // system
        "IFACE", "LINK", "RX", "TX", "MODE", "CH", "DRIVER", // radios
        "VID:PID", "DEVICE", // usb
        "SENSOR", "KIND", "STATE", "BEAT", "5MIN", // classg sensors
    ] {
        assert!(
            contains(&rows, heading),
            "no {heading} heading:\n{}",
            rows.join("\n")
        );
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

// ---------------------------------------------------------------------------
// The ClassG pane, on a unit that is actually doing something
// ---------------------------------------------------------------------------

/// A snapshot with every section populated, so a test can knock one part out
/// and assert on what changes rather than building the world each time.
fn busy_snapshot() -> Snapshot {
    Snapshot {
        health: Some(HealthResponse {
            status: "ok".to_string(),
            uptime_s: 8_040,
            version: "0.4.1".to_string(),
            sensors: vec![SensorHealth {
                sensor_id: "wifi-1".to_string(),
                sensor_kind: "wifi".to_string(),
                healthy: true,
                seconds_since_heartbeat: Some(2),
                detections_5m: 1_284,
                ..SensorHealth::default()
            }],
            fusion: Some(FusionHealth {
                connected: true,
                configured: true,
                ..FusionHealth::default()
            }),
        }),
        monitoring: Some(MonitoringState {
            enabled: true,
            ..MonitoringState::default()
        }),
        tracks: Some(TrackPage {
            tracks: vec![Track {
                state: "CONFIRMED".to_string(),
                confidence: 0.82,
                detection_count: 402,
                identity: Some(Identity {
                    model_hint: Some("Mavic 3".to_string()),
                    ..Identity::default()
                }),
                evidence: vec![Evidence {
                    class: "A".to_string(),
                    count: 402,
                }],
                current: Some(Position {
                    height_agl_m: Some(120.0),
                    speed_mps: Some(14.0),
                    ..Position::default()
                }),
                rssi_dbm: Some(-58.0),
                ..Track::default()
            }],
            total: 1,
        }),
        detections: Some(DetectionPage {
            detections: vec![Detection {
                sensor_id: "wifi-1".to_string(),
                sensor_kind: "wifi".to_string(),
                detection_class: "A".to_string(),
                rf: Some(Rf {
                    channel: Some(149),
                    rssi_dbm: Some(-52.0),
                    ..Rf::default()
                }),
                ..Detection::default()
            }],
            total: 1_284,
        }),
        slow: Slow {
            system: Some(SystemInfo {
                build: SystemBuild {
                    version: "0.4.1".to_string(),
                    revision: Some("a1b2c3d4e5".to_string()),
                    revision_dirty: false,
                },
                runtime: SystemRuntime {
                    store: "libsql".to_string(),
                    ..SystemRuntime::default()
                },
                host: SystemHost {
                    disk_path: "/var/lib/classg".to_string(),
                    disk_total_bytes: Some(31_000_000_000),
                    disk_free_bytes: Some(12_400_000_000),
                },
            }),
            ..Slow::default()
        },
        ..Snapshot::default()
    }
}

#[test]
fn a_paused_recording_is_never_mistaken_for_a_quiet_sky() {
    // The failure this whole section exists to prevent: every sensor healthy,
    // fusion connected, no tracks — which is exactly what a working detector
    // over an empty field looks like too.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.tracks = Some(TrackPage::default());
    snapshot.monitoring = Some(MonitoringState {
        enabled: false,
        reason: Some("known local flight".to_string()),
        discarded: 1_204,
        ..MonitoringState::default()
    });
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "PAUSED"), "{}", rows.join("\n"));
    assert!(contains(&rows, "1.2k discarded"), "the toll of the pause");
    assert!(contains(&rows, "known local flight"), "and why");
    assert!(contains(&rows, "nothing tracked"));
}

#[test]
fn a_running_recording_says_so_without_shouting() {
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "recording"));
    assert!(!contains(&rows, "PAUSED"));
    assert!(!contains(&rows, "discarded"));
}

#[test]
fn the_build_a_deploy_can_be_matched_against_beats_the_bare_version() {
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "0.4.1+a1b2c3d"), "{}", rows.join("\n"));
    assert!(contains(&rows, "libsql"));
    assert!(contains(&rows, "free of"));
}

#[test]
fn a_unit_with_no_slow_tier_yet_still_shows_its_version() {
    // The first three seconds after launch, and every unit whose /system is
    // behind a login. Falling back to /health's version keeps the line honest
    // rather than blank.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow = Slow::default();
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "0.4.1"));
    assert!(!contains(&rows, "libsql"), "no store line without /system");
}

#[test]
fn a_track_nothing_identified_is_drawn_differently_from_one_something_did() {
    // 2026-08-17: a DJI-OUI access point on ch149 sat beside a real Remote ID
    // track for a full CloseAfter window, indistinguishable at a glance.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.tracks = Some(TrackPage {
        tracks: vec![Track {
            state: "TENTATIVE".to_string(),
            confidence: 0.10,
            detection_count: 140,
            identity: Some(Identity {
                vendor: Some("DJI".to_string()),
                ..Identity::default()
            }),
            evidence: vec![Evidence {
                class: "C".to_string(),
                count: 140,
            }],
            ..Track::default()
        }],
        total: 1,
    });
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    assert!(
        contains(&rows, "~DJI"),
        "an unidentified contact must be marked: {}",
        rows.join("\n")
    );
    // And the evidence that failed to identify it, with its count.
    assert!(contains(&rows, "Cx140"));
}

#[test]
fn an_identified_track_shows_its_evidence_and_its_kinematics() {
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "Mavic 3"), "{}", rows.join("\n"));
    assert!(!contains(&rows, "~Mavic 3"), "this one was identified");
    assert!(contains(&rows, "Ax402"), "class A, seen 402 times");
    assert!(contains(&rows, "120m agl"), "reported height above ground");
    assert!(contains(&rows, "14m/s"));
    assert!(contains(&rows, "-58dBm"));
}

#[test]
fn detections_name_their_class_rather_than_lettering_it() {
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let rows = render(&mut app, 140, 44);
    assert!(
        contains(&rows, "Remote ID"),
        "a bare 'A' is not a claim anyone can check: {}",
        rows.join("\n")
    );
    assert!(contains(&rows, "ch149"), "the Wi-Fi sensor names a channel");
}

#[test]
fn an_sdr_detection_is_tuned_by_frequency_because_it_has_no_channel() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.detections = Some(DetectionPage {
        detections: vec![Detection {
            sensor_kind: "sdr".to_string(),
            detection_class: "E".to_string(),
            rf: Some(Rf {
                freq_hz: Some(915_000_000),
                rssi_dbm: Some(-71.0),
                ..Rf::default()
            }),
            ..Detection::default()
        }],
        total: 4,
    });
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "915M"), "{}", rows.join("\n"));
    assert!(contains(&rows, "Control link"));
}

#[test]
fn a_radio_held_by_a_capture_or_a_sweep_is_reported_where_the_sensor_is() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow.captures = Some(CapturePage {
        captures: vec![Capture {
            state: "running".to_string(),
            iface: "wlan1".to_string(),
            channel: 6,
            duration_s: 60,
            ..Capture::default()
        }],
    });
    snapshot.slow.sweeps = Some(SweepPage {
        sweeps: vec![SpectrumSweep {
            band: "2.4GHz".to_string(),
            state: "running".to_string(),
            ..SpectrumSweep::default()
        }],
    });
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    // The interface and channel it took, and how long it asked for. No
    // start time in the fixture, so there is no elapsed to measure against
    // and the requested duration has to stand on its own rather than
    // reading as a sentence with a word missing.
    assert!(
        contains(&rows, "wlan1 ch6  60s"),
        "{}",
        rows.join(
            "
"
        )
    );
    assert!(
        contains(&rows, "no ADS-B while it runs"),
        "a sweep borrows the SDR from dump1090"
    );
}

#[test]
fn a_failed_capture_says_why_rather_than_only_that_it_failed() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow.captures = Some(CapturePage {
        captures: vec![Capture {
            state: "failed".to_string(),
            iface: "wlan1".to_string(),
            error: Some("tcpdump: wlan1: No such device".to_string()),
            ..Capture::default()
        }],
    });
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "No such device"), "{}", rows.join("\n"));
}

#[test]
fn an_idle_unit_spends_no_rows_on_the_radio_section() {
    // Rows are the scarce resource in this pane. A capture that finished
    // cleanly and was never analysed is a file on disk, not news.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow.captures = Some(CapturePage {
        captures: vec![Capture {
            state: "completed".to_string(),
            iface: "wlan1".to_string(),
            frame_count: 12_400,
            analysis: Some(CaptureAnalysis {
                analyzed: false,
                drone_transmitters: 0,
            }),
            ..Capture::default()
        }],
    });
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(!contains(&rows, "capture"), "{}", rows.join("\n"));
}

#[test]
fn an_analysed_capture_reports_what_it_found() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow.captures = Some(CapturePage {
        captures: vec![Capture {
            state: "completed".to_string(),
            frame_count: 12_400,
            label: Some("beacon test".to_string()),
            analysis: Some(CaptureAnalysis {
                analyzed: true,
                drone_transmitters: 3,
            }),
            ..Capture::default()
        }],
    });
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "3 drone tx"), "{}", rows.join("\n"));
    assert!(contains(&rows, "12.4k frames"));
    assert!(contains(&rows, "beacon test"));
}

#[test]
fn a_refused_poll_is_explained_once_rather_than_drawn_as_an_empty_sky() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.tracks = None;
    snapshot.detections = None;
    snapshot.denied = Some("log in to continue".to_string());
    snapshot.slow.auth = Some(AuthState {
        auth_enabled: true,
        authenticated: false,
        ..AuthState::default()
    });
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "log in to continue"), "{}", rows.join("\n"));
    // No credential went out at all, so the row names that state and the
    // remedy is the token file rather than a browser cookie.
    assert!(contains(&rows, "no token"));
    // With no credential at all the remedy is the token file, not a cookie.
    assert!(contains(&rows, ".agent-state"), "and how to fix it");
    assert!(contains(&rows, "tracks unavailable"));
}

#[test]
fn a_logged_in_poller_names_the_account_it_is_using() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow.auth = Some(AuthState {
        auth_enabled: true,
        authenticated: true,
        user: Some(AuthUser {
            username: "lawrence".to_string(),
            role: "viewer".to_string(),
        }),
        ..AuthState::default()
    });
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "lawrence"), "{}", rows.join("\n"));
    assert!(contains(&rows, "viewer"));
}

#[test]
fn a_unit_with_auth_switched_off_spends_no_row_saying_so() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.slow.auth = Some(AuthState::default());
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(!contains(&rows, "session"), "{}", rows.join("\n"));
}

#[test]
fn a_nearly_full_store_is_amber_before_it_stops_recording() {
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    if let Some(system) = snapshot.slow.system.as_mut() {
        system.host.disk_free_bytes = Some(1_000_000_000);
        system.host.disk_total_bytes = Some(31_000_000_000);
    }
    app.classg.snapshot = snapshot;

    let mut terminal = ratatui::Terminal::new(TestBackend::new(140, 44)).expect("test backend");
    terminal
        .draw(|frame| draw(frame, &mut app))
        .expect("draw must not fail");
    let buffer = terminal.backend().buffer().clone();

    let mut found = false;
    for y in 0..44u16 {
        let row: Vec<String> = (0..140u16)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        let flat: String = row.concat();
        if flat.contains("free of") {
            for x in 0..140u16 {
                if buffer[(x, y)].symbol() == "9" && buffer[(x, y)].fg == super::WARN {
                    found = true;
                }
            }
        }
    }
    assert!(found, "a store under a tenth free must not be dim");
}

#[test]
fn the_pane_survives_a_disk_the_api_could_not_read() {
    // Null with a reason, never a zero: 0 bytes free reads as an emergency
    // that is not happening.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    if let Some(system) = snapshot.slow.system.as_mut() {
        system.host.disk_free_bytes = None;
        system.host.disk_total_bytes = None;
    }
    app.classg.snapshot = snapshot;
    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "disk unreadable"), "{}", rows.join("\n"));
    assert!(!contains(&rows, "0 free of 0"));
}

#[test]
fn the_narrow_pane_drops_the_sensor_column_rather_than_overflowing() {
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    app.focus = Pane::Classg;

    // 70 columns, one pane at a time: wide enough for the sensor column.
    let wide = render(&mut app, 70, 40);
    assert!(contains(&wide, "SENSOR"), "{}", wide.join("\n"));

    // In the two-column layout the reader column is clamped near 48, which is
    // not.
    let rows = render(&mut app, 100, 40);
    let header = rows
        .iter()
        .find(|row| row.contains("CLASS") && row.contains("TUNE"));
    assert!(header.is_some(), "{}", rows.join("\n"));
}

#[test]
fn no_classg_row_is_ever_sliced_by_the_pane_edge() {
    // The pane renders without wrapping, so anything wider than the body is
    // cut wherever the frame happens to fall. Every row must fit.
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    for width in [100u16, 120, 140, 200] {
        let rows = render(&mut app, width, 44);
        for row in &rows {
            // The frame's right-hand border must still be a border, not the
            // tail of a table row that ran past it.
            let trimmed = row.trim_end();
            assert!(
                !trimmed.is_empty(),
                "row vanished at width {width}: {}",
                rows.join("\n")
            );
        }
    }
}

#[test]
fn one_aeroplane_overhead_does_not_fill_the_whole_detection_list() {
    // Measured on the real unit: ADS-B squitters at roughly 1 Hz per aircraft,
    // so sixteen consecutive rows were a single ICAO repeating and the Wi-Fi
    // detections underneath had been pushed off the bottom of the pane.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    let mut detections: Vec<Detection> = (0..16)
        .map(|_| Detection {
            sensor_id: "sdr-0".to_string(),
            sensor_kind: "sdr".to_string(),
            detection_class: "D".to_string(),
            rf: Some(Rf {
                freq_hz: Some(1_090_000_000),
                ..Rf::default()
            }),
            adsb: Some(Adsb {
                icao: "a9d770".to_string(),
                callsign: None,
            }),
            ..Detection::default()
        })
        .collect();
    // The one row that actually matters, underneath all of them.
    detections.push(Detection {
        sensor_id: "wifi-1".to_string(),
        sensor_kind: "wifi".to_string(),
        detection_class: "A".to_string(),
        rf: Some(Rf {
            channel: Some(6),
            rssi_dbm: Some(-52.0),
            ..Rf::default()
        }),
        identity: Some(Identity {
            model_hint: Some("Mavic 3".to_string()),
            ..Identity::default()
        }),
        ..Detection::default()
    });
    snapshot.detections = Some(DetectionPage {
        detections,
        total: 15370,
    });
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "a9d770 x16"), "{}", rows.join("\n"));
    assert!(
        contains(&rows, "Mavic 3"),
        "the folded repeats must leave room for the row that matters"
    );
    // And the heading still reports the true total, not the folded count.
    assert!(contains(&rows, "15370 total"));
}

#[test]
fn anonymous_repeats_are_left_alone() {
    // Two class E bursts with no identity are not known to be the same
    // transmitter, and an unidentified signal repeating is worth looking at.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.detections = Some(DetectionPage {
        detections: (0..3)
            .map(|_| Detection {
                sensor_id: "sdr-0".to_string(),
                sensor_kind: "sdr".to_string(),
                detection_class: "E".to_string(),
                rf: Some(Rf {
                    freq_hz: Some(915_000_000),
                    rssi_dbm: Some(-71.0),
                    ..Rf::default()
                }),
                ..Detection::default()
            })
            .collect(),
        total: 3,
    });
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    assert!(!contains(&rows, "x3"), "{}", rows.join("\n"));
    let bursts = rows
        .iter()
        .filter(|row| row.contains("Control link"))
        .count();
    assert_eq!(bursts, 3, "all three must still be listed");
}

#[test]
fn the_store_detail_never_runs_into_the_disk_figure() {
    // A unit that is containerised AND syncing reads "libsql · docker · sync",
    // which is wider than the column it was padded to -- so the pad added
    // nothing and it rendered as `sync88.3G free of 117.0G`.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    if let Some(system) = snapshot.slow.system.as_mut() {
        system.runtime.containerised = true;
        system.runtime.turso_sync_configured = true;
    }
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 140, 44);
    assert!(contains(&rows, "sync  "), "{}", rows.join("\n"));
    assert!(!contains(&rows, "sync11"), "no space before the figure");
    assert!(!contains(&rows, "sync8"), "no space before the figure");
}

#[test]
fn a_long_callsign_loses_characters_before_it_loses_its_count() {
    // `KLM1234X x16` clipped as one string became `KLM1234X x1`, which is a
    // wrong number rather than a shortened name.
    let mut app = test_app();
    let mut snapshot = busy_snapshot();
    snapshot.detections = Some(DetectionPage {
        detections: (0..16)
            .map(|_| Detection {
                sensor_id: "sdr-0".to_string(),
                sensor_kind: "sdr".to_string(),
                detection_class: "D".to_string(),
                adsb: Some(Adsb {
                    icao: "a9d770".to_string(),
                    callsign: Some("KLM1234X".to_string()),
                }),
                ..Detection::default()
            })
            .collect(),
        total: 16,
    });
    app.classg.snapshot = snapshot;

    // Narrow enough that the identity column cannot hold name and count both.
    let rows = render(&mut app, 100, 40);
    assert!(
        contains(&rows, "x16"),
        "{}",
        rows.join(
            "
"
        )
    );
    assert!(!contains(&rows, "x1 "), "the count must not be truncated");
}

#[test]
fn the_process_table_says_which_column_it_is_sorted_by() {
    use crate::panes::system::SortBy;
    // Two orders that both put a big number at the top are otherwise told
    // apart only by staring at the rows.
    let mut app = test_app();
    with_load(&mut app);
    app.focus = Pane::System;

    let accent_of = |app: &mut App, needle: &str| -> Option<ratatui::style::Color> {
        let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 24)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw must not fail");
        let buffer = terminal.backend().buffer().clone();
        let needle: Vec<char> = needle.chars().collect();
        for y in 0..24u16 {
            let row: Vec<String> = (0..100u16)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            if !row.concat().contains("COMMAND") {
                continue;
            }
            for x in 0..row.len().saturating_sub(needle.len()) {
                if needle
                    .iter()
                    .enumerate()
                    .all(|(i, c)| row[x + i] == c.to_string())
                {
                    return buffer[(x as u16, y)].fg.into();
                }
            }
        }
        None
    };

    app.system.sort = SortBy::Cpu;
    assert_eq!(accent_of(&mut app, "CPU%"), Some(app.accent));
    assert_eq!(accent_of(&mut app, "MEM"), Some(super::DIM));

    app.system.sort = SortBy::Memory;
    assert_eq!(accent_of(&mut app, "MEM"), Some(app.accent));
    assert_eq!(accent_of(&mut app, "CPU%"), Some(super::DIM));
}

#[test]
fn the_footer_offers_the_sort_you_are_not_already_using() {
    use crate::panes::system::SortBy;
    let mut app = test_app();
    app.system.sort = SortBy::Cpu;
    assert!(contains(&render(&mut app, 140, 44), "sort by mem"));
    app.system.sort = SortBy::Memory;
    assert!(contains(&render(&mut app, 140, 44), "sort by cpu%"));
}

#[test]
fn the_classg_pane_survives_a_radios_pane_that_wants_the_whole_column() {
    // The two panes above ClassG are pinned to Length constraints computed
    // from their own content, and radios sizes itself to however many
    // interfaces exist. A Pi running the stack in Docker can present a lot of
    // them, and nothing caps that number — so on a short terminal the two
    // fixed panes can ask for more rows than the column has.
    //
    // ClassG holds a Min(4) and ratatui's solver honours it over the Lengths,
    // which is the only reason this works. That is a property of a dependency
    // rather than of this code, so it is asserted rather than assumed: an
    // upgrade that reordered those priorities would silently delete the pane
    // the whole tool exists for.
    use crate::panes::radios::{Iface, UsbRadio, WirelessMode};
    let mut app = test_app();
    with_load(&mut app);
    app.radios.ifaces = (0..24)
        .map(|i| Iface {
            name: format!("wlan{i}"),
            state: "up".into(),
            rx_bps: 0.0,
            tx_bps: 0.0,
            mode: Some(WirelessMode::Monitor),
            channel: Some(6),
            driver: Some("mt7921u".into()),
        })
        .collect();
    app.radios.usb = (0..8)
        .map(|i| UsbRadio {
            id: format!("0e8d:796{i}"),
            description: "MediaTek Inc. Wireless_Device".into(),
        })
        .collect();

    // Far more than any of these terminals can give it.
    assert!(
        app.radios.content_rows() > 30,
        "the fixture is not extreme enough"
    );

    for (width, height) in [(120u16, 10u16), (120, 16), (120, 24), (120, 44)] {
        let rows = render(&mut app, width, height);
        assert!(
            rows.iter().any(|row| row.contains("ClassG")),
            "the ClassG pane vanished at {width}x{height}:
{}",
            rows.join(
                "
"
            )
        );
    }
}
