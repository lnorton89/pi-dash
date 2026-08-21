//! The ClassG pane.
//!
//! The pane answers one question in the order an operator actually asks it:
//! is the box recording, are the sensors alive, is fusion producing tracks,
//! is anything holding a radio, and only then what is in the sky. Everything
//! above the track list is a reason the track list might be empty, which is
//! why it is above the track list — an empty list is the single most ambiguous
//! thing this dashboard can draw, and every line before it removes one way of
//! misreading it.
//!
//! Rows are scarce. This pane is handed whatever the two fixed-height panes
//! above it did not use, so every section is written to be dropped: the ones
//! that cost nothing when there is nothing to say (radio activity, a pause
//! reason, a login banner) render zero lines in the ordinary case, and the two
//! lists shrink to fit whatever is left.

use chrono::Local;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use super::{numbered_pane_block, push_if_fits, table_header, BAD, DIM, GUTTER, OK, WARN};
use crate::app::{App, Pane};
use crate::format::{clip, coarse_uptime, compact_count, human_bytes, short_age};
use crate::panes::classg::{
    collapse_runs, detection_class_label, Capture, CredentialKind, Detection, DetectionPage,
    FlexTime, HealthResponse, Snapshot, SpectrumSweep, Track, TrackPage, MAX_DETECTION_ROWS,
};

/// Below this the detections table drops its SENSOR column. On a unit with one
/// Wi-Fi sensor the column is the same word on every row; on one with two it is
/// the difference between "the radios are fine" and "one of them stopped", so
/// it is the first thing to come back when there is room for it.
const SENSOR_COLUMN_AT: usize = 54;

pub(crate) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" ClassG  {} ", app.config.api.trim_start_matches("http://"));
    let block = numbered_pane_block(
        Pane::Classg,
        &title,
        app.accent,
        app.glyphs,
        app.focus == Pane::Classg,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let width = inner.width as usize;

    let snapshot = &app.classg.snapshot;
    let mut lines: Vec<Line> = Vec::new();

    let Some(health) = &snapshot.health else {
        lines.extend(unreachable_lines(
            snapshot.error.as_deref(),
            &app.config.api,
        ));
        // The only wrapped paragraph in the dashboard. A transport error is
        // the one string here whose length is not ours to control, and
        // truncating it at the pane edge hides the half that says *why* —
        // "Connection refused" versus "No route to host" is the whole
        // diagnosis.
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        // Nothing else is being rendered, so ask the poller for the minimum. It
        // clamps to 1 anyway; this just stops a failed pane from requesting
        // forty rows it has nowhere to put.
        app.classg.set_hints(1, 1);
        return;
    };

    lines.extend(status_lines(snapshot, health, width));
    lines.extend(sensor_lines(health, width));
    lines.push(Line::default());
    lines.push(fusion_line(health, width));
    lines.extend(radio_lines(snapshot, width));

    // Everything above is what you cannot afford to lose, so the lists are what
    // gets dropped when the pane is short: rendering eleven lines into seven
    // rows scrolls the recording state and the sensor verdict off the top and
    // the pane silently becomes a track list.
    let mut track_room = 0usize;
    let mut detection_room = 0usize;

    // Two rows of overhead per section: the blank spacer and the heading.
    let room = |used: usize| (inner.height as usize).saturating_sub(used + 2);

    if room(lines.len()) >= 1 {
        track_room = room(lines.len());
        lines.extend(track_lines(snapshot.tracks.as_ref(), track_room, width));
    }
    if room(lines.len()) >= 1 {
        detection_room = room(lines.len());
        lines.extend(detection_lines(
            snapshot.detections.as_ref(),
            detection_room,
            width,
        ));
    }

    // Ask for what will fit next time. This pane is handed the slack left over
    // from the two fixed-height ones, so on a tall terminal that is a real
    // list rather than the first three rows of one.
    app.classg.set_hints(
        track_room.max(1),
        detection_request(app.classg.snapshot.detections.as_ref(), detection_room),
    );

    frame.render_widget(Paragraph::new(lines), inner);
}

