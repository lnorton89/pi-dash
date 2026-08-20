//! `pi-dash --once`: one plain-text snapshot, no terminal required.
//!
//! This exists because the interesting failures on this box are not the kind a
//! type-checker or a unit test can see — a wedged adapter, an under-volting
//! supply, an API that is not listening. `--once` is how you check the readers
//! against real hardware over SSH, from a cron job, or from a CI step, without
//! a TTY to drive.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::Config;
use crate::format::{human_bytes, human_kb, human_rate, human_rate_compact, short_age, uptime};
use crate::panes::classg::{
    collapse_runs, detection_class_label, Client, CredentialKind, Slow, Snapshot,
};
use crate::panes::health::{HealthPane, Tense, Throttle};
use crate::panes::radios::{RadiosPane, WirelessMode};
use crate::panes::system::SystemPane;

/// Gap between the two samples. Every rate on this dashboard is a difference
/// between two readings, so a single sample can only ever report zero — the
/// snapshot has to take two and wait in between, and that wait is the price of
/// the numbers being real.
const SETTLE: Duration = Duration::from_millis(700);

pub(crate) fn print_once(config: &Config, out: &mut impl Write) -> Result<()> {
    let mut system = SystemPane::default();
    let mut health = HealthPane::default();
    let mut radios = RadiosPane::default();

    system.sample(Instant::now());
    health.sample(Instant::now());
    radios.sample(
        Instant::now(),
        &config.usb_vendor_ids,
        &config.ignore_interfaces,
    );
    std::thread::sleep(SETTLE);
    let now = Instant::now();
    system.sample(now);
    health.sample(now);
    radios.sample(now, &config.usb_vendor_ids, &config.ignore_interfaces);

    writeln!(out, "system")?;
    match &system.unavailable {
        Some(reason) => writeln!(out, "  {reason}")?,
        None => {
            writeln!(
                out,
                "  cpu      {}  ({} cores)",
                system
                    .cpu_pct
                    .map(|p| format!("{p:.0}%"))
                    .unwrap_or_else(|| "-".to_string()),
                system.core_pct.len()
            )?;
            writeln!(
                out,
                "  mem      {}/{} ({:.0}%)   swap {}",
                human_kb(system.mem.used_kb()),
                human_kb(system.mem.total_kb),
                system.mem.used_pct(),
                if system.mem.swap_total_kb == 0 {
                    "off".to_string()
                } else {
                    format!(
                        "{}/{}",
                        human_kb(system.mem.swap_used_kb()),
                        human_kb(system.mem.swap_total_kb)
                    )
                }
            )?;
            writeln!(
                out,
                "  load     {:.2} {:.2} {:.2}   up {}   {} tasks",
                system.load[0],
                system.load[1],
                system.load[2],
                uptime(system.uptime_secs),
                system.task_count
            )?;
            for proc in system.procs.iter().take(5) {
                // The command line, not just the comm. The kernel truncates
                // comm to fifteen characters, so `classg-sensor-` and
                // `classg-sensor-s` are the same string and the argv is the
                // only thing that says which sensor it is. The pane shows both
                // columns; this showed neither.
                //
                // A kernel thread has no argv at all, and its bracketed comm
                // is what says so — the same convention the pane uses, because
                // `[kworker/1:2-events]` reading differently in the two views
                // would be two facts where there is one.
                let command = if proc.cmdline.is_empty() {
                    format!("[{}]", proc.name)
                } else {
                    proc.cmdline.clone()
                };
                writeln!(
                    out,
                    "  {:>7}  {:>6.1}%  {:>7}  {}",
                    proc.pid,
                    proc.cpu_pct,
                    human_kb(proc.rss_kb),
                    command
                )?;
            }
        }
    }

    writeln!(out)?;
    writeln!(out, "health")?;
    writeln!(
        out,
        "  temp     {}",
        health
            .temp_c
            .map(|t| format!("{t:.1}C"))
            .unwrap_or_else(|| "no thermal zone".to_string())
    )?;
    writeln!(
        out,
        "  power    {}   clock {}",
        health
            .volts
            .map(|v| format!("{v:.4}V"))
            .unwrap_or_else(|| "?V".to_string()),
        match (health.arm_mhz, health.max_mhz) {
            (Some(now), Some(max)) => format!("{now}/{max} MHz"),
            (Some(now), None) => format!("{now} MHz"),
            _ => "?".to_string(),
        }
    )?;
    writeln!(out, "  throttle {}", describe_throttle(health.throttle))?;
    writeln!(
        out,
        "  disk     {}",
        health
            .disk
            .map(|d| format!(
                // Free is the number you act on, and it is df's Available --
                // not total minus used, which counts the 5% ext4 holds back
                // for root and nothing here can write into.
                "{} used, {} free of {} ({:.0}%)",
                human_kb(d.used_kb),
                human_kb(d.avail_kb),
                human_kb(d.total_kb),
                d.pct()
            ))
            .unwrap_or_else(|| "unavailable".to_string())
    )?;
    writeln!(
        out,
        "  io       r {}  w {}",
        human_rate(health.io.read_bps),
        human_rate(health.io.write_bps)
    )?;

    writeln!(out)?;
    writeln!(out, "radios")?;
    for iface in &radios.ifaces {
        writeln!(
            out,
            "  {:<10} {:<8} v{:<8} ^{:<8} {}{}",
            iface.name,
            iface.state,
            human_rate_compact(iface.rx_bps),
            human_rate_compact(iface.tx_bps),
            match iface.mode {
                Some(WirelessMode::Monitor) => "monitor",
                Some(WirelessMode::Managed) => "managed",
                None => "",
            },
            iface.channel.map(|c| format!(" ch{c}")).unwrap_or_default()
        )?;
    }
    if radios.usb.is_empty() {
        writeln!(out, "  usb      none present - adapters gone from the bus")?;
    } else {
        for device in &radios.usb {
            writeln!(out, "  usb      {}  {}", device.id, device.description)?;
        }
    }

    writeln!(out)?;
    writeln!(out, "classg  {}", config.api)?;
    // A one-shot has no previous slow tier to carry forward and no second
    // chance to fetch one, so it always asks for the whole picture.
    let client = Client::new(config.api.clone(), config.credential());
    print_classg(&client.poll(5, 5, true, &Slow::default()), out)?;
    Ok(())
}

