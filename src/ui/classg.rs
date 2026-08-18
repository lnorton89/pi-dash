//! The ClassG pane.
//!
//! Everything below the sensor block is the "feed in ClassG data" hook: any
//! endpoint under `services/api/internal/httpapi` can be rendered here the
//! same way. Tracks and detections are the useful defaults.

use chrono::Local;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use super::{pane_block, BAD, DIM, OK, WARN};
use crate::app::App;
use crate::format::{clip, coarse_uptime, short_age};
use crate::panes::classg::{DetectionPage, FlexTime, HealthResponse, TrackPage};

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" ClassG  {} ", app.config.api.trim_start_matches("http://"));
    let block = pane_block(&title, app.accent, app.glyphs);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

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

    lines.extend(health_lines(health));

    // Sensor state above is what you cannot afford to lose, so tracks are what
    // gets dropped when the pane is short: rendering eleven lines into seven
    // rows scrolls the health verdict off the top and the pane silently
    // becomes a track list.
    let mut track_room = 0usize;
    let mut detection_room = 0usize;

    // Two rows of overhead per section: the blank spacer and the heading.
    let room = |used: usize| (inner.height as usize).saturating_sub(used + 2);

    if room(lines.len()) >= 1 {
        track_room = room(lines.len());
        lines.extend(track_lines(snapshot.tracks.as_ref(), track_room));
    }
    if room(lines.len()) >= 1 {
        detection_room = room(lines.len());
        lines.extend(detection_lines(
            snapshot.detections.as_ref(),
            detection_room,
        ));
    }

    // Ask for exactly what will fit next time. This pane is handed the slack
    // left over from the two fixed-height ones, so on a tall terminal that is
    // a real list rather than the first three rows of one.
    app.classg
        .set_hints(track_room.max(1), detection_room.max(1));

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

fn health_lines<'a>(health: &HealthResponse) -> Vec<Line<'a>> {
    let status_color = match health.status.as_str() {
        "ok" | "healthy" => OK,
        "degraded" => WARN,
        _ => BAD,
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("  {}", health.status),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "   up {}   {}",
                    coarse_uptime(health.uptime_s),
                    health.version
                ),
                Style::default().fg(DIM),
            ),
        ]),
        Line::from(Span::styled(
            "  sensors",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];

    if health.sensors.is_empty() {
        lines.push(Line::from(Span::styled(
            "   no sensors reporting",
            Style::default().fg(DIM),
        )));
    }
    for sensor in &health.sensors {
        // An optional sensor that is down is a configuration you chose, not a
        // fault — the SDR is optional on a Wi-Fi-only build. It gets amber and
        // lower case so a genuinely broken required sensor still stands out.
        let (mark, color) = match (sensor.healthy, sensor.optional) {
            (true, _) => ("ok  ", OK),
            (false, true) => ("off ", WARN),
            (false, false) => ("DOWN", BAD),
        };
        lines.push(Line::from(vec![
            Span::raw(format!("   {:<10} ", clip(&sensor.sensor_id, 10))),
            Span::styled(
                format!("{:<4} ", clip(&sensor.sensor_kind, 4)),
                Style::default().fg(DIM),
            ),
            Span::styled(mark, Style::default().fg(color)),
            Span::raw(format!(
                " {:>5} ",
                sensor
                    .seconds_since_heartbeat
                    .map(|s| format!("{s}s"))
                    .unwrap_or_else(|| "-".to_string())
            )),
            Span::styled(
                format!("5m:{}", sensor.detections_5m),
                Style::default().fg(DIM),
            ),
        ]));
        // The reason a sensor is down is the whole point of looking at this
        // pane, so it gets its own line rather than being truncated into the
        // margin.
        if let Some(reason) = sensor.reason.as_deref().filter(|r| !r.is_empty()) {
            lines.push(Line::from(Span::styled(
                format!("     {}", clip(reason, 40)),
                Style::default().fg(WARN),
            )));
        }
    }

    let fusion = health.fusion.clone().unwrap_or_default();
    lines.push(if fusion.connected {
        Line::from(vec![
            Span::raw("  fusion   "),
            Span::styled("connected", Style::default().fg(OK)),
            Span::styled(
                format!(" last {}", age_of(fusion.last_message.as_ref())),
                Style::default().fg(DIM),
            ),
        ])
    } else if fusion.configured {
        Line::from(vec![
            Span::raw("  fusion   "),
            Span::styled("down", Style::default().fg(BAD)),
            Span::styled(
                format!(" {}", clip(fusion.reason.as_deref().unwrap_or(""), 30)),
                Style::default().fg(DIM),
            ),
        ])
    } else {
        Line::from(vec![
            Span::raw("  fusion   "),
            Span::styled("not configured", Style::default().fg(DIM)),
        ])
    });

    lines
}