/// An API that is not up is a normal state on a bare Pi, not an error worth a
/// stack trace. Say where it looked and how to start it.
fn unreachable_lines<'a>(error: Option<&str>, base: &str) -> Vec<Line<'a>> {
    let mut lines = vec![Line::from(vec![
        Span::styled("  not reachable", Style::default().fg(WARN)),
        Span::styled(format!(" at {base}"), Style::default().fg(DIM)),
    ])];
    match error {
        Some(error) => lines.push(Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(DIM),
        ))),
        None => lines.push(Line::from(Span::styled(
            "  connecting…",
            Style::default().fg(DIM),
        ))),
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  start it with: make dev   (or set CLASSG_API)",
        Style::default().fg(DIM),
    )));
    lines
}

/// A label in the gutter every pane shares, so values line up down the column.
fn labelled<'a>(label: &str, value: Vec<Span<'a>>) -> Line<'a> {
    let mut spans = vec![Span::styled(
        format!("  {label:<10}"),
        Style::default().fg(DIM),
    )];
    spans.extend(value);
    Line::from(spans)
}

fn dim<'a>(text: String) -> Span<'a> {
    Span::styled(text, Style::default().fg(DIM))
}

// ---------------------------------------------------------------------------
// What the unit is, and whether it is recording
// ---------------------------------------------------------------------------

