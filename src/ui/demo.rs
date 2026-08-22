//! The frame the README opens with, and the generator behind it.
//!
//! The picture at the top of the README used to be a block of text somebody
//! kept up to date by hand, and it drifted: it had untitled panes long after
//! the titles carried their hotkey number, and a footer with no `f filter` on
//! it. So it is generated now, from this fixture, through the same `draw`
//! every other rendering test drives -- which means the layout, the meter
//! ramp, the row stripes and the accent on the focused pane are the code's
//! and not a drawing of it. Only the numbers are staged.
//!
//! Regenerate after any change that moves the frame:
//!
//! ```text
//! PI_DASH_WRITE_DEMO=1 cargo test the_readme_demo
//! ```
//!
//! which rewrites `assets/pi-dash.svg` and the text block inside the README's
//! `<details>`. Without the variable the test still renders and asserts, so a
//! layout change that breaks the demo fails in CI rather than at the next
//! person to look at the front page.
//!
//! The frame is a healthy unit on purpose. The failure rendering is worth
//! seeing and every pane has tests for it, but the first thing anybody sees
//! of this project should not be a box with a radio missing.

use std::fmt::Write as _;

use ratatui::{
    backend::TestBackend,
    style::{Color, Modifier},
    Terminal,
};

use super::draw;
use crate::app::App;
use crate::config::Config;
use crate::panes::classg::{
    Detection, DetectionPage, Evidence, FlexTime, FusionHealth, HealthResponse, Identity,
    MonitoringState, Position, Rf, SensorHealth, Slow, SystemBuild, SystemHost, SystemInfo,
    SystemRuntime, Track, TrackPage,
};
use crate::panes::health::{DiskUsage, IoRates, Throttle};
use crate::panes::radios::{Iface, UsbRadio, WirelessMode};
use crate::panes::system::{MemInfo, ProcRow};

/// Wide enough for both columns, tall enough that the ClassG pane still has
/// rows for the detection list once the sensors and the track have had theirs.
const WIDTH: u16 = 128;
const HEIGHT: u16 = 46;

fn ago(secs: i64) -> FlexTime {
    FlexTime(chrono::Utc::now() - chrono::Duration::seconds(secs))
}

