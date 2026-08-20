//! `pi-dash --check`: one verdict and an exit code, for cron and CI.
//!
//! `--once` prints everything and always exits 0, because a snapshot's job is
//! to render and it rendered. That makes it useless as a monitoring probe: the
//! README has always advertised it for cron and a CI step, and neither of those
//! can act on a wall of text. This is the other half — one line, and a status
//! the shell can branch on.
//!
//! What it reports on is what this dashboard can see, which is deliberately not
//! the same as what `/health` alone says. A ClassG API can be perfectly healthy
//! on a Pi that is browning out, thermally throttled, or three days from a full
//! card, and each of those takes the detector down eventually. The rule is the
//! one the panes follow: never invent a verdict, and never stay silent about a
//! condition that will end in a detector that has stopped detecting.
//!
//! Silence is not success here. The verdict line is always printed, so that
//! `pi-dash --check` run by hand says something, and a cron job that only wants
//! mail on failure redirects stdout. That is one documented redirect against a
//! command that otherwise appears to do nothing.

use std::fmt::Write as _;
use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;

use crate::config::Config;
use crate::panes::classg::{Client, Slow};
use crate::panes::health::{HealthPane, Tense};

/// The three states, worst wins.
///
/// Ordered so the derived `Ord` is the severity order, which is what lets a
/// list of findings collapse to one verdict without a hand-written table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum Verdict {
    #[default]
    Ok,
    /// Working, but something here ends badly if it is left alone.
    Degraded,
    /// Not detecting. The API is unreachable, or says so itself.
    Down,
}

impl Verdict {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::Degraded => "degraded",
            Verdict::Down => "down",
        }
    }

    /// 0, 1, 2. Chosen so `--check && echo fine` does the obvious thing and a
    /// caller that only cares whether anything is wrong can test for non-zero.
    pub(crate) fn code(self) -> ExitCode {
        match self {
            Verdict::Ok => ExitCode::from(0),
            Verdict::Degraded => ExitCode::from(1),
            Verdict::Down => ExitCode::from(2),
        }
    }
}

/// One finding: how bad, and what to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) verdict: Verdict,
    pub(crate) note: String,
}

/// Reads the box once and reports what is wrong with it.
///
/// No settling pause. `--once` waits 700 ms because every rate it prints is a
/// difference between two readings; nothing judged here is a rate, so a probe
/// that runs from cron every minute does not pay for one.
pub(crate) fn run(config: &Config, out: &mut impl Write) -> Result<ExitCode> {
    let mut health = HealthPane::default();
    health.sample(Instant::now());

    let client = Client::new(config.api.clone(), config.credential());
    let snapshot = client.poll(1, 1, true, &Slow::default());

    let findings = judge(&health, &snapshot);
    let verdict = findings
        .iter()
        .map(|f| f.verdict)
        .max()
        .unwrap_or(Verdict::Ok);

    writeln!(out, "{}", summarise(verdict, &findings))?;
    Ok(verdict.code())
}

/// `degraded: recording paused; sdr-0 is down` — the verdict, then every
/// finding that produced it, in severity order.
///
/// Findings at a lower severity than the verdict are still listed. A unit that
/// is down AND browning out has two problems, and the one that gets fixed is
/// the one that got printed.
pub(crate) fn summarise(verdict: Verdict, findings: &[Finding]) -> String {
    let mut line = verdict.label().to_string();
    let mut ordered: Vec<&Finding> = findings.iter().collect();
    // Worst first, and stable, so findings of equal severity stay in the order
    // they were gathered — the API's own verdict ahead of the sensors that
    // explain it.
    ordered.sort_by_key(|f| std::cmp::Reverse(f.verdict));

    for (index, finding) in ordered.iter().enumerate() {
        let separator = if index == 0 { ": " } else { "; " };
        // Infallible on a String; the result is discarded rather than
        // unwrapped so this cannot become the one panic in the binary.
        let _ = write!(line, "{separator}{}", finding.note);
    }
    line
}