fn status_lines<'a>(snapshot: &Snapshot, health: &HealthResponse, width: usize) -> Vec<Line<'a>> {
    let status_color = match health.status.as_str() {
        "ok" | "healthy" => OK,
        "degraded" => WARN,
        _ => BAD,
    };

    // /system knows the git revision; /health only knows the version string.
    // The revision is what answers "is this the build I deployed", so it wins
    // whenever the slow tier has answered.
    let build = snapshot
        .slow
        .system
        .as_ref()
        .map(|system| system.build_label())
        .unwrap_or_else(|| health.version.clone());

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("  {}", health.status),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
        dim(format!(
            "   up {}   {}",
            coarse_uptime(health.uptime_s),
            clip(&build, width.saturating_sub(24))
        )),
    ])];

    if let Some(system) = &snapshot.slow.system {
        // Disk on the filesystem detections land on, which is not necessarily
        // the one the Pi-health pane measured: the store can sit on a USB
        // stick while `/` has plenty of room.
        let mut detail = system.runtime.store.clone();
        if system.runtime.containerised {
            detail.push_str(" · docker");
        }
        if system.runtime.turso_sync_configured {
            detail.push_str(" · sync");
        }
        // Two spaces after the pad, not a pad alone. `{:<12}` adds nothing
        // once the text is already twelve wide, and on a unit that is
        // containerised AND syncing the detail reads
        // "libsql · docker · sync" -- which ran straight into the figure
        // after it as `sync88.3G free of 117.0G`.
        let detail = format!("{:<10}  ", detail);
        let mut used = GUTTER + 1 + detail.chars().count();
        let mut spans = vec![Span::raw(detail)];
        match (system.host.disk_free_bytes, system.host.disk_total_bytes) {
            (Some(free), Some(total)) => {
                // Amber under a tenth left: a full store stops recording, and
                // the pane exists to say that before it happens.
                let color = if total > 0 && free * 10 < total {
                    WARN
                } else {
                    DIM
                };
                let free_text = format!("{} free", human_bytes(free));
                let full = format!("{free_text} of {}", human_bytes(total));
                // "11.5G free of 28.9G" sliced at the pane edge became
                // "11.5G free of 28", which is a different and much worse
                // number. Offer the short form rather than half the long one.
                let text = if used + full.chars().count() <= width {
                    full
                } else {
                    free_text
                };
                used += text.chars().count();
                spans.push(Span::styled(text, Style::default().fg(color)));
            }
            // Null with a reason, never a zero — a disk reported as 0 bytes
            // free reads as an emergency that is not happening.
            _ => spans.push(dim("disk unreadable".to_string())),
        }
        let _ = used;
        lines.push(labelled("store", spans));
    }

    if let Some(state) = &snapshot.monitoring {
        lines.push(if state.enabled {
            labelled(
                "recording",
                vec![
                    Span::styled("on", Style::default().fg(OK)),
                    dim(match state.since.as_ref() {
                        Some(ts) => format!("   since {} ago", short_age(ts.age_secs())),
                        None => String::new(),
                    }),
                ],
            )
        } else {
            labelled(
                "recording",
                vec![
                    Span::styled(
                        "PAUSED",
                        Style::default().fg(BAD).add_modifier(Modifier::BOLD),
                    ),
                    // The count is the whole point: a pause with detections
                    // piling up behind it is a pause somebody forgot about.
                    dim(format!(
                        "  {} discarded{}",
                        compact_count(state.discarded),
                        state
                            .since
                            .as_ref()
                            .map(|ts| format!(", {}", short_age(ts.age_secs())))
                            .unwrap_or_default()
                    )),
                ],
            )
        });
        if let Some(reason) = state.reason.as_deref().filter(|r| !r.is_empty()) {
            if !state.enabled {
                lines.push(Line::from(dim(format!(
                    "            {}",
                    clip(reason, width.saturating_sub(13))
                ))));
            }
        }
    }

    // Only when authentication is switched on. On a loopback unit with it off
    // — the common case — a line saying so every frame is noise.
    if let Some(auth) = snapshot.slow.auth.as_ref().filter(|a| a.auth_enabled) {
        lines.push(match (&auth.user, auth.setup_required) {
            (_, true) => labelled(
                "session",
                vec![Span::styled(
                    "this unit has no accounts yet",
                    Style::default().fg(WARN),
                )],
            ),
            (Some(user), _) => labelled(
                "session",
                vec![
                    Span::styled(user.username.clone(), Style::default().fg(OK)),
                    dim(format!("  {}", user.role)),
                ],
            ),
            (None, _) => {
                let (state, detail) = remedy(snapshot.credential);
                let mut spans = vec![Span::styled(state, Style::default().fg(WARN))];
                let mut used = GUTTER + 1 + state.len();
                // The detail is the first thing to drop. "not logged in" plus a
                // sentence was wider than a 46-column pane and got sliced at
                // the frame, which turned advice into a fragment.
                push_if_fits(&mut spans, &mut used, width, format!("  {detail}"));
                labelled("session", spans)
            }
        });
    }

    // Said once, at the top. Three sections each drawing "log in to continue"
    // over an empty list explains it no better and costs three rows.
    if let Some(denied) = &snapshot.denied {
        lines.push(labelled(
            "refused",
            vec![Span::styled(
                clip(denied, width.saturating_sub(13)),
                Style::default().fg(WARN),
            )],
        ));
    }

    lines.push(Line::default());
    lines
}

// ---------------------------------------------------------------------------
// Sensors and fusion
// ---------------------------------------------------------------------------