fn demo_app() -> App {
    let mut app = App::new(Config {
        api: "http://127.0.0.1:8081".to_string(),
        ..Config::default()
    });
    app.host = "classg-pi".to_string();

    app.system.unavailable = None;
    app.system.cpu_pct = Some(24.0);
    app.system.core_pct = vec![Some(22.0), Some(14.0), Some(41.0), Some(9.0)];
    app.system.mem = MemInfo {
        total_kb: 8_000_000,
        available_kb: 4_960_000,
        cached_kb: 1_990_000,
        buffers_kb: 90_000,
        swap_total_kb: 0,
        swap_free_kb: 0,
    };
    app.system.load = [0.52, 0.31, 0.20];
    app.system.uptime_secs = 273_900;
    app.system.thread_count = 214;
    app.system.runnable = 2;
    app.system.total_procs = 214;
    // A shape rather than a flat line: a quiet box with two bursts in the
    // window, which is what the graph is there to show.
    app.system.cpu_history = (0..320)
        .map(|i| {
            let x = i as f64 / 16.0;
            let base = 0.16 + 0.10 * (x * 0.6).sin() + 0.05 * (x * 2.9).cos();
            let burst = if (120..176).contains(&i) {
                0.70 * (((i - 120) as f64) / 56.0 * std::f64::consts::PI).sin()
            } else if (232..252).contains(&i) {
                0.45
            } else if (280..296).contains(&i) {
                0.24
            } else {
                0.0
            };
            (base + burst).clamp(0.03, 0.97)
        })
        .collect();
    app.system.core_history = (0..4)
        .map(|core| {
            (0..48)
                .map(|i| {
                    let x = (i as f64 + core as f64 * 7.0) / 5.0;
                    (0.16 + 0.14 * x.sin() + 0.09 * (x * 1.7).cos() + core as f64 * 0.04)
                        .clamp(0.02, 0.95)
                })
                .collect()
        })
        .collect();
    app.system.procs = demo_procs();

    app.health.temp_c = Some(58.4);
    app.health.volts = Some(0.8563);
    app.health.arm_mhz = Some(1500);
    app.health.max_mhz = Some(1800);
    app.health.throttle = Some(Throttle::decode(0));
    app.health.disk = Some(DiskUsage {
        used_kb: 22_020_096,
        total_kb: 58_720_256,
        avail_kb: 34_603_008,
    });
    app.health.io = IoRates {
        read_bps: 0.0,
        write_bps: 86_016.0,
    };

    app.radios.ifaces = vec![
        Iface {
            name: "wlan1".to_string(),
            state: "up".to_string(),
            rx_bps: 1_258_291.0,
            tx_bps: 0.0,
            rx_total: 41_000_000_000,
            tx_total: 0,
            mode: Some(WirelessMode::Monitor),
            channel: Some(6),
            driver: Some("mt7921u".to_string()),
        },
        Iface {
            name: "eth0".to_string(),
            state: "up".to_string(),
            rx_bps: 4_096.0,
            tx_bps: 2_048.0,
            rx_total: 900_000_000,
            tx_total: 240_000_000,
            mode: None,
            channel: None,
            driver: Some("bcmgenet".to_string()),
        },
    ];
    app.radios.usb = vec![
        UsbRadio {
            id: "0e8d:7961".to_string(),
            description: "MediaTek ALFA AWUS036AXML".to_string(),
        },
        UsbRadio {
            id: "0bda:2838".to_string(),
            description: "Realtek RTL2838 (RTL-SDR V4)".to_string(),
        },
    ];
    app.radios.throughput = (0..80)
        .map(|i| {
            let x = i as f64 / 6.0;
            600_000.0 + 420_000.0 * x.sin().abs() + 120_000.0 * (x * 2.1).cos()
        })
        .collect();

    app.classg.sensor_history = [
        (
            "wifi-1".to_string(),
            (0..60)
                .map(|i| {
                    let x = i as f64 / 7.0;
                    (0.35 + 0.34 * x.sin() + 0.14 * (x * 2.3).cos()).clamp(0.05, 0.98)
                })
                .collect::<Vec<f64>>(),
        ),
        (
            "sdr-1".to_string(),
            (0..60)
                .map(|i| {
                    let x = i as f64 / 9.0;
                    (0.18 + 0.16 * (x * 1.3).sin() + 0.07 * (x * 0.5).cos()).clamp(0.03, 0.9)
                })
                .collect::<Vec<f64>>(),
        ),
    ]
    .into_iter()
    .collect();
    app.classg.polls = 312;
    app.classg.last_ok = Some(std::time::Instant::now());
    app.classg.snapshot = crate::panes::classg::Snapshot {
        health: Some(HealthResponse {
            status: "ok".to_string(),
            uptime_s: 271_500,
            version: "0.4.1".to_string(),
            sensors: vec![
                SensorHealth {
                    sensor_id: "wifi-1".to_string(),
                    sensor_kind: "wifi".to_string(),
                    healthy: true,
                    seconds_since_heartbeat: Some(1),
                    detections_5m: 12,
                    ..SensorHealth::default()
                },
                SensorHealth {
                    sensor_id: "sdr-1".to_string(),
                    sensor_kind: "sdr".to_string(),
                    healthy: true,
                    seconds_since_heartbeat: Some(2),
                    detections_5m: 4,
                    ..SensorHealth::default()
                },
            ],
            fusion: Some(FusionHealth {
                connected: true,
                configured: true,
                last_message: Some(ago(2)),
                reason: None,
            }),
        }),
        monitoring: Some(MonitoringState {
            enabled: true,
            since: Some(ago(271_000)),
            ..MonitoringState::default()
        }),
        tracks: Some(TrackPage {
            tracks: vec![Track {
                state: "CONFIRMED".to_string(),
                confidence: 0.82,
                first_seen: Some(ago(240)),
                last_seen: Some(ago(3)),
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
            detections: vec![
                Detection {
                    ts: Some(ago(3)),
                    sensor_id: "wifi-1".to_string(),
                    sensor_kind: "wifi".to_string(),
                    detection_class: "A".to_string(),
                    rf: Some(Rf {
                        channel: Some(149),
                        rssi_dbm: Some(-52.0),
                        ..Rf::default()
                    }),
                    identity: Some(Identity {
                        model_hint: Some("Mavic 3".to_string()),
                        ..Identity::default()
                    }),
                    ..Detection::default()
                },
                Detection {
                    ts: Some(ago(19)),
                    sensor_id: "wifi-1".to_string(),
                    sensor_kind: "wifi".to_string(),
                    detection_class: "C".to_string(),
                    rf: Some(Rf {
                        channel: Some(6),
                        rssi_dbm: Some(-71.0),
                        ..Rf::default()
                    }),
                    ..Detection::default()
                },
                Detection {
                    ts: Some(ago(34)),
                    sensor_id: "sdr-1".to_string(),
                    sensor_kind: "sdr".to_string(),
                    detection_class: "E".to_string(),
                    rf: Some(Rf {
                        freq_hz: Some(2_437_000_000),
                        rssi_dbm: Some(-64.0),
                        ..Rf::default()
                    }),
                    ..Detection::default()
                },
            ],
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
        ..crate::panes::classg::Snapshot::default()
    };
    app
}

/// The columns the process table draws, in the order it draws them: pid,
/// name, state, CPU percent, RSS in kB, thread count, user, command line.
type DemoProc = (
    i32,
    &'static str,
    char,
    f64,
    u64,
    u64,
    &'static str,
    &'static str,
);

fn demo_procs() -> Vec<ProcRow> {
    let rows: &[DemoProc] = &[
        (
            1_284,
            "classg-api",
            'S',
            41.2,
            122_880,
            14,
            "classg",
            "/usr/local/bin/classg-api --config /etc/classg/api.toml",
        ),
        (
            1_290,
            "classg_wifi",
            'S',
            18.7,
            65_536,
            6,
            "classg",
            "/usr/bin/python3 -m classg_wifi --iface wlan1",
        ),
        (
            1_301,
            "classg-fusion",
            'S',
            4.1,
            9_216,
            4,
            "classg",
            "/usr/local/bin/classg-fusion",
        ),
        (
            902,
            "dockerd",
            'S',
            1.4,
            210_944,
            23,
            "root",
            "/usr/bin/dockerd -H fd://",
        ),
        (
            1_337,
            "pi-dash",
            'R',
            0.9,
            7_168,
            3,
            "classg",
            "/usr/local/bin/pi-dash",
        ),
        (
            704,
            "containerd",
            'S',
            0.7,
            71_680,
            17,
            "root",
            "/usr/bin/containerd",
        ),
        (
            1_312,
            "classg-web",
            'S',
            0.6,
            43_008,
            5,
            "classg",
            "/usr/local/bin/classg-web --listen 0.0.0.0:8080",
        ),
        (
            448,
            "systemd-journald",
            'S',
            0.5,
            18_432,
            1,
            "root",
            "/lib/systemd/systemd-journald",
        ),
        (61, "kworker/1:2-events", 'I', 0.4, 0, 1, "root", ""),
        (
            1_198,
            "sshd",
            'S',
            0.3,
            11_264,
            1,
            "root",
            "sshd: classg@pts/0",
        ),
        (
            236,
            "rtl_433",
            'S',
            0.2,
            5_120,
            2,
            "classg",
            "/usr/bin/rtl_433 -F json",
        ),
        (
            1_155,
            "classg-deploy-agent",
            'S',
            0.2,
            14_336,
            4,
            "classg",
            "/usr/local/bin/classg-deploy-agent",
        ),
        (
            1_163,
            "classg-watchdog",
            'S',
            0.2,
            6_144,
            2,
            "classg",
            "/usr/local/bin/classg-watchdog",
        ),
        (
            1,
            "systemd",
            'S',
            0.1,
            12_288,
            1,
            "root",
            "/sbin/init splash",
        ),
        (9, "ksoftirqd/0", 'S', 0.1, 0, 1, "root", ""),
        (
            392,
            "systemd-udevd",
            'S',
            0.1,
            7_168,
            1,
            "root",
            "/lib/systemd/systemd-udevd",
        ),
        (
            511,
            "avahi-daemon",
            'S',
            0.1,
            3_072,
            1,
            "avahi",
            "avahi-daemon: running [classg-pi.local]",
        ),
        (
            598,
            "NetworkManager",
            'S',
            0.1,
            20_480,
            3,
            "root",
            "/usr/sbin/NetworkManager --no-daemon",
        ),
        (
            612,
            "wpa_supplicant",
            'S',
            0.1,
            8_192,
            1,
            "root",
            "/sbin/wpa_supplicant -u -s",
        ),
        (
            877,
            "chronyd",
            'S',
            0.1,
            4_096,
            1,
            "chrony",
            "/usr/sbin/chronyd -F 1",
        ),
        (63, "kworker/2:1-mm_percpu_wq", 'I', 0.1, 0, 1, "root", ""),
        (
            1_401,
            "docker-proxy",
            'S',
            0.0,
            9_216,
            6,
            "root",
            "/usr/bin/docker-proxy -proto tcp -host-port 8081",
        ),
        (
            1_420,
            "redis-server",
            'S',
            0.0,
            15_360,
            4,
            "redis",
            "redis-server 127.0.0.1:6379",
        ),
        (
            2_011,
            "cron",
            'S',
            0.0,
            3_072,
            1,
            "root",
            "/usr/sbin/cron -f",
        ),
        (
            2_087,
            "rsyslogd",
            'S',
            0.0,
            5_120,
            4,
            "root",
            "/usr/sbin/rsyslogd -n -iNONE",
        ),
        (
            2_154,
            "dbus-daemon",
            'S',
            0.0,
            4_096,
            1,
            "messagebus",
            "/usr/bin/dbus-daemon --system",
        ),
        (
            2_201,
            "bluetoothd",
            'S',
            0.0,
            6_144,
            1,
            "root",
            "/usr/libexec/bluetooth/bluetoothd",
        ),
        (
            2_260,
            "polkitd",
            'S',
            0.0,
            9_216,
            3,
            "polkitd",
            "/usr/lib/polkit-1/polkitd --no-debug",
        ),
    ];
    rows.iter()
        .map(
            |(pid, name, state, cpu_pct, rss_kb, threads, user, cmdline)| ProcRow {
                pid: *pid,
                name: name.to_string(),
                state: *state,
                cpu_pct: *cpu_pct,
                rss_kb: *rss_kb,
                threads: *threads,
                user: user.to_string(),
                cmdline: cmdline.to_string(),
            },
        )
        .collect()
}

// ---------------------------------------------------------------------------
// The frame, and the SVG it becomes
// ---------------------------------------------------------------------------

/// One stretch of a row that shares a colour, which is also one `<text>`.
struct Run {
    fg: Color,
    bg: Color,
    bold: bool,
    text: String,
}

/// Renders the demo and collects the buffer into runs, the same way the
/// terminal backend would: neighbouring cells that share a style are one span.
fn render_runs() -> Vec<Vec<Run>> {
    let mut app = demo_app();
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test backend");
    terminal
        .draw(|frame| draw(frame, &mut app))
        .expect("draw must not fail");
    let buffer = terminal.backend().buffer().clone();

    (0..HEIGHT)
        .map(|y| {
            let mut runs: Vec<Run> = Vec::new();
            for x in 0..WIDTH {
                let cell = &buffer[(x, y)];
                let bold = cell.modifier.contains(Modifier::BOLD);
                match runs.last_mut() {
                    Some(last) if last.fg == cell.fg && last.bg == cell.bg && last.bold == bold => {
                        last.text.push_str(cell.symbol());
                    }
                    _ => runs.push(Run {
                        fg: cell.fg,
                        bg: cell.bg,
                        bold,
                        text: cell.symbol().to_string(),
                    }),
                }
            }
            runs
        })
        .collect()
}

fn plain_text(rows: &[Vec<Run>]) -> String {
    let mut out = String::new();
    for runs in rows {
        let line: String = runs.iter().map(|run| run.text.as_str()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

// The sixteen ANSI slots, as a terminal with a dark theme would draw them. The
// cube and greyscale indices below are xterm's own values instead, because the
// meters address those directly: the gradient is the app's, not a choice to be
// made a second time here.
const ANSI: [&str; 16] = [
    "#0d1017", "#e05561", "#3fca7c", "#e0b755", "#4b9fea", "#c678dd", "#3fb6cf", "#c8d0da",
    "#6e7787", "#ff7b86", "#6ee79b", "#f2d17b", "#79bdff", "#e0a3f0", "#6fd7ea", "#eef2f7",
];
const BACKGROUND: &str = "#0d1017";
const FOREGROUND: &str = "#c8d0da";
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn indexed(n: u8) -> String {
    match n {
        0..=15 => ANSI[n as usize].to_string(),
        16..=231 => {
            let n = n - 16;
            format!(
                "#{:02x}{:02x}{:02x}",
                CUBE[(n / 36) as usize],
                CUBE[((n / 6) % 6) as usize],
                CUBE[(n % 6) as usize]
            )
        }
        232..=255 => {
            let level = 8 + (n as u16 - 232) * 10;
            format!("#{level:02x}{level:02x}{level:02x}")
        }
    }
}

/// `None` where the terminal would leave the cell to the theme underneath,
/// which for a background means drawing no rectangle at all.
fn colour(color: Color) -> Option<String> {
    let ansi = |slot: usize| Some(ANSI[slot].to_string());
    match color {
        Color::Reset => None,
        Color::Black => ansi(0),
        Color::Red => ansi(1),
        Color::Green => ansi(2),
        Color::Yellow => ansi(3),
        Color::Blue => ansi(4),
        Color::Magenta => ansi(5),
        Color::Cyan => ansi(6),
        Color::Gray => ansi(7),
        Color::DarkGray => ansi(8),
        Color::LightRed => ansi(9),
        Color::LightGreen => ansi(10),
        Color::LightYellow => ansi(11),
        Color::LightBlue => ansi(12),
        Color::LightMagenta => ansi(13),
        Color::LightCyan => ansi(14),
        Color::White => ansi(15),
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(n) => Some(indexed(n)),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Cell geometry. Every run is placed at its own column and given a
/// `textLength`, so a font that renders braille or box drawing a shade wide --
/// which is most of them, since the fallback for those glyphs is rarely the
/// same face as the rest of the line -- cannot walk a row out of alignment.
const ADVANCE: f64 = 8.4;
const LINE: f64 = 16.1;
const SIZE: f64 = 14.0;
const PAD_X: f64 = 18.0;
const PAD_Y: f64 = 15.0;

fn svg(rows: &[Vec<Run>]) -> String {
    let width = WIDTH as f64 * ADVANCE + 2.0 * PAD_X;
    let height = rows.len() as f64 * LINE + 2.0 * PAD_Y;
    let mut out = String::new();

    // Intrinsic width and height, and no styling on the <img> side: GitHub
    // caps an image at the column width and scales its height to match, which
    // is the whole reason this is a picture and not the text it replaced.
    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.0} {height:.0}\" \
         width=\"{width:.0}\" height=\"{height:.0}\" \
         font-family=\"ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', monospace\" \
         font-size=\"{SIZE}\" role=\"img\" \
         aria-label=\"pi-dash running on a Raspberry Pi: the System pane with a CPU history graph, \
per-core meters and the process table, beside the Pi health, Radios and ClassG panes\">"
    );
    let _ = writeln!(
        out,
        "<rect width=\"100%\" height=\"100%\" rx=\"10\" fill=\"{BACKGROUND}\"/>"
    );

    // Backgrounds first, so the striped process rows sit under their own text.
    for (y, runs) in rows.iter().enumerate() {
        let mut x = 0usize;
        for run in runs {
            let cells = run.text.chars().count();
            if let Some(fill) = colour(run.bg) {
                let _ = writeln!(
                    out,
                    "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"{fill}\"/>",
                    PAD_X + x as f64 * ADVANCE,
                    PAD_Y + y as f64 * LINE + 1.0,
                    cells as f64 * ADVANCE,
                    LINE
                );
            }
            x += cells;
        }
    }

    for (y, runs) in rows.iter().enumerate() {
        let baseline = PAD_Y + y as f64 * LINE + SIZE * 0.78 + 2.0;
        let mut x = 0usize;
        for run in runs {
            let cells = run.text.chars().count();
            if !run.text.trim().is_empty() {
                let fill = colour(run.fg).unwrap_or_else(|| FOREGROUND.to_string());
                let weight = if run.bold {
                    " font-weight=\"bold\""
                } else {
                    ""
                };
                let _ = writeln!(
                    out,
                    "<text x=\"{:.2}\" y=\"{baseline:.2}\" fill=\"{fill}\"{weight} \
                     textLength=\"{:.2}\" lengthAdjust=\"spacing\" xml:space=\"preserve\">{}</text>",
                    PAD_X + x as f64 * ADVANCE,
                    cells as f64 * ADVANCE,
                    escape(&run.text)
                );
            }
            x += cells;
        }
    }
    out.push_str("</svg>\n");
    out
}

/// Replaces the frame inside the README's `<details>` and leaves everything
/// around it alone. `None` if the markers are not where they were, because
/// rewriting the wrong half of a README is worse than not rewriting it.
fn splice_readme(readme: &str, frame: &str) -> Option<String> {
    let summary = readme.find("<summary>The same frame as text</summary>")?;
    let open = readme[summary..].find("```")? + summary;
    let body = open + "```\n".len();
    let close = readme[body..].find("```")? + body;
    Some(format!("{}{frame}{}", &readme[..body], &readme[close..]))
}

#[test]
fn the_readme_demo_frame_still_renders() {
    let rows = render_runs();
    let text = plain_text(&rows);

    for marker in [
        "1 System",
        "2 Pi health",
        "3 Radios & network",
        "4 ClassG",
        // The pane that loses its rows first when the layout moves: it is the
        // one handed whatever the other three did not take.
        "detections 1284 total",
    ] {
        assert!(text.contains(marker), "no {marker} in the demo:\n{text}");
    }
    // The verdict chip is the loudest thing on the frame, and a demo that
    // opens on a fault reads as a broken unit. The fixture is a healthy box;
    // this is what keeps it one.
    let header = text.lines().next().unwrap_or_default();
    assert!(
        header.contains(" ok "),
        "the demo must open on a healthy unit, not `{header}`"
    );

    if std::env::var("PI_DASH_WRITE_DEMO").is_err() {
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(root.join("assets/pi-dash.svg"), svg(&rows)).expect("write the SVG");
    let readme = std::fs::read_to_string(root.join("README.md")).expect("read the README");
    let spliced = splice_readme(&readme, &text).expect("the README's demo block");
    std::fs::write(root.join("README.md"), spliced).expect("write the README");
}

#[test]
fn the_readme_splice_leaves_the_prose_alone() {
    let readme = "# t\n\n![x](assets/pi-dash.svg)\n\n<details>\n<summary>The same frame as text</summary>\n\n```\nold\n```\n\n</details>\n\ntail\n";
    let spliced = splice_readme(readme, "new\n").expect("markers");
    assert!(spliced.contains("```\nnew\n```"), "{spliced}");
    assert!(spliced.ends_with("</details>\n\ntail\n"), "{spliced}");
    assert!(!spliced.contains("old"), "{spliced}");
    // A README that has been restructured past recognition is left alone.
    assert!(splice_readme("# t\n\nno markers here\n", "new\n").is_none());
}
