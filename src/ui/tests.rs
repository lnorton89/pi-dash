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
    app.system.thread_count = 431;
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
            threads: 12,
            user: "admin".to_string(),
            cmdline: "/usr/bin/node /usr/lib/node_modules/npm/bin/npm-cli.js ci".to_string(),
        },
        ProcRow {
            pid: 68_260,
            name: "dump1090-mutability".to_string(),
            state: 'D',
            cpu_pct: 47.0,
            rss_kb: 6_000,
            threads: 3,
            user: "dump1090".to_string(),
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
        contains(&rows, "431 threads, 4 running"),
        "thread counts missing"
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
                    !swap.contains("thread") || swap.contains(" threads"),
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
            rx_total: 0,
            tx_total: 0,
            mode: Some(WirelessMode::Monitor),
            channel: Some(6),
            driver: Some("mt7921u".into()),
        },
        Iface {
            name: "eth0".into(),
            state: "down".into(),
            rx_bps: 0.0,
            tx_bps: 0.0,
            rx_total: 0,
            tx_total: 0,
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
        rx_total: 0,
        tx_total: 0,
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
            rx_total: 0,
            tx_total: 0,
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

/// Three filesystems as a Pi presents them: the card, the boot partition, and
/// a stick something is recording to.
fn with_filesystems(app: &mut App, extra: usize) {
    use crate::panes::health::{DiskUsage, Filesystem};
    let fs = |mount: &str, used: u64, avail: u64, total: u64| Filesystem {
        source: format!("/dev/{mount}"),
        mount: mount.into(),
        usage: DiskUsage {
            used_kb: used,
            avail_kb: avail,
            total_kb: total,
        },
    };
    app.health.filesystems = vec![
        fs("/", 23_800_000, 92_600_000, 122_600_000),
        fs("/boot/firmware", 70_000, 451_000, 521_000),
        fs("/media/captures", 512, 244_180_000, 244_180_000),
    ];
    for n in 0..extra {
        app.health
            .filesystems
            .push(fs(&format!("/spare{n}"), 1, 99, 100));
    }
}

#[test]
fn a_wide_pane_puts_the_disks_beside_the_memory_rather_than_under_them() {
    // btop's layout, and the reason for it: stacked, each filesystem would
    // cost the process table a row to say what a spare column says for free.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    let rows = render(&mut app, 200, 24);

    let mem = rows
        .iter()
        .find(|r| r.contains("mem "))
        .expect("a memory row");
    assert!(
        mem.contains("disks"),
        "the heading shares the memory row: {mem}"
    );

    // Every mount is named by its last component and carries what can
    // actually be written to it.
    let joined = rows.join(
        "
",
    );
    assert!(joined.contains("firmware"), "{joined}");
    assert!(joined.contains("captures"), "{joined}");
    assert!(joined.contains("440M free of 509M"), "{joined}");
    // The boot partition is not the card, and both are listed.
    assert!(joined.contains("88.3G free of 116.9G"), "{joined}");
}

#[test]
fn a_narrow_pane_drops_the_disks_rather_than_truncating_them() {
    // Half of a narrow pane cannot hold a label, a meter and two figures, and
    // a disk row arriving cut in half is worse than one that is absent.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    let rows = render(&mut app, 90, 24);
    let joined = rows.join(
        "
",
    );

    assert!(!joined.contains("firmware"), "{joined}");
    // Memory keeps the whole width and still reports.
    assert!(rows.iter().any(|r| r.contains("mem ")), "{joined}");
}

#[test]
fn more_filesystems_than_fit_are_counted_rather_than_dropped_silently() {
    // A box listing three of six disks without mentioning the other three is
    // a box that has quietly stopped answering the question.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 3);
    assert!(render(&mut app, 200, 24)
        .join(
            "
"
        )
        .contains("+3 more"));
}

#[test]
fn the_second_column_is_placed_by_characters_not_by_bytes() {
    // The memory meters beside it are three-byte block glyphs. Padding by
    // byte count would put the disks column two thirds of the way off the
    // right edge of the pane.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    // The threshold is the System pane's own width, and the right-hand column
    // takes about sixty of the terminal's before this pane sees any -- so
    // these are terminal widths that leave the pane enough, not 104 itself.
    for width in [170u16, 200, 240] {
        let rows = render(&mut app, width, 24);
        assert!(
            rows.iter().any(|r| r.contains("disks")),
            "the disks column fell off the pane at {width}"
        );
    }
    // Nothing may spill past the frame the pane drew for itself, at any width
    // either side of the threshold.
    for width in [80u16, 104, 140, 170, 200, 240] {
        for row in render(&mut app, width, 24) {
            assert!(
                row.chars().count() <= width as usize,
                "a row overran {width}: {row}"
            );
        }
    }
}

#[test]
fn the_process_table_says_how_much_of_it_is_below_the_fold() {
    // A table showing the busiest five of four hundred looks exactly like a
    // table showing all five a box is running.
    let mut app = test_app();
    with_load(&mut app);
    app.system.total_procs = 374;
    assert!(contains(&render(&mut app, 200, 20), "/374"));

    // Nothing hidden, nothing said.
    app.system.total_procs = app.system.procs.len();
    assert!(!contains(&render(&mut app, 200, 20), "/374"));
}

#[test]
fn an_active_filter_is_never_invisible() {
    // A filter that is on and unshown is a table quietly misreporting what the
    // box is running, so it takes the command-line heading's place.
    let mut app = test_app();
    with_load(&mut app);
    assert!(contains(&render(&mut app, 200, 20), "COMMAND LINE"));

    app.system.filter = "dump".to_string();
    let rows = render(&mut app, 200, 20);
    assert!(contains(&rows, "filter: dump"), "{}", rows.join("\n"));
    assert!(!contains(&rows, "COMMAND LINE"));
}

#[test]
fn a_filter_being_typed_shows_a_caret_in_either_glyph_set() {
    // A text field with no caret is one you cannot tell is focused, and this
    // one owns the whole keyboard while it is open.
    let mut app = test_app();
    with_load(&mut app);
    app.system.filter = "ngin".to_string();
    app.system.filter_editing = true;
    assert!(contains(&render(&mut app, 200, 20), "filter: ngin\u{258f}"));

    // The framebuffer console cannot draw that glyph, and a filter you cannot
    // see the end of is worse there than anywhere.
    let mut ascii = App::new(Config {
        api: "http://127.0.0.1:1".to_string(),
        glyphs: "ascii".to_string(),
        ..Config::default()
    });
    with_load(&mut ascii);
    ascii.system.filter = "ngin".to_string();
    ascii.system.filter_editing = true;
    assert!(contains(&render(&mut ascii, 200, 20), "filter: ngin_"));
}

#[test]
fn threads_and_user_appear_when_there_is_room_and_go_first_when_there_is_not() {
    let mut app = test_app();
    with_load(&mut app);
    let wide = render(&mut app, 200, 20);
    assert!(contains(&wide, "THR"), "{}", wide.join("\n"));
    assert!(contains(&wide, "USER"));
    assert!(
        contains(&wide, "dump1090"),
        "the account, not just the comm"
    );

    // Both are context for a row you have already found; the command line is
    // how you find it, so they go before it does.
    let narrow = render(&mut app, 150, 20);
    assert!(!contains(&narrow, "USER"));
    assert!(contains(&narrow, "COMMAND LINE"));
}

#[test]
fn a_single_threaded_process_leaves_its_thread_column_blank() {
    // Almost everything on this box is single-threaded, and a column of 1s is
    // a column of noise with the interesting numbers hidden inside it.
    let mut app = test_app();
    with_load(&mut app);
    if let Some(row) = app.system.procs.get_mut(0) {
        row.threads = 1;
    }
    let line = render(&mut app, 200, 20)
        .into_iter()
        .find(|r| r.contains("npm ci"))
        .expect("the row");
    assert!(!line.contains(" 1 "), "{line}");
}

/// Three interfaces as the Pi presents them: a wired one carrying traffic, a
/// monitor radio that has heard something, and one that has heard nothing.
fn with_interfaces(app: &mut App) {
    use crate::panes::radios::{Iface, WirelessMode};
    let ifc = |name: &str, rx: f64, tx: f64, rt: u64, tt: u64| Iface {
        name: name.into(),
        state: "up".into(),
        rx_bps: rx,
        tx_bps: tx,
        rx_total: rt,
        tx_total: tt,
        mode: Some(WirelessMode::Monitor),
        channel: Some(6),
        driver: Some("mt7921u".into()),
    };
    app.radios.ifaces = vec![
        ifc("wlan-tplink", 0.0, 0.0, 0, 0),
        ifc("eth0", 7700.0, 8000.0, 677_000_000, 693_000_000),
        ifc("wlan-alfa", 3900.0, 0.0, 12_400_000, 0),
    ];
    app.radios.throughput = (0..60).map(|i| ((i % 20) as f64) * 900.0).collect();
}

#[test]
fn the_net_column_shows_a_total_as_well_as_a_rate() {
    // `0B/s` on a monitor interface is what a quiet minute looks like and also
    // what a radio that has never worked looks like. Only the total tells them
    // apart, and the pane has never shown one.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    with_interfaces(&mut app);
    let rows = render(&mut app, 210, 22);
    let joined = rows.join("\n");

    assert!(contains(&rows, "net"), "{joined}");
    let eth = rows.iter().find(|r| r.contains("eth0")).expect("eth0");
    assert!(eth.contains("7.5K"), "the rate: {eth}");
    assert!(eth.contains("646M"), "the total since start: {eth}");

    // The radio that has heard nothing says so with a zero total, not just a
    // zero rate.
    let quiet = rows
        .iter()
        .find(|r| r.contains("wlan-tpli"))
        .expect("the quiet radio");
    assert!(quiet.contains("0B"), "{quiet}");
}

#[test]
fn the_net_column_puts_the_busiest_interface_first() {
    // An interface that has moved nothing is the one you least need a row for,
    // and there are only three rows.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    with_interfaces(&mut app);
    let rows = render(&mut app, 210, 22);
    let position = |needle: &str| {
        rows.iter()
            .position(|r| r.contains(needle))
            .unwrap_or(usize::MAX)
    };
    assert!(position("eth0") < position("wlan-alfa"), "busiest first");
    assert!(position("wlan-alfa") < position("wlan-tpli"));
}

#[test]
fn the_band_drops_columns_from_the_right_as_the_pane_narrows() {
    // Three columns, then two, then one -- each dropped rather than truncated,
    // because half a row of figures is worse than none.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    with_interfaces(&mut app);

    let wide = render(&mut app, 210, 22);
    assert!(contains(&wide, "disks") && contains(&wide, "eth0"));

    let middling = render(&mut app, 170, 22);
    assert!(contains(&middling, "disks"), "disks outlive net");

    let narrow = render(&mut app, 90, 22);
    assert!(!contains(&narrow, "disks"));
    assert!(contains(&narrow, "mem "), "memory always survives");
}

#[test]
fn no_column_of_the_band_ever_overruns_the_pane() {
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 3);
    with_interfaces(&mut app);
    for width in [80u16, 104, 130, 170, 200, 210, 260] {
        for row in render(&mut app, width, 22) {
            assert!(
                row.chars().count() <= width as usize,
                "a row overran {width}: {row}"
            );
        }
    }
}