fn sensor_lines<'a>(health: &HealthResponse, width: usize) -> Vec<Line<'a>> {
    let mut lines = vec![Line::from(Span::styled(
        "  sensors",
        Style::default().add_modifier(Modifier::BOLD),
    ))];

    if health.sensors.is_empty() {
        // Not "quiet": an API that knows about no sensors has no basis to
        // claim anything about the sky, which is why /health calls this down.
        lines.push(Line::from(dim("   no sensors reporting".to_string())));
        return lines;
    }
    // BEAT is the age of the last heartbeat and 5MIN the detection count over
    // the last five minutes. Unheaded they rendered as `5s 5m:40`, which is
    // two numbers and a unit nobody can expand on sight.
    lines.push(table_header(format!(
        "   {:<11}{:<6}{:<6}{:>5}{:>7}",
        "SENSOR", "KIND", "STATE", "BEAT", "5MIN"
    )));

    for sensor in &health.sensors {
        // An optional sensor that is down is a configuration you chose, not a
        // fault — the SDR is optional on a Wi-Fi-only build. It gets amber and
        // lower case so a genuinely broken required sensor still stands out.
        let (mark, color) = match (sensor.healthy, sensor.optional) {
            (true, _) => ("ok", OK),
            (false, true) => ("off", WARN),
            (false, false) => ("DOWN", BAD),
        };
        lines.push(Line::from(vec![
            Span::raw(format!("   {:<11}", clip(&sensor.sensor_id, 10))),
            dim(format!("{:<6}", clip(&sensor.sensor_kind, 5))),
            Span::styled(format!("{mark:<6}"), Style::default().fg(color)),
            Span::styled(
                format!(
                    "{:>5}",
                    sensor
                        .seconds_since_heartbeat
                        .map(|s| format!("{s}s"))
                        .unwrap_or_else(|| "-".to_string())
                ),
                // A heartbeat that has stopped is the thing this column is
                // for, so it stops being dim once it is older than a poll or
                // two. The API's own health check is slower than that.
                Style::default().fg(match sensor.seconds_since_heartbeat {
                    Some(s) if s > 30 => WARN,
                    _ => DIM,
                }),
            ),
            dim(format!("{:>7}", compact_count(sensor.detections_5m))),
        ]));
        // The reason a sensor is down is the whole point of looking at this
        // pane, so it gets its own line rather than being truncated into the
        // margin.
        if let Some(reason) = sensor.reason.as_deref().filter(|r| !r.is_empty()) {
            lines.push(Line::from(Span::styled(
                format!("     {}", clip(reason, width.saturating_sub(6))),
                Style::default().fg(WARN),
            )));
        }
    }
    lines
}

fn fusion_line<'a>(health: &HealthResponse, width: usize) -> Line<'a> {
    let fusion = health.fusion.clone().unwrap_or_default();
    if fusion.connected {
        return labelled(
            "fusion",
            vec![
                Span::styled("connected", Style::default().fg(OK)),
                // "last -" said nothing. The age is of the last message off the
                // link, which is the only thing that distinguishes a live
                // connection from a socket nobody has written to since boot.
                dim(match fusion.last_message.as_ref() {
                    Some(ts) => format!("   last message {}", short_age(ts.age_secs())),
                    None => "   no messages yet".to_string(),
                }),
            ],
        );
    }
    if fusion.configured {
        // Capped at degraded on the API side for the same reason it is red
        // here: every sensor can heartbeat happily while nothing produces
        // tracks, and the empty map that results is indistinguishable from a
        // quiet sky.
        return labelled(
            "fusion",
            vec![
                Span::styled("down", Style::default().fg(BAD)),
                dim(format!(
                    "   {}",
                    clip(
                        fusion.reason.as_deref().unwrap_or(""),
                        width.saturating_sub(20)
                    )
                )),
            ],
        );
    }
    labelled("fusion", vec![dim("not configured".to_string())])
}

// ---------------------------------------------------------------------------
// Whatever is holding a radio
// ---------------------------------------------------------------------------