fn track_lines<'a>(page: Option<&TrackPage>, room: usize) -> Vec<Line<'a>> {
    let mut lines = vec![Line::default()];
    let Some(page) = page else {
        lines.push(Line::from(Span::styled(
            "  tracks unavailable",
            Style::default().fg(DIM),
        )));
        return lines;
    };
    lines.push(Line::from(vec![
        Span::styled("  tracks", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} total", page.total), Style::default().fg(DIM)),
    ]));

    if page.tracks.is_empty() {
        lines.push(Line::from(Span::styled(
            "   nothing tracked",
            Style::default().fg(DIM),
        )));
        return lines;
    }
    for track in page.tracks.iter().take(room) {
        let confidence = track.confidence;
        lines.push(Line::from(vec![
            Span::raw(format!("   {:<9} ", clip(&track.state, 9))),
            Span::styled(
                format!("{confidence:.2}"),
                Style::default().fg(if confidence >= 0.7 {
                    OK
                } else if confidence >= 0.4 {
                    WARN
                } else {
                    DIM
                }),
            ),
            Span::raw(format!(
                " {:<16} ",
                clip(
                    &track
                        .identity
                        .as_ref()
                        .map(|i| i.label())
                        .unwrap_or_else(|| "unknown".to_string()),
                    16
                )
            )),
            Span::styled(age_of(track.last_seen.as_ref()), Style::default().fg(DIM)),
        ]));
    }
    lines
}

/// Tracks are sparse — most of the time nothing is flying, and on a tall pane
/// that leaves forty-odd rows empty. Detections are the stream underneath them
/// and are never empty while a sensor is alive, so they fill whatever is left.
fn detection_lines<'a>(page: Option<&DetectionPage>, room: usize) -> Vec<Line<'a>> {
    let mut lines = vec![Line::default()];
    let Some(page) = page else {
        return Vec::new();
    };
    lines.push(Line::from(vec![
        Span::styled(
            "  detections",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {} total", page.total), Style::default().fg(DIM)),
    ]));

    if page.detections.is_empty() {
        lines.push(Line::from(Span::styled(
            "   nothing heard",
            Style::default().fg(DIM),
        )));
        return lines;
    }
    for detection in page.detections.iter().take(room) {
        let rf = detection.rf.clone().unwrap_or_default();
        let (rssi_text, rssi_color) = match rf.rssi_dbm {
            Some(rssi) if rssi > -60.0 => (format!("{rssi:>4.0}"), OK),
            Some(rssi) if rssi > -75.0 => (format!("{rssi:>4.0}"), WARN),
            Some(rssi) => (format!("{rssi:>4.0}"), DIM),
            None => ("   -".to_string(), DIM),
        };
        lines.push(Line::from(vec![
            Span::raw(format!("   {} ", clock_of(detection.ts.as_ref()))),
            Span::styled(
                format!("{:<4} ", clip(&detection.sensor_kind, 4)),
                Style::default().fg(DIM),
            ),
            Span::raw(format!("{:<13} ", clip(&detection.detection_class, 13))),
            Span::styled(rssi_text, Style::default().fg(rssi_color)),
            Span::styled(
                format!(
                    " {:<5}",
                    rf.channel.map(|c| format!("ch{c}")).unwrap_or_default()
                ),
                Style::default().fg(DIM),
            ),
            Span::raw(clip(
                &detection
                    .identity
                    .as_ref()
                    .map(|i| i.label())
                    .unwrap_or_default(),
                10,
            )),
        ]));
    }
    lines
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