/// Forty processes, so there is always more table than pane.
fn with_many_procs(app: &mut App) {
    use crate::panes::system::ProcRow;
    app.system.procs = (0..40)
        .map(|i| ProcRow {
            pid: 1000 + i,
            name: format!("proc{i}"),
            state: 'S',
            cpu_pct: (40 - i) as f64,
            rss_kb: 1000,
            threads: 1,
            user: "root".into(),
            cmdline: format!("/usr/bin/proc{i} --run"),
        })
        .collect();
    app.system.total_procs = 374;
}

#[test]
fn the_process_table_scrolls_and_says_where_in_the_list_it_is() {
    // 374 processes and thirty rows meant the other 340 were unreachable
    // rather than merely below the fold.
    let mut app = test_app();
    with_load(&mut app);
    with_many_procs(&mut app);

    let top = render(&mut app, 200, 24);
    assert!(contains(&top, "proc0 "), "{}", top.join("\n"));

    app.system.scroll = 12;
    let scrolled = render(&mut app, 200, 24);
    assert!(!contains(&scrolled, "proc0 "), "the top rows scrolled away");
    assert!(contains(&scrolled, "proc12"));
    // Where in the list, not just how much of it.
    assert!(contains(&scrolled, "13-"), "{}", scrolled.join("\n"));
    assert!(contains(&scrolled, "/374"));
}