/// Captures and sweeps, but only when there is something to say.
///
/// Both take a radio exclusively — a capture owns the monitor interface for
/// its duration, and a sweep borrows the SDR from dump1090 (ADR-0008). Either
/// one is the answer to "why has that sensor gone quiet", and neither is
/// visible anywhere else on this dashboard.
///
/// Drawn as fields alongside `fusion` rather than under a heading of their
/// own: a heading costs two rows to introduce at most two, and on an idle unit
/// — which is nearly always — this whole section is zero lines.
fn radio_lines<'a>(snapshot: &Snapshot, width: usize) -> Vec<Line<'a>> {
    let mut body: Vec<Line> = Vec::new();

    if let Some(capture) = snapshot.running_capture() {
        body.push(capture_running_line(capture));
    } else if let Some(capture) = snapshot.latest_capture() {
        if let Some(line) = capture_finished_line(capture, width) {
            body.push(line);
        }
    }

    if let Some(sweep) = snapshot.running_sweep() {
        // The consequence, not just the fact. A sweep takes the dongle off
        // dump1090 for its duration, so an operator looking at an SDR sensor
        // that has stopped producing needs to be told this is why — but it is
        // a sentence, and this pane is 46 columns on a bad day, so it is also
        // the first thing to drop rather than something to slice in half.
        let mut spans = vec![Span::styled("running", Style::default().fg(WARN))];
        let head = format!(
            "  {}{}",
            clip(&sweep.band, 10),
            elapsed(sweep.started_at.as_ref())
        );
        let mut used = GUTTER + 1 + "running".len() + head.chars().count();
        spans.push(dim(head));
        push_if_fits(
            &mut spans,
            &mut used,
            width,
            "   no ADS-B while it runs".to_string(),
        );
        body.push(labelled("sweep", spans));
    } else if let Some(sweep) = snapshot.latest_sweep() {
        if let Some(line) = sweep_finished_line(sweep, width) {
            body.push(line);
        }
    }

    body
}

fn capture_running_line<'a>(capture: &Capture) -> Line<'a> {
    // How far in, against how long it asked for. Without a start time there is
    // no elapsed to report and the requested duration stands alone, rather
    // than "ch6 of 60s" reading as a sentence with a word missing.
    let progress = match capture.started_at.as_ref() {
        Some(started) => format!(
            "  {} of {}s",
            short_age(started.age_secs()),
            capture.duration_s
        ),
        None => format!("  {}s", capture.duration_s),
    };
    labelled(
        "capture",
        vec![
            Span::styled("running", Style::default().fg(WARN)),
            dim(format!(
                "  {} ch{}{progress}",
                clip(&capture.iface, 8),
                capture.channel
            )),
        ],
    )
}

/// A finished capture is only worth a row when it failed, or when it found
/// something. A successful, unanalysed pcap is a file on disk and not news.
fn capture_finished_line<'a>(capture: &Capture, width: usize) -> Option<Line<'a>> {
    if capture.state == "failed" {
        // The field ClassG's own web app spent a release discarding, which
        // left a red "failed" badge and no way to find out why.
        return Some(labelled(
            "capture",
            vec![
                Span::styled("failed", Style::default().fg(BAD)),
                dim(format!(
                    "  {}",
                    clip(
                        capture.error.as_deref().unwrap_or("no reason recorded"),
                        width.saturating_sub(21)
                    )
                )),
            ],
        ));
    }
    let analysis = capture.analysis.as_ref().filter(|a| a.analyzed)?;
    let label = capture
        .label
        .as_deref()
        .filter(|l| !l.is_empty())
        .map(|l| format!(" \"{}\"", clip(l, 12)))
        .unwrap_or_default();
    Some(labelled(
        "capture",
        vec![
            Span::styled(
                format!("{} drone tx", analysis.drone_transmitters),
                Style::default().fg(if analysis.drone_transmitters > 0 {
                    WARN
                } else {
                    DIM
                }),
            ),
            dim(format!(
                "  in {} frames{label}",
                compact_count(capture.frame_count)
            )),
        ],
    ))
}

/// Energy only, and never a classification: a peak above threshold means
/// something is transmitting, not that a drone is.
fn sweep_finished_line<'a>(sweep: &SpectrumSweep, width: usize) -> Option<Line<'a>> {
    if sweep.state == "failed" {
        return Some(labelled(
            "sweep",
            vec![
                Span::styled("failed", Style::default().fg(BAD)),
                dim(format!(
                    "  {}",
                    clip(
                        sweep.error.as_deref().unwrap_or("no reason recorded"),
                        width.saturating_sub(21)
                    )
                )),
            ],
        ));
    }
    let peak = sweep.peak_dbfs?;
    let mut spans = vec![
        Span::raw(format!("{:<11}", clip(&sweep.band, 10))),
        dim(format!("peak {peak:.0} dBFS")),
    ];
    // Short reads mean the band was not fully covered and the trace has
    // genuine holes in it, so the peak above is a floor rather than a finding.
    if sweep.short_reads > 0 {
        spans.push(Span::styled(
            format!("  {} short", sweep.short_reads),
            Style::default().fg(WARN),
        ));
    }
    Some(labelled("sweep", spans))
}