/// Every condition worth waking somebody for, gathered from one sample.
///
/// Split from [`run`] so the whole judgement is testable without a Pi, an API,
/// or a clock.
pub(crate) fn judge(
    health: &HealthPane,
    snapshot: &crate::panes::classg::Snapshot,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut add = |verdict: Verdict, note: String| findings.push(Finding { verdict, note });

    // ── the detector ──
    match &snapshot.health {
        // The transport error already names the URL it failed to reach, so
        // this does not repeat it.
        None => add(
            Verdict::Down,
            format!(
                "the API is not answering: {}",
                snapshot.error.as_deref().unwrap_or("no reason given")
            ),
        ),
        Some(api) => {
            match api.status.as_str() {
                "ok" | "healthy" => {}
                "degraded" => add(Verdict::Degraded, "the API reports degraded".to_string()),
                // Anything unrecognised is treated as down rather than
                // ignored: a status this build has never heard of is not a
                // reason to report a unit as healthy.
                other => add(
                    Verdict::Down,
                    format!(
                        "the API reports {}",
                        if other.is_empty() { "no status" } else { other }
                    ),
                ),
            }
            // Named individually, because "degraded" does not say which radio.
            // Optional hardware that was never fitted is a build, not a fault
            // -- the same rule /health itself applies.
            for sensor in &api.sensors {
                if !sensor.healthy && !sensor.optional {
                    let reason = sensor
                        .reason
                        .as_deref()
                        .filter(|r| !r.is_empty())
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default();
                    add(
                        Verdict::Degraded,
                        format!("sensor {} is down{reason}", sensor.sensor_id),
                    );
                }
            }
        }
    }

    // A paused unit is the failure this whole dashboard is shaped around: it
    // looks exactly like a quiet sky and it is not one.
    if let Some(state) = &snapshot.monitoring {
        if !state.enabled {
            let reason = state
                .reason
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            add(
                Verdict::Degraded,
                format!(
                    "recording is paused{reason}, {} detections discarded",
                    state.discarded
                ),
            );
        }
    }

    if let Some(denied) = &snapshot.denied {
        add(Verdict::Degraded, format!("the API refused us: {denied}"));
    }

    // ── the Pi underneath ──
    //
    // Only the live register, never the sticky one. A brownout at three in the
    // morning is real history and belongs on the pane, but a probe that fails
    // for ever afterwards is a probe somebody silences.
    if let Some(throttle) = health.throttle {
        if throttle.now.any() {
            add(
                Verdict::Degraded,
                format!("the Pi is {}", throttle.now.labels(Tense::Now).join(", ")),
            );
        }
    }

    if let Some(disk) = health.disk {
        let pct = disk.pct();
        if pct >= DISK_FULL_PCT {
            add(Verdict::Degraded, format!("the disk is {pct:.0}% full"));
        }
    }

    findings
}