fn describe_throttle(throttle: Option<Throttle>) -> String {
    let Some(throttle) = throttle else {
        return "unknown - no vcgencmd here".to_string();
    };
    if throttle.clean() {
        return "ok, clean since boot".to_string();
    }
    // Named rather than concatenated: "under-voltage, throttled" on its own
    // does not say whether the supply is sagging now or sagged an hour ago,
    // and that is the entire difference between the two failure modes.
    let join = |labels: Vec<&str>| {
        if labels.is_empty() {
            "-".to_string()
        } else {
            labels.join(", ")
        }
    };
    format!(
        "now [{}]  since boot [{}]  (0x{:x})",
        join(throttle.now.labels(Tense::Now)),
        join(throttle.since_boot.labels(Tense::SinceBoot)),
        throttle.raw
    )
}

fn print_classg(snapshot: &Snapshot, out: &mut impl Write) -> Result<()> {
    let Some(health) = &snapshot.health else {
        writeln!(
            out,
            "  not reachable: {}",
            snapshot.error.as_deref().unwrap_or("unknown error")
        )?;
        writeln!(out, "  start it with: make dev   (or set CLASSG_API)")?;
        return Ok(());
    };

    // The build string beats the bare version when /system answered: a
    // revision is what tells you whether the binary on the Pi is the one you
    // last deployed, and /health cannot say.
    let build = snapshot
        .slow
        .system
        .as_ref()
        .map(|system| system.build_label())
        .unwrap_or_else(|| health.version.clone());
    writeln!(
        out,
        "  status   {}  up {}s  {}",
        health.status, health.uptime_s, build
    )?;

    if let Some(system) = &snapshot.slow.system {
        // Free space on the filesystem detections land on, which is not
        // necessarily the one the health pane above measured.
        let disk = match (system.host.disk_free_bytes, system.host.disk_total_bytes) {
            (Some(free), Some(total)) => {
                format!("{} free of {}", human_bytes(free), human_bytes(total))
            }
            _ => "unavailable".to_string(),
        };
        writeln!(
            out,
            "  store    {}  {}  {}",
            system.runtime.store, system.host.disk_path, disk
        )?;
    }

    // Recording state before anything it affects. A paused stack reports
    // healthy sensors, a connected fusion and an empty track list, which is
    // line for line what a quiet sky looks like.
    match &snapshot.monitoring {
        Some(state) if state.enabled => writeln!(out, "  record   on")?,
        Some(state) => writeln!(
            out,
            "  record   PAUSED  {} discarded  {}",
            state.discarded,
            state.reason.as_deref().unwrap_or("no reason given")
        )?,
        None => {}
    }

    if let Some(denied) = &snapshot.denied {
        writeln!(out, "  auth     refused: {denied}")?;
        if let Some(auth) = &snapshot.slow.auth {
            if auth.setup_required {
                writeln!(out, "           this unit has no accounts yet")?;
            } else if auth.auth_enabled && !auth.authenticated {
                // Which credential went out decides the remedy. A local token
                // that is refused has almost certainly just been rotated by an
                // API restart, and the running dashboard re-reads it; sending
                // somebody to set CLASSG_SESSION would be advice for a problem
                // they do not have.
                writeln!(
                    out,
                    "           {}",
                    match snapshot.credential {
                        Some(CredentialKind::Local) =>
                            "the local-agent token was rejected; the API rewrites it on restart",
                        Some(CredentialKind::Session) => "CLASSG_SESSION has expired",
                        None => "no local-agent token found; check .agent-state is readable",
                    }
                )?;
            }
        }
    }

    for sensor in &health.sensors {
        let state = match (sensor.healthy, sensor.optional) {
            (true, _) => "ok",
            (false, true) => "off",
            (false, false) => "DOWN",
        };
        writeln!(
            out,
            "  sensor   {:<12} {:<5} beat {:<5} 5m:{:<6} {}",
            sensor.sensor_id,
            state,
            sensor
                .seconds_since_heartbeat
                .map(|s| format!("{s}s"))
                .unwrap_or_else(|| "-".to_string()),
            sensor.detections_5m,
            sensor.reason.as_deref().unwrap_or("")
        )?;
    }

    let fusion = health.fusion.clone().unwrap_or_default();
    writeln!(
        out,
        "  fusion   {}",
        if fusion.connected {
            match fusion.last_message.as_ref() {
                Some(ts) => format!("connected, last message {}", short_age(ts.age_secs())),
                None => "connected, no messages yet".to_string(),
            }
        } else if fusion.configured {
            format!("DOWN  {}", fusion.reason.as_deref().unwrap_or(""))
        } else {
            "not configured".to_string()
        }
    )?;

    // A capture or a sweep holds a radio exclusively, so either one is the
    // explanation for a sensor that has only just gone quiet.
    if let Some(capture) = snapshot.running_capture() {
        writeln!(
            out,
            "  capture  running on {} ch{} for {}s",
            capture.iface, capture.channel, capture.duration_s
        )?;
    } else if let Some(capture) = snapshot.latest_capture().filter(|c| c.state == "failed") {
        writeln!(
            out,
            "  capture  failed: {}",
            capture.error.as_deref().unwrap_or("no reason recorded")
        )?;
    }
    if let Some(sweep) = snapshot.running_sweep() {
        writeln!(out, "  sweep    running on {} (radio busy)", sweep.band)?;
    }

    match &snapshot.tracks {
        Some(page) => {
            writeln!(out, "  tracks   {} live", page.total)?;
            for track in &page.tracks {
                let identity = track
                    .identity
                    .as_ref()
                    .map(|i| i.label())
                    .unwrap_or_else(|| "unknown".to_string());
                writeln!(
                    out,
                    "    {:<10} {:.2}  {:<20} {}x  {}{}",
                    track.state,
                    track.confidence,
                    identity,
                    track.detection_count,
                    track.evidence_classes(),
                    // Corroborating-only evidence is consistent with an
                    // aircraft without being one. Saying so is the whole
                    // difference between a Remote ID contact and a
                    // DJI-branded access point.
                    if track.identified() {
                        ""
                    } else {
                        "  (not identified)"
                    }
                )?;
            }
        }
        None => writeln!(out, "  tracks   unavailable")?,
    }

    match &snapshot.detections {
        Some(page) => {
            writeln!(out, "  detects  {} total", page.total)?;
            // Folded exactly as the pane folds them, so the two views cannot
            // disagree about what happened. One aeroplane overhead otherwise
            // fills this list too.
            for (detection, run) in collapse_runs(&page.detections) {
                let rf = detection.rf.clone().unwrap_or_default();
                let label = match run {
                    0 | 1 => detection.label(),
                    n => format!("{} x{n}", detection.label()),
                };
                writeln!(
                    out,
                    "    {:<5} {:<15} {:<7} {:<6} {label}",
                    detection.sensor_kind,
                    class_label(&detection.detection_class),
                    rf.rssi_dbm
                        .map(|r| format!("{r:.0}dBm"))
                        .unwrap_or_default(),
                    rf.tuning().unwrap_or_default(),
                )?;
            }
        }
        None => writeln!(out, "  detects  unavailable")?,
    }
    Ok(())
}