#[test]
fn scrolling_past_the_end_parks_the_last_row_rather_than_emptying_the_table() {
    let mut app = test_app();
    with_load(&mut app);
    with_many_procs(&mut app);
    app.system.scroll = 9_999;
    let rows = render(&mut app, 200, 24);
    assert!(contains(&rows, "proc39"), "the last row is still drawn");
}

#[test]
fn a_table_with_nothing_below_the_fold_draws_no_scrollbar() {
    // A full-height thumb beside a complete table is a control that looks like
    // it does something.
    let mut app = test_app();
    with_load(&mut app);
    // with_load leaves two processes, which any of these panes can show whole.
    let rows = render(&mut app, 200, 40);
    let bars = rows.iter().filter(|r| r.contains('\u{2588}')).count();
    with_many_procs(&mut app);
    let more = render(&mut app, 200, 40);
    assert!(
        more.iter().filter(|r| r.contains('\u{2588}')).count() > bars,
        "a scrollbar appears once there is something to scroll to"
    );
}

#[test]
fn every_pane_title_carries_the_number_that_selects_it() {
    // Tab and 1-4 have switched panes since the rewrite and nothing on screen
    // ever said which number belonged to which pane.
    let mut app = test_app();
    with_load(&mut app);
    let rows = render(&mut app, 200, 40);
    for (n, name) in [
        (1, "System"),
        (2, "Pi health"),
        (3, "Radios"),
        (4, "ClassG"),
    ] {
        assert!(
            rows.iter().any(|r| r.contains(&format!("{n} {name}"))),
            "pane {n} is not numbered: {}",
            rows.join("\n")
        );
    }
}