/// `  14s` since a start time, or nothing when the record carries none.
fn elapsed(started: Option<&FlexTime>) -> String {
    started
        .map(|ts| format!("  {}", short_age(ts.age_secs())))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

fn track_lines<'a>(page: Option<&TrackPage>, room: usize, width: usize) -> Vec<Line<'a>> {
    let mut lines = vec![Line::default()];
    let Some(page) = page else {
        lines.push(Line::from(dim("  tracks unavailable".to_string())));
        return lines;
    };
    lines.push(Line::from(vec![
        Span::styled("  tracks", Style::default().add_modifier(Modifier::BOLD)),
        // "live", not "total": the poller asks for TENTATIVE, CONFIRMED and
        // COASTING only, because closed tracks are history and enough of them
        // accumulate to push every live contact off a pane this short.
        dim(format!(" {} live", page.total)),
    ]));

    if page.tracks.is_empty() {
        lines.push(Line::from(dim("   nothing tracked".to_string())));
        return lines;
    }

    // STATE 10, CONF 5, EVID 6, DET 5, SEEN 5, plus the three-space gutter.
    let identity_w = width.saturating_sub(34).clamp(6, 26);
    lines.push(table_header(format!(
        "   {:<10}{:<5}{:<identity_w$}{:<6}{:>4} {:>4}",
        "STATE", "CONF", "IDENTITY", "EVID", "DET", "SEEN"
    )));

    let mut budget = room.saturating_sub(1);
    for track in &page.tracks {
        if budget == 0 {
            break;
        }
        lines.push(track_row(track, identity_w));
        budget -= 1;
        // The detail line is a luxury: it goes in only while there are rows
        // spare, and a track with nothing extra to say never asks for one.
        if budget > 0 {
            if let Some(detail) = track_detail(track) {
                lines.push(Line::from(dim(format!(
                    "     {}",
                    clip(&detail, width.saturating_sub(6))
                ))));
                budget -= 1;
            }
        }
    }
    lines
}

fn track_row<'a>(track: &Track, identity_w: usize) -> Line<'a> {
    let confidence = track.confidence;
    let identity = track
        .identity
        .as_ref()
        .map(|i| i.label())
        .unwrap_or_else(|| "unknown".to_string());
    let identified = track.identified();

    Line::from(vec![
        Span::raw(format!("   {:<10}", clip(&track.state, 9))),
        Span::styled(
            format!("{confidence:<5.2}"),
            Style::default().fg(if confidence >= 0.7 {
                OK
            } else if confidence >= 0.4 {
                WARN
            } else {
                DIM
            }),
        ),
        // A contact built only from corroborating evidence is consistent with
        // an aircraft without anything having said it is one. It gets a tilde
        // and the dim ink, because on 2026-08-17 a DJI-branded access point on
        // ch149 sat in this list beside a real Remote ID track and the two were
        // indistinguishable at a glance.
        Span::styled(
            format!(
                "{:<identity_w$}",
                clip(
                    &if identified {
                        identity
                    } else {
                        format!("~{identity}")
                    },
                    identity_w.saturating_sub(1)
                )
            ),
            if identified {
                Style::default()
            } else {
                Style::default().fg(DIM)
            },
        ),
        dim(format!("{:<6}", clip(&track.evidence_summary(), 5))),
        dim(format!("{:>4} ", compact_count(track.detection_count))),
        dim(format!("{:>4}", age_of(track.last_seen.as_ref()))),
    ])
}

/// Kinematics and signal strength, for a track that has any. Nothing about a
/// position is guessed: an aircraft that reported height above ground says so,
/// and one that reported a geodetic altitude is not silently relabelled.
fn track_detail(track: &Track) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(position) = &track.current {
        if let Some((altitude, unit)) = position.altitude() {
            parts.push(format!("{altitude:.0}m {unit}"));
        }
        if let Some(speed) = position.speed_mps {
            parts.push(format!("{speed:.0}m/s"));
        }
    }
    if let Some(rssi) = track.rssi_dbm {
        parts.push(format!("{rssi:.0}dBm"));
    }
    // Correlated with manned traffic on 1090 MHz: this contact is very
    // probably an aeroplane, and saying so is what class D is for.
    if track.adsb_correlated {
        parts.push("ADS-B".to_string());
    }
    if let Some(first) = track.first_seen.as_ref() {
        parts.push(format!("held {}", short_age(first.age_secs())));
    }
    (!parts.is_empty()).then(|| parts.join("  "))
}