/// `A Remote ID`, or a bare letter for a class this build has never heard of.
///
/// The letter always survives. It is what the API actually said, and a label
/// this binary is too old to know is not a reason to print nothing.
fn class_label(code: &str) -> String {
    match detection_class_label(code) {
        "" => code.to_string(),
        label => format!("{code} {label}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_renders_without_a_terminal_and_without_an_api() {
        let config = Config {
            api: "http://127.0.0.1:1".to_string(),
            ..Config::default()
        };
        let mut buffer: Vec<u8> = Vec::new();
        print_once(&config, &mut buffer).expect("snapshot must not fail");
        let text = String::from_utf8(buffer).expect("utf-8");
        assert!(text.contains("system"));
        assert!(text.contains("health"));
        assert!(text.contains("radios"));
        assert!(text.contains("classg"));
        // A dead API degrades into a hint, it does not abort the snapshot.
        assert!(text.contains("not reachable"), "{text}");
        assert!(text.contains("make dev"));
    }

    use crate::panes::classg::{
        Adsb, AuthState, Capture, CapturePage, CredentialKind, Detection, DetectionPage, Evidence,
        FusionHealth, HealthResponse, Identity, MonitoringState, Rf, SensorHealth, Slow,
        SpectrumSweep, SweepPage, SystemBuild, SystemHost, SystemInfo, SystemRuntime, Track,
        TrackPage,
    };

    /// Renders just the ClassG section, which is the half of `--once` that is
    /// not simply a transcription of a struct.
    fn classg(snapshot: &Snapshot) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        print_classg(snapshot, &mut buffer).expect("rendering must not fail");
        String::from_utf8(buffer).expect("utf-8")
    }

    fn working() -> Snapshot {
        Snapshot {
            health: Some(HealthResponse {
                status: "ok".to_string(),
                uptime_s: 8040,
                version: "0.4.1".to_string(),
                sensors: vec![SensorHealth {
                    sensor_id: "wifi-1".to_string(),
                    sensor_kind: "wifi".to_string(),
                    healthy: true,
                    seconds_since_heartbeat: Some(2),
                    detections_5m: 1284,
                    ..SensorHealth::default()
                }],
                fusion: Some(FusionHealth {
                    configured: true,
                    connected: true,
                    ..FusionHealth::default()
                }),
            }),
            monitoring: Some(MonitoringState {
                enabled: true,
                ..MonitoringState::default()
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn an_api_that_is_not_up_says_where_it_looked_and_how_to_start_it() {
        let text = classg(&Snapshot {
            error: Some("Connection refused (os error 111)".to_string()),
            ..Snapshot::default()
        });
        assert!(text.contains("not reachable"), "{text}");
        assert!(text.contains("Connection refused"), "{text}");
        assert!(text.contains("make dev"), "{text}");
        // Nothing else is invented for a unit that never answered.
        assert!(!text.contains("sensor"), "{text}");
    }

    #[test]
    fn a_paused_recording_is_stated_before_anything_it_explains() {
        // Every sensor healthy, fusion connected, nothing tracked -- which is
        // line for line what a quiet sky looks like.
        let mut snapshot = working();
        snapshot.monitoring = Some(MonitoringState {
            enabled: false,
            reason: Some("known local flight".to_string()),
            discarded: 1204,
            ..MonitoringState::default()
        });
        snapshot.tracks = Some(TrackPage::default());
        let text = classg(&snapshot);

        assert!(text.contains("PAUSED"), "{text}");
        assert!(text.contains("1204 discarded"), "{text}");
        assert!(text.contains("known local flight"), "{text}");

        let record = text.find("PAUSED").expect("a record line");
        let tracks = text.find("tracks").expect("a tracks line");
        assert!(record < tracks, "the pause must precede what it explains");
    }

    #[test]
    fn the_build_from_system_beats_the_bare_version_from_health() {
        // A revision is what answers "is this the binary I deployed"; /health
        // knows only a version string.
        let mut snapshot = working();
        snapshot.slow.system = Some(SystemInfo {
            build: SystemBuild {
                version: "0.4.1".to_string(),
                revision: Some("a1b2c3d4e5f6".to_string()),
                revision_dirty: true,
            },
            runtime: SystemRuntime {
                store: "libsql".to_string(),
                ..SystemRuntime::default()
            },
            host: SystemHost {
                disk_path: "/var/lib/classg".to_string(),
                disk_total_bytes: Some(31_000_000_000),
                disk_free_bytes: Some(2_100_000_000),
            },
        });
        let text = classg(&snapshot);
        assert!(text.contains("0.4.1+a1b2c3d-dirty"), "{text}");
        assert!(text.contains("libsql"), "{text}");
        assert!(text.contains("/var/lib/classg"), "{text}");
        assert!(text.contains("free of"), "{text}");

        // Without /system the version still appears rather than a blank.
        let mut bare = working();
        bare.slow = Slow::default();
        assert!(classg(&bare).contains("0.4.1"));
    }

    #[test]
    fn a_disk_the_api_could_not_measure_says_so_rather_than_showing_zero() {
        let mut snapshot = working();
        snapshot.slow.system = Some(SystemInfo {
            host: SystemHost {
                disk_path: "/".to_string(),
                disk_total_bytes: None,
                disk_free_bytes: None,
            },
            ..SystemInfo::default()
        });
        let text = classg(&snapshot);
        assert!(text.contains("unavailable"), "{text}");
        assert!(!text.contains("0 free of 0"), "{text}");
    }

    #[test]
    fn a_refusal_names_the_remedy_for_the_credential_that_was_actually_sent() {
        // One sentence for all three cases was wrong in two of them.
        let cases = [
            (Some(CredentialKind::Local), "rewrites it on restart"),
            (Some(CredentialKind::Session), "CLASSG_SESSION has expired"),
            (None, ".agent-state"),
        ];
        for (credential, expected) in cases {
            let mut snapshot = working();
            snapshot.credential = credential;
            snapshot.denied = Some("log in to continue".to_string());
            snapshot.slow.auth = Some(AuthState {
                auth_enabled: true,
                authenticated: false,
                ..AuthState::default()
            });
            let text = classg(&snapshot);
            assert!(text.contains("log in to continue"), "{text}");
            assert!(text.contains(expected), "for {credential:?}:\n{text}");
        }
    }

    #[test]
    fn a_sensor_that_was_never_fitted_reads_differently_from_one_that_broke() {
        let mut snapshot = working();
        if let Some(api) = snapshot.health.as_mut() {
            api.sensors.push(SensorHealth {
                sensor_id: "sdr-1".to_string(),
                healthy: false,
                reason: Some("rtl_sdr: device not found".to_string()),
                ..SensorHealth::default()
            });
            api.sensors.push(SensorHealth {
                sensor_id: "ble-1".to_string(),
                healthy: false,
                optional: true,
                reason: Some("not fitted".to_string()),
                ..SensorHealth::default()
            });
        }
        let text = classg(&snapshot);
        assert!(text.contains("DOWN"), "{text}");
        assert!(text.contains("rtl_sdr: device not found"), "{text}");
        // Lower case, and not DOWN: a build you chose is not a fault.
        let ble = text
            .lines()
            .find(|l| l.contains("ble-1"))
            .expect("the optional sensor is still listed");
        assert!(ble.contains("off"), "{ble}");
        assert!(!ble.contains("DOWN"), "{ble}");
    }

    #[test]
    fn a_radio_held_by_a_capture_or_a_sweep_is_reported() {
        let mut snapshot = working();
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
        let text = classg(&snapshot);
        assert!(text.contains("wlan1 ch6"), "{text}");
        assert!(text.contains("radio busy"), "{text}");
    }

    #[test]
    fn a_failed_capture_says_why_rather_than_only_that_it_did() {
        let mut snapshot = working();
        snapshot.slow.captures = Some(CapturePage {
            captures: vec![Capture {
                state: "failed".to_string(),
                error: Some("tcpdump: wlan1: No such device".to_string()),
                ..Capture::default()
            }],
        });
        assert!(classg(&snapshot).contains("No such device"));
    }

    #[test]
    fn a_track_nothing_identified_is_marked_as_such() {
        let mut snapshot = working();
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
        let text = classg(&snapshot);
        assert!(text.contains("(not identified)"), "{text}");
        assert!(text.contains("140x"), "{text}");
    }

    #[test]
    fn detections_are_named_folded_and_tuned_the_way_the_pane_shows_them() {
        let mut snapshot = working();
        let mut detections: Vec<Detection> = (0..4)
            .map(|_| Detection {
                sensor_kind: "sdr".to_string(),
                sensor_id: "sdr-0".to_string(),
                detection_class: "D".to_string(),
                rf: Some(Rf {
                    freq_hz: Some(1_090_000_000),
                    ..Rf::default()
                }),
                adsb: Some(Adsb {
                    icao: "a9d770".to_string(),
                    callsign: Some("N172SP".to_string()),
                }),
                ..Detection::default()
            })
            .collect();
        detections.push(Detection {
            sensor_kind: "wifi".to_string(),
            detection_class: "A".to_string(),
            rf: Some(Rf {
                channel: Some(149),
                rssi_dbm: Some(-52.0),
                ..Rf::default()
            }),
            ..Detection::default()
        });
        snapshot.detections = Some(DetectionPage {
            detections,
            total: 15370,
        });
        let text = classg(&snapshot);

        // The class is named, not lettered -- with the letter kept, because
        // this view has the room the pane does not.
        assert!(text.contains("A Remote ID"), "{text}");
        assert!(text.contains("D ADS-B"), "{text}");
        // Megahertz, not a rounded gigahertz that loses the band.
        assert!(text.contains("1090M"), "{text}");
        assert!(text.contains("ch149"), "{text}");
        // Folded exactly as the pane folds, so the two cannot disagree.
        assert!(text.contains("N172SP x4"), "{text}");
        // And the true total survives the folding.
        assert!(text.contains("15370 total"), "{text}");
    }

    #[test]
    fn a_class_this_build_has_never_heard_of_still_shows_its_letter() {
        let mut snapshot = working();
        snapshot.detections = Some(DetectionPage {
            detections: vec![Detection {
                sensor_kind: "sdr".to_string(),
                detection_class: "Z".to_string(),
                ..Detection::default()
            }],
            total: 1,
        });
        let text = classg(&snapshot);
        // What the API actually said. A label this binary is too old to know
        // is not a reason to print nothing.
        assert!(text.contains(" Z "), "{text}");
    }

    #[test]
    fn a_section_the_api_would_not_serve_says_unavailable_not_empty() {
        // tracks: None is "we could not ask", which is a different fact from
        // "nothing is flying" and must not render as the same line.
        let mut snapshot = working();
        snapshot.tracks = None;
        snapshot.detections = None;
        let text = classg(&snapshot);
        assert!(text.contains("tracks   unavailable"), "{text}");
        assert!(text.contains("detects  unavailable"), "{text}");
    }

    #[test]
    fn throttle_description_distinguishes_unknown_from_clean() {
        assert_eq!(describe_throttle(None), "unknown - no vcgencmd here");
        assert_eq!(
            describe_throttle(Some(Throttle::decode(0))),
            "ok, clean since boot"
        );
        assert_eq!(
            describe_throttle(Some(Throttle::decode(0x50001))),
            "now [UNDER-VOLTAGE NOW]  since boot [under-voltage, throttled]  (0x50001)"
        );
        // A sticky-only register must not read as if it were happening now.
        assert_eq!(
            describe_throttle(Some(Throttle::decode(0x50000))),
            "now [-]  since boot [under-voltage, throttled]  (0x50000)"
        );
    }
}