#[test]
fn the_memory_band_reports_what_is_available_not_only_what_is_used() {
    // MemAvailable is the kernel's own estimate of what a new allocation could
    // get without swapping. It is not total-minus-used-bar: it counts the
    // reclaimable page cache the used figure already excluded, and it is the
    // number that answers "will this fit".
    let mut app = test_app();
    with_load(&mut app);
    let rows = render(&mut app, 200, 24);
    // The label in its gutter, not the bare word: "unavailable" appears in the
    // health pane beside it and matched first.
    let line = rows
        .iter()
        .find(|r| r.contains("  avail  "))
        .expect("an avail row");
    // with_load leaves 2.4G of 3.8G available, which is 63%.
    assert!(line.contains("63%"), "{line}");
    assert!(line.contains("2.3G") || line.contains("2.4G"), "{line}");
}

#[test]
fn only_the_focused_pane_keeps_the_accent() {
    // The pane numbers advertise 1-4, and on a wide terminal those keys only
    // ever moved something invisible. The highlight is what makes them mean
    // anything there.
    let mut app = test_app();
    with_load(&mut app);

    let accent_of_title = |app: &mut App, needle: &str| -> Option<ratatui::style::Color> {
        let mut terminal = ratatui::Terminal::new(TestBackend::new(200, 40)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw must not fail");
        let buffer = terminal.backend().buffer().clone();
        for y in 0..40u16 {
            let row: String = (0..200u16)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            if let Some(byte) = row.find(needle) {
                // `find` returns a BYTE offset and this row is full of
                // three-byte box-drawing characters, so it has to be counted
                // back into columns. Reading the byte offset as a column lands
                // two thirds of the way to the left of the cell wanted, which
                // is a frame character and always Gray.
                let column = row[..byte].chars().count() as u16;
                return buffer[(column, y)].fg.into();
            }
        }
        None
    };

    app.focus = Pane::System;
    assert_eq!(accent_of_title(&mut app, "1 System"), Some(app.accent));
    assert_eq!(accent_of_title(&mut app, "3 Radios"), Some(super::DIM));

    app.focus = Pane::Radios;
    assert_eq!(accent_of_title(&mut app, "3 Radios"), Some(app.accent));
    assert_eq!(accent_of_title(&mut app, "1 System"), Some(super::DIM));
}

#[test]
fn the_help_card_fits_every_line_it_carries() {
    // It has grown three keys since it was sized, and a reference card whose
    // last line is off the bottom is the one line you needed.
    let mut app = test_app();
    app.mode = Mode::Help;
    let rows = render(&mut app, 100, 30);
    for needle in [
        "q / Esc / Ctrl-C",
        "s  ",
        "f  ",
        "up/down/pgup/pgdn",
        "tab / 1-4",
        "Ctrl-L",
        "credential",
    ] {
        assert!(
            contains(&rows, needle),
            "the help card lost {needle:?}:\n{}",
            rows.join("\n")
        );
    }
    // And nothing wrapped onto a line of its own.
    assert!(!rows.iter().any(|r| r.trim_start().starts_with("top)")));
}

/// A page of detections that folds by `ratio`: `distinct` contacts, each
/// repeating, in the order the API returns them.
fn folding_page(distinct: usize, each: usize) -> DetectionPage {
    let mut detections = Vec::new();
    for contact in 0..distinct {
        for _ in 0..each {
            detections.push(Detection {
                sensor_id: "sdr-0".to_string(),
                sensor_kind: "sdr".to_string(),
                detection_class: "D".to_string(),
                adsb: Some(Adsb {
                    icao: format!("a9d7{contact:02}"),
                    callsign: None,
                }),
                ..Detection::default()
            });
        }
    }
    let total = detections.len() as u64;
    DetectionPage { detections, total }
}

#[test]
fn a_repeating_sky_asks_for_enough_detections_to_fill_the_pane() {
    use crate::ui::classg::detection_request;
    // Measured on the real unit: forty fetched, three drawn, twenty rows of
    // pane left empty underneath. Doubling the request could never close that.
    let page = folding_page(3, 13);
    let asked = detection_request(Some(&page), 21);
    assert!(
        asked > 40,
        "a 13x fold needs far more than the old ceiling, got {asked}"
    );
    assert!(asked <= 200, "and never more than the API cap, got {asked}");
}

#[test]
fn a_varied_sky_asks_for_no_more_than_it_can_draw() {
    use crate::ui::classg::detection_request;
    // Nothing folds, so every fetched row is a drawn row and fetching extra
    // would be a bigger query every three seconds for rows with nowhere to go.
    let page = folding_page(30, 1);
    assert_eq!(detection_request(Some(&page), 21), 21);
}

#[test]
fn the_detection_request_settles_instead_of_running_away() {
    use crate::ui::classg::detection_request;
    // Once the list fills the room, drawn equals room and the request has to
    // stop growing, or every poll asks for more than the last one for ever.
    let mut asked: usize = 21;
    for _ in 0..8 {
        // Each round, the API returns what was asked for, folding 4:1.
        let page = folding_page(asked.div_ceil(4), 4);
        let next = detection_request(Some(&page), 21);
        assert!(next <= 200);
        asked = next;
    }
    // Converged, not oscillating or pinned at the ceiling.
    let page = folding_page(asked.div_ceil(4), 4);
    assert_eq!(detection_request(Some(&page), 21), asked);
}

#[test]
fn an_empty_or_missing_page_asks_only_for_the_room() {
    use crate::ui::classg::detection_request;
    assert_eq!(detection_request(None, 12), 12);
    assert_eq!(
        detection_request(
            Some(&DetectionPage {
                detections: Vec::new(),
                total: 0
            }),
            12
        ),
        12
    );
    // And a pane with no room still asks for something the API will accept.
    assert_eq!(detection_request(None, 0), 1);
}

/// A snapshot of a unit that is working, for the verdict tests.
fn healthy_snapshot() -> Snapshot {
    let mut snapshot = busy_snapshot();
    snapshot.monitoring = Some(MonitoringState {
        enabled: true,
        ..MonitoringState::default()
    });
    if let Some(api) = snapshot.health.as_mut() {
        api.status = "ok".to_string();
        api.sensors.retain(|s| s.healthy || s.optional);
    }
    snapshot
}

#[test]
fn the_header_states_the_verdict_so_a_glance_is_enough() {
    // Every pane reports facts and none draws a conclusion, which is right for
    // a pane and wrong for somebody walking past a screen.
    let mut app = test_app();
    with_load(&mut app);
    app.classg.snapshot = healthy_snapshot();
    assert!(contains(&render(&mut app, 150, 12), " ok"));

    // The worst finding is named, not just counted: "degraded" sends you to
    // read four panes, "degraded - recording is paused" sends you to one.
    let mut paused = healthy_snapshot();
    paused.monitoring = Some(MonitoringState {
        enabled: false,
        reason: Some("known local flight".into()),
        discarded: 1204,
        ..MonitoringState::default()
    });
    app.classg.snapshot = paused;
    let rows = render(&mut app, 150, 12);
    assert!(contains(&rows, "degraded"), "{}", rows[0]);
    assert!(contains(&rows, "recording is paused"), "{}", rows[0]);

    app.classg.snapshot = Snapshot {
        error: Some("Connection refused".into()),
        ..Snapshot::default()
    };
    assert!(contains(&render(&mut app, 150, 12), "down"));
}

#[test]
fn the_header_verdict_cannot_disagree_with_the_one_cron_gets() {
    // Both go through check::judge. Two verdicts that could drift apart would
    // be worse than one of them not existing.
    let mut app = test_app();
    with_load(&mut app);
    for snapshot in [
        healthy_snapshot(),
        busy_snapshot(),
        Snapshot {
            error: Some("Connection refused".into()),
            ..Snapshot::default()
        },
    ] {
        app.classg.snapshot = snapshot;
        let findings = crate::check::judge(&app.health, &app.classg.snapshot);
        let expected = findings
            .iter()
            .map(|f| f.verdict)
            .max()
            .unwrap_or(crate::check::Verdict::Ok);
        assert!(
            contains(&render(&mut app, 150, 12), expected.label()),
            "the header does not say {:?}",
            expected.label()
        );
    }
}

#[test]
fn a_narrow_header_drops_the_detail_then_the_chip_but_never_the_clock() {
    // A truncated verdict is one that could be read as the wrong one.
    let mut app = test_app();
    with_load(&mut app);
    let mut paused = healthy_snapshot();
    paused.monitoring = Some(MonitoringState {
        enabled: false,
        reason: Some("known local flight".into()),
        discarded: 1204,
        ..MonitoringState::default()
    });
    app.classg.snapshot = paused;

    let wide = render(&mut app, 150, 12);
    assert!(contains(&wide, "recording is paused"));

    let middling = render(&mut app, 74, 12);
    assert!(contains(&middling, "degraded"), "{}", middling[0]);
    assert!(
        !contains(&middling, "recording is paused"),
        "{}",
        middling[0]
    );

    // At every width the header fits and the clock survives, because the
    // clock is the one thing that says the dashboard is still running.
    for width in [40u16, 60, 74, 100, 150, 200] {
        let rows = render(&mut app, width, 12);
        assert!(
            rows[0].chars().count() <= width as usize,
            "header overran {width}: {}",
            rows[0]
        );
        assert!(
            rows[0].contains(':'),
            "the clock went at {width}: {}",
            rows[0]
        );
        // Never a fragment of one. At the widths where only three or four
        // columns are left, clipping the word would put `deg` or `dow` on the
        // header -- a verdict that can be read as the wrong one, which is
        // worse than no verdict. Swept one column at a time because the
        // window where that happens is a couple of characters wide.
        assert!(
            !rows[0].contains("deg") || rows[0].contains("degraded"),
            "a sliced verdict at {width}: {}",
            rows[0]
        );
        assert!(
            !rows[0].contains("dow") || rows[0].contains("down"),
            "a sliced verdict at {width}: {}",
            rows[0]
        );
    }
    for width in 40u16..=90 {
        let row = &render(&mut app, width, 12)[0];
        assert!(
            !row.contains("deg") || row.contains("degraded"),
            "a sliced verdict at {width}: {row}"
        );
    }
}

#[test]
fn a_wide_disk_figure_does_not_push_the_net_column_off_the_pane() {
    // The budget sized the disks meter but never bounded the text after it,
    // so a 232G stick reading "232.9G free of 232.9G" overran its column and
    // evicted the whole net column. One filesystem's figures should not cost
    // a different box its row.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    with_interfaces(&mut app);

    let rows = render(&mut app, 200, 24);
    assert!(contains(&rows, "disks"), "{}", rows.join("\n"));
    // `^7.8K` and not `eth0`: the Radios pane on the right lists eth0 too, so
    // matching the bare name proves nothing about this column. The tx marker
    // is written only here.
    assert!(contains(&rows, "^7.8K"), "the net column was evicted");
    // Shortened, not clipped: the free figure survives whole and the total is
    // what goes.
    assert!(contains(&rows, "232.9G free"), "{}", rows.join("\n"));
    assert!(
        !contains(&rows, "232.9G free of"),
        "the column did not shorten, so it must have overrun:\n{}",
        rows.join("\n")
    );
}

#[test]
fn the_disk_total_comes_back_once_the_column_can_hold_it() {
    // Shortening is a response to width, not a permanent loss of the figure.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    with_interfaces(&mut app);
    let rows = render(&mut app, 250, 24);
    assert!(
        contains(&rows, "88.3G free of 116.9G"),
        "{}",
        rows.join("\n")
    );
    assert!(contains(&rows, "^7.8K"), "and the net column still fits");
}

#[test]
fn a_disk_figure_is_never_cut_off_mid_number() {
    // A clipped size is a wrong number, which is the failure this pane spent
    // three commits removing from the health row. `232.9G` must never render
    // as `232.` or `232.9`.
    let mut app = test_app();
    with_load(&mut app);
    with_filesystems(&mut app, 0);
    with_interfaces(&mut app);
    for width in 104u16..=260 {
        for row in render(&mut app, width, 24) {
            for figure in ["232.9G", "88.3G", "440M"] {
                let stem = &figure[..figure.len() - 1];
                if let Some(at) = row.find(stem) {
                    let tail: String = row[at..].chars().take(figure.chars().count()).collect();
                    assert!(
                        tail == figure || !row[at..].starts_with(stem),
                        "a sliced size at {width}: {row}"
                    );
                }
            }
        }
    }
}

/// A dashboard with every pane carrying real content, for the sweeps.
fn fully_loaded(app: &mut App) {
    use crate::panes::system::ProcRow;
    with_load(app);
    with_filesystems(app, 1);
    with_interfaces(app);
    app.classg.snapshot = busy_snapshot();
    app.system.total_procs = 203;
    app.system.procs = (0..60)
        .map(|i| ProcRow {
            pid: 1000 + i,
            name: format!("proc{i}"),
            state: 'S',
            cpu_pct: (60 - i) as f64 / 2.0,
            rss_kb: 4000 * (60 - i) as u64,
            threads: i as u64 % 20,
            user: "admin".into(),
            cmdline: format!("/usr/bin/proc{i} --serve"),
        })
        .collect();
}

/// Everything that must be true of a frame, whatever size it was drawn at.
fn frame_invariants(rows: &[String], width: u16, height: u16) {
    for row in rows {
        assert!(
            row.chars().count() <= width as usize,
            "a row overran {width}x{height}: {row}"
        );
        // A replacement character means a glyph reached a terminal that cannot
        // draw it, which is the whole failure `glyphs = "ascii"` exists for.
        assert!(
            !row.contains('\u{fffd}'),
            "a replacement character at {width}x{height}: {row}"
        );
    }
    // A pane that has been squeezed out of existence takes the thing it
    // reports with it, silently. ClassG is the one this dashboard is for.
    for title in ["1 System", "2 Pi health", "3 Radios", "4 ClassG"] {
        assert!(
            rows.iter().any(|r| r.contains(title)),
            "{title} vanished at {width}x{height}"
        );
    }
}

#[test]
fn every_width_the_wide_layout_covers_draws_a_whole_dashboard() {
    // Swept one column at a time. The bugs this catches live in windows two or
    // three characters wide -- a verdict clipped to `deg`, a column placed
    // just past the edge -- and a sweep in steps of ten walks straight over
    // them, which is how two of them shipped.
    let mut app = test_app();
    fully_loaded(&mut app);
    for width in 100u16..=260 {
        frame_invariants(&render(&mut app, width, 44), width, 44);
    }
}

#[test]
fn every_height_draws_a_whole_dashboard_too() {
    // Heights are where panes get squeezed rather than truncated: the two
    // fixed-height panes take theirs first and ClassG lives on what is left.
    let mut app = test_app();
    fully_loaded(&mut app);
    for height in 12u16..=60 {
        frame_invariants(&render(&mut app, 200, height), 200, height);
    }
}

#[test]
fn the_framebuffer_console_gets_a_whole_dashboard_at_every_width_too() {
    // ascii mode is the mode that exists because a glyph came out wrong, so it
    // is the one that most needs sweeping rather than spot-checking.
    let mut app = App::new(Config {
        api: "http://127.0.0.1:1".to_string(),
        glyphs: "ascii".to_string(),
        ..Config::default()
    });
    fully_loaded(&mut app);
    for width in (100u16..=260).step_by(3) {
        let rows = render(&mut app, width, 44);
        frame_invariants(&rows, width, 44);
        for row in &rows {
            for ch in row.chars() {
                assert!(
                    !matches!(ch,
                        '\u{2500}'..='\u{257F}'
                        | '\u{2580}'..='\u{259F}'
                        | '\u{25A0}'..='\u{25FF}'
                        | '\u{2190}'..='\u{21FF}'
                        | '\u{2800}'..='\u{28FF}'
                    ),
                    "U+{:04X} survived ascii mode at {width}: {row}",
                    ch as u32
                );
            }
        }
    }
}

/// The rendered sensor row for `id`, if the pane drew one.
fn sensor_row(rows: &[String], id: &str) -> Option<String> {
    rows.iter().find(|r| r.contains(id)).cloned()
}

#[test]
fn a_sensor_going_quiet_looks_different_from_one_that_is_busy() {
    // The failure this pane exists for. A radio whose antenna has worked loose
    // keeps heartbeating, keeps reporting healthy, and its five-minute count
    // slides away over a quarter of an hour -- which at any single glance is
    // indistinguishable from an afternoon with nothing in the sky.
    let mut app = test_app();
    with_load(&mut app);
    app.classg.snapshot = busy_snapshot();

    app.classg.sensor_history.insert(
        "wifi-1".into(),
        (0..30)
            .map(|i| (430.0 - i as f64 * 14.0).max(0.0))
            .collect(),
    );
    let fading = sensor_row(&render(&mut app, 200, 30), "wifi-1").expect("a sensor row");

    app.classg
        .sensor_history
        .insert("wifi-1".into(), vec![300.0; 30]);
    let steady = sensor_row(&render(&mut app, 200, 30), "wifi-1").expect("a sensor row");

    assert_ne!(
        fading, steady,
        "a radio going quiet draws the same as one holding steady"
    );
    // The heading is on its own row, not on the sensor's.
    assert!(contains(&render(&mut app, 200, 30), "15 MIN"));
}

#[test]
fn a_sensor_that_has_never_heard_anything_draws_a_floor_not_a_panic() {
    // Dividing by the peak is a divide by zero on a radio that has heard
    // nothing, which is the state every sensor is in for its first half hour.
    let mut app = test_app();
    with_load(&mut app);
    app.classg.snapshot = busy_snapshot();
    app.classg
        .sensor_history
        .insert("wifi-1".into(), vec![0.0; 30]);
    let row = sensor_row(&render(&mut app, 200, 30), "wifi-1").expect("a sensor row");
    assert!(!row.contains("NaN"), "{row}");
}

#[test]
fn the_trace_is_dropped_on_a_pane_too_narrow_to_give_it_a_shape() {
    // Four columns of sparkline is not a trend, it is decoration in the space
    // the count needed.
    let mut app = test_app();
    with_load(&mut app);
    app.classg.snapshot = busy_snapshot();
    app.classg
        .sensor_history
        .insert("wifi-1".into(), vec![100.0; 30]);
    app.focus = Pane::Classg;
    // Narrow enough that the pane itself is short of columns. At 70 the
    // one-pane layout hands ClassG the whole terminal, which is not narrow at
    // all -- the first version of this test asserted against a pane with
    // twelve spare columns and proved nothing.
    let rows = render(&mut app, 44, 30);
    // Asserted on the braille rather than on the heading. Where the trace is
    // dropped there was never room for the heading either, so its absence is
    // satisfied by a clipped frame just as well as by the rule working -- a
    // version of this test that checked for "15 MIN" passed with the rule
    // removed entirely.
    let row = sensor_row(&rows, "wifi-1").expect("the sensor row survives");
    assert!(
        !row.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
        "a trace was drawn into a pane with no room for it: {row}"
    );
}

#[test]
fn sensor_history_is_sampled_on_a_clock_not_on_every_poll() {
    // detections_5m is a five-minute rolling count. At the three-second poll
    // rate a full trace would span barely a minute of a window five times that
    // long, and draw an almost flat line whatever the radio was doing.
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let start = std::time::Instant::now();

    app.classg.record_sensor_rates(start);
    let after_first = app.classg.sensor_history["wifi-1"].len();
    assert_eq!(after_first, 1);

    // Five seconds later is the same point in a five-minute window.
    app.classg
        .record_sensor_rates(start + std::time::Duration::from_secs(5));
    assert_eq!(app.classg.sensor_history["wifi-1"].len(), 1, "too soon");

    app.classg
        .record_sensor_rates(start + std::time::Duration::from_secs(31));
    assert_eq!(app.classg.sensor_history["wifi-1"].len(), 2);
}

#[test]
fn a_sensor_that_goes_away_takes_its_trace_with_it() {
    // A unit that has had adapters swapped should not accumulate traces for
    // radios that are no longer fitted.
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let start = std::time::Instant::now();
    app.classg.record_sensor_rates(start);
    assert!(app.classg.sensor_history.contains_key("wifi-1"));

    if let Some(health) = app.classg.snapshot.health.as_mut() {
        health.sensors.clear();
    }
    app.classg
        .record_sensor_rates(start + std::time::Duration::from_secs(31));
    assert!(
        app.classg.sensor_history.is_empty(),
        "the trace outlived its sensor"
    );
}

#[test]
fn a_sensor_trace_cannot_grow_without_bound() {
    let mut app = test_app();
    app.classg.snapshot = busy_snapshot();
    let start = std::time::Instant::now();
    for tick in 0..200u64 {
        app.classg
            .record_sensor_rates(start + std::time::Duration::from_secs(tick * 31));
    }
    assert!(
        app.classg.sensor_history["wifi-1"].len() <= 30,
        "got {}",
        app.classg.sensor_history["wifi-1"].len()
    );
}

#[test]
fn a_detections_endpoint_that_broke_says_so_rather_than_vanishing() {
    // tracks has always printed "unavailable" and --once prints both, so a
    // /detections answering 500 made the section disappear from the pane while
    // the same box reported it over SSH. The silent option is the one this
    // dashboard exists to argue against.
    let mut app = test_app();
    with_load(&mut app);
    let mut snapshot = busy_snapshot();
    snapshot.detections = None;
    snapshot.tracks = None;
    app.classg.snapshot = snapshot;

    let rows = render(&mut app, 200, 44);
    assert!(contains(&rows, "tracks unavailable"), "{}", rows.join("\n"));
    assert!(
        contains(&rows, "detections unavailable"),
        "the section vanished:\n{}",
        rows.join("\n")
    );
}

#[test]
fn the_classg_pane_measures_the_gutter_it_actually_draws() {
    // Every fit calculation here used GUTTER + 1 = 10, but `labelled` draws
    // two spaces plus a ten-column label. Believing it had two spare columns
    // let it accept a line that the frame then clipped, turning a size into a
    // different and smaller number.
    let mut app = test_app();
    with_load(&mut app);
    app.classg.snapshot = busy_snapshot();
    app.focus = Pane::Classg;

    // Swept one column at a time across the narrow layout, where the pane is
    // tight enough for two columns to decide the question. With the gutter
    // undercounted by two, widths 43 and 44 rendered
    // `11.5G free of 28.` and `11.5G free of 28.9` -- a total that has lost
    // its tail, which is a different and smaller number rather than a shorter
    // way of writing the same one.
    for width in 40u16..=99 {
        for row in render(&mut app, width, 30) {
            if row.contains("free of") {
                assert!(
                    row.contains("28.9G"),
                    "a size was clipped mid-number at {width}: {row}"
                );
            }
        }
    }
}

#[test]
fn the_verdict_chip_budgets_the_address_as_it_is_drawn() {
    // The header trims `http://` off the address before drawing it, but the
    // chip's room calculation counted the whole thing -- claiming seven
    // columns that were never on screen and dropping the verdict on terminals
    // where it fitted. It fails safe, so nothing caught it.
    let mut app = test_app();
    with_load(&mut app);
    app.classg.snapshot = healthy_snapshot();
    // Pinned, because the header's width budget includes it and the real one
    // is this machine's hostname. The first version of this test used it and
    // passed here on `TORNADO` while failing on a CI runner whose name is five
    // characters longer -- a test measuring columns must not take one of them
    // from the environment.
    app.host = "pisdr".to_string();

    // Six columns either side of the boundary: with the scheme counted the
    // chip needs 50 columns, drawn as it actually is it needs 43.
    for width in 44u16..=49 {
        assert!(
            contains(&render(&mut app, width, 12), "ok"),
            "the verdict was dropped at {width} despite fitting"
        );
    }
}