// ---------------------------------------------------------------------------
// Detections
// ---------------------------------------------------------------------------

/// Tracks are sparse — most of the time nothing is flying, and on a tall pane
/// that leaves forty-odd rows empty. Detections are the stream underneath them
/// and are never empty while a sensor is alive, so they fill whatever is left.
/// How many detections to ask for, so the folded list fills the space it has.
///
/// Consecutive repeats of one contact fold into a single row, and the fold
/// ratio is a property of the sky rather than of this code: on a quiet unit it
/// is 1, and on one with an aeroplane overhead it was measured at thirteen --
/// forty detections fetched, three rows drawn, and twenty spare rows of pane
/// left empty underneath them. A fixed multiplier cannot cover both, so this
/// measures what the last page actually folded to and scales the next request
/// by it.
///
/// It converges rather than runs away. Once the list fills the room, drawn
/// equals room and the request settles at whatever produced that. The extra
/// cost is only ever paid on a unit that is repeating itself, which is exactly
/// the unit where the spare rows were being wasted.
pub(crate) fn detection_request(page: Option<&DetectionPage>, room: usize) -> usize {
    let room = room.max(1);
    let Some(page) = page else {
        return room.min(MAX_DETECTION_ROWS);
    };
    let fetched = page.detections.len();
    let drawn = collapse_runs(&page.detections).len();
    // Nothing came back, or nothing folded: ask for the room and no more.
    if fetched == 0 || drawn == 0 {
        return room.min(MAX_DETECTION_ROWS);
    }
    (room.saturating_mul(fetched))
        .div_ceil(drawn)
        .clamp(1, MAX_DETECTION_ROWS)
}