/// Where a disk stops being something to watch and starts being something that
/// will end a capture mid-write.
const DISK_FULL_PCT: f64 = 90.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::classg::{HealthResponse, MonitoringState, SensorHealth, Snapshot};
    use crate::panes::health::{DiskUsage, Throttle};

    fn healthy_api() -> Snapshot {
        Snapshot {
            health: Some(HealthResponse {
                status: "ok".to_string(),
                sensors: vec![SensorHealth {
                    sensor_id: "wifi-1".to_string(),
                    healthy: true,
                    ..SensorHealth::default()
                }],
                ..HealthResponse::default()
            }),
            monitoring: Some(MonitoringState {
                enabled: true,
                ..MonitoringState::default()
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_working_unit_has_nothing_to_say() {
        let findings = judge(&HealthPane::default(), &healthy_api());
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(summarise(Verdict::Ok, &findings), "ok");
    }

    #[test]
    fn an_unreachable_api_is_down_not_degraded() {
        let snapshot = Snapshot {
            error: Some("connection refused".to_string()),
            ..Snapshot::default()
        };
        let findings = judge(&HealthPane::default(), &snapshot);
        assert_eq!(findings[0].verdict, Verdict::Down);
        assert!(findings[0].note.contains("connection refused"));
    }

    #[test]
    fn a_status_this_build_has_never_heard_of_is_not_treated_as_healthy() {
        // The API growing a status is not a reason to report a unit as fine.
        let mut snapshot = healthy_api();
        if let Some(api) = snapshot.health.as_mut() {
            api.status = "reconfiguring".to_string();
        }
        let findings = judge(&HealthPane::default(), &snapshot);
        assert_eq!(findings[0].verdict, Verdict::Down);
        assert!(findings[0].note.contains("reconfiguring"));
    }

    #[test]
    fn a_paused_recording_fails_the_check() {
        // The whole point. Every sensor healthy, fusion fine, nothing flying —
        // and nothing being written down either.
        let mut snapshot = healthy_api();
        snapshot.monitoring = Some(MonitoringState {
            enabled: false,
            reason: Some("known local flight".to_string()),
            discarded: 1204,
            ..MonitoringState::default()
        });
        let findings = judge(&HealthPane::default(), &snapshot);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].verdict, Verdict::Degraded);
        assert!(
            findings[0].note.contains("known local flight"),
            "{:?}",
            findings[0]
        );
        assert!(findings[0].note.contains("1204"));
    }

    #[test]
    fn a_sensor_that_is_down_is_named_because_degraded_does_not_say_which() {
        let mut snapshot = healthy_api();
        if let Some(api) = snapshot.health.as_mut() {
            api.status = "degraded".to_string();
            api.sensors.push(SensorHealth {
                sensor_id: "sdr-1".to_string(),
                healthy: false,
                reason: Some("rtl_sdr: device not found".to_string()),
                ..SensorHealth::default()
            });
        }
        let findings = judge(&HealthPane::default(), &snapshot);
        let line = summarise(Verdict::Degraded, &findings);
        assert!(line.contains("sdr-1"), "{line}");
        assert!(line.contains("rtl_sdr: device not found"), "{line}");
    }

    #[test]
    fn optional_hardware_that_was_never_fitted_is_not_a_fault() {
        // A Pi built without an SDR must not fail a check for ever, or the
        // check is one somebody turns off.
        let mut snapshot = healthy_api();
        if let Some(api) = snapshot.health.as_mut() {
            api.sensors.push(SensorHealth {
                sensor_id: "ble-1".to_string(),
                healthy: false,
                optional: true,
                reason: Some("not fitted".to_string()),
                ..SensorHealth::default()
            });
        }
        assert!(judge(&HealthPane::default(), &snapshot).is_empty());
    }

    #[test]
    fn a_live_brownout_fails_but_an_old_one_does_not() {
        let mut health = HealthPane::default();
        // Sticky bits only: this happened, and is not happening.
        health.throttle = Some(Throttle::decode(0x50000));
        assert!(judge(&health, &healthy_api()).is_empty());

        // Live under-voltage, which drops USB radios while you are watching.
        health.throttle = Some(Throttle::decode(0x50001));
        let findings = judge(&health, &healthy_api());
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].note.contains("UNDER-VOLTAGE NOW"),
            "{:?}",
            findings[0]
        );
    }

    #[test]
    fn a_disk_about_to_end_a_capture_mid_write_fails() {
        let mut health = HealthPane::default();
        health.disk = Some(DiskUsage {
            used_kb: 95,
            avail_kb: 5,
            total_kb: 100,
        });
        let findings = judge(&health, &healthy_api());
        assert_eq!(findings[0].verdict, Verdict::Degraded);
        assert!(findings[0].note.contains("95% full"), "{:?}", findings[0]);

        // Half full is not news.
        health.disk = Some(DiskUsage {
            used_kb: 50,
            avail_kb: 50,
            total_kb: 100,
        });
        assert!(judge(&health, &healthy_api()).is_empty());
    }

    #[test]
    fn the_worst_finding_decides_the_verdict_and_the_rest_are_still_printed() {
        // A unit that is down AND browning out has two problems, and the one
        // that gets fixed is the one that got printed.
        let mut health = HealthPane::default();
        health.throttle = Some(Throttle::decode(0x50001));
        let snapshot = Snapshot {
            error: Some("connection refused".to_string()),
            ..Snapshot::default()
        };
        let findings = judge(&health, &snapshot);
        let verdict = findings.iter().map(|f| f.verdict).max().expect("findings");
        assert_eq!(verdict, Verdict::Down);

        let line = summarise(verdict, &findings);
        assert!(line.starts_with("down: "), "{line}");
        assert!(line.contains("connection refused"), "{line}");
        assert!(line.contains("UNDER-VOLTAGE NOW"), "{line}");
    }

    #[test]
    fn the_exit_codes_are_the_ones_a_shell_can_branch_on() {
        assert_eq!(
            format!("{:?}", Verdict::Ok.code()),
            format!("{:?}", ExitCode::from(0))
        );
        assert_eq!(
            format!("{:?}", Verdict::Degraded.code()),
            format!("{:?}", ExitCode::from(1))
        );
        assert_eq!(
            format!("{:?}", Verdict::Down.code()),
            format!("{:?}", ExitCode::from(2))
        );
        // Severity ordering is what collapses a list of findings to one
        // verdict, so it has to hold.
        assert!(Verdict::Down > Verdict::Degraded);
        assert!(Verdict::Degraded > Verdict::Ok);
    }

    #[test]
    fn the_check_runs_end_to_end_without_an_api_and_reports_it() {
        let config = Config {
            api: "http://127.0.0.1:1".to_string(),
            ..Config::default()
        };
        let mut buffer: Vec<u8> = Vec::new();
        let code = run(&config, &mut buffer).expect("check must not fail");
        let text = String::from_utf8(buffer).expect("utf-8");
        assert!(text.starts_with("down: "), "{text}");
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(2)));
    }
}