fn detection_lines<'a>(page: Option<&DetectionPage>, room: usize, width: usize) -> Vec<Line<'a>> {
    let mut lines = vec![Line::default()];
    let Some(page) = page else {
        return Vec::new();
    };
    lines.push(Line::from(vec![
        Span::styled(
            "  detections",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        dim(format!(" {} total", page.total)),
    ]));

    if page.detections.is_empty() {
        lines.push(Line::from(dim("   nothing heard".to_string())));
        return lines;
    }

    let with_sensor = width >= SENSOR_COLUMN_AT;
    // TIME 9, CLASS 13, dBm 5, TUNE 6, two spaces, plus the gutter — and the
    // SENSOR column when the pane is wide enough to earn it.
    let fixed = 38 + if with_sensor { 8 } else { 0 };
    let label_w = width.saturating_sub(fixed).clamp(4, 20);

    let mut header = format!("   {:<9}", "TIME");
    if with_sensor {
        header.push_str(&format!("{:<8}", "SENSOR"));
    }
    header.push_str(&format!(
        "{:<13}{:>5}{:>6}  {:<label_w$}",
        "CLASS", "dBm", "TUNE", "ID"
    ));
    lines.push(table_header(header));

    for (detection, run) in collapse_runs(&page.detections)
        .into_iter()
        .take(room.saturating_sub(1))
    {
        let rf = detection.rf.clone().unwrap_or_default();
        let (rssi_text, rssi_color) = match rf.rssi_dbm {
            // Thresholds are the same ones the radios pane uses for a link, so
            // a colour means the same thing wherever it appears.
            Some(rssi) if rssi > -60.0 => (format!("{rssi:.0}"), OK),
            Some(rssi) if rssi > -75.0 => (format!("{rssi:.0}"), WARN),
            Some(rssi) => (format!("{rssi:.0}"), DIM),
            None => ("-".to_string(), DIM),
        };

        let mut spans = vec![Span::raw(format!(
            "   {:<9}",
            clock_of(detection.ts.as_ref())
        ))];
        if with_sensor {
            spans.push(dim(format!("{:<8}", clip(sensor_of(detection), 7))));
        }
        spans.push(Span::styled(
            format!("{:<13}", clip(&class_label(&detection.detection_class), 12)),
            // A class that only corroborates gets dim ink here too: it is the
            // same claim the track list makes with a tilde.
            if is_weak(&detection.detection_class) {
                Style::default().fg(DIM)
            } else {
                Style::default()
            },
        ));
        spans.push(Span::styled(
            format!("{rssi_text:>5}"),
            Style::default().fg(rssi_color),
        ));
        // One column for two things no detection ever fills both of: the
        // Wi-Fi sensor names a channel, the SDR knows only a frequency.
        spans.push(dim(format!("{:>6}  ", rf.tuning().unwrap_or_default())));
        // The count is clipped last and never clipped away. Appending it and
        // then trimming the whole string to the column turned `x16` into `x1`
        // on any identity long enough to push it over -- a wrong number rather
        // than a shortened one, which is the failure this pane exists to avoid.
        let label = detection.label();
        spans.push(Span::raw(match run {
            0 | 1 => clip(&label, label_w),
            n => {
                let count = format!(" x{n}");
                let room = label_w.saturating_sub(count.chars().count());
                format!("{}{count}", clip(&label, room))
            }
        }));
        lines.push(Line::from(spans));
    }
    lines
}

/// What to actually do about a credential the API would not accept.
///
/// One sentence for all three cases was wrong in two of them. Which credential
/// went out decides the remedy: a local token that is refused has almost
/// certainly just been rotated by an API restart, and the poller re-reads it
/// by itself, so the honest thing to say is that it is recovering rather than
/// to send somebody to a config file.
fn remedy(credential: Option<CredentialKind>) -> (&'static str, &'static str) {
    match credential {
        // Rotated on every API start. The poller picks the new one up by
        // itself on the next poll, so this row is what the intervening second
        // or two looks like and not something to go and fix.
        Some(CredentialKind::Local) => ("rejected", "token rotated, re-reading"),
        // Sessions slide out after twelve hours and nothing here can renew one.
        Some(CredentialKind::Session) => ("rejected", "CLASSG_SESSION expired"),
        // Never sent anything. Either the API writes no token on this layout,
        // or this process cannot read the one it writes -- that file is 0640,
        // and being able to read it *is* the credential.
        None => ("no token", ".agent-state unreadable?"),
    }
}

/// `wifi-1` where the record names its sensor, the bare kind where it does not.
fn sensor_of(detection: &Detection) -> &str {
    if detection.sensor_id.is_empty() {
        &detection.sensor_kind
    } else {
        &detection.sensor_id
    }
}

/// The class as a name rather than a letter. Falls back to the letter for a
/// class this build has never heard of: it is what the API actually said, and
/// a label this binary is too old to know is not a reason to print nothing.
fn class_label(code: &str) -> String {
    match detection_class_label(code) {
        "" => code.to_string(),
        label => label.to_string(),
    }
}

fn is_weak(code: &str) -> bool {
    crate::panes::classg::is_corroborating_only(code)
}

fn age_of(ts: Option<&FlexTime>) -> String {
    ts.map(|t| short_age(t.age_secs()))
        .unwrap_or_else(|| "-".to_string())
}

/// Detection timestamps are shown as a wall clock, not an age: you are
/// matching them against something you just heard or saw.
fn clock_of(ts: Option<&FlexTime>) -> String {
    ts.map(|t| t.0.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "  --:--".to_string())
}
