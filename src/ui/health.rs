//! The health pane: temperature, power, clock, throttle bits, disk and I/O.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::gauge;
use super::{field, pane_block, threshold_color, BAD, DIM, OK, WARN};
use crate::app::App;
use crate::format::{human_kb, human_rate};
use crate::panes::health::{
    Tense, Throttle, TEMP_HOT_C, TEMP_METER_HI, TEMP_METER_LO, TEMP_WARN_C,
};

/// Lines this pane writes, and therefore the height the layout pins it to:
/// temp, power, throttle-now, throttle-since-boot, disk, io, api freshness.
/// It is a constant rather than a measurement because the pane is laid out
/// before its content exists, and because a pane whose height changed with
/// its contents would shove the two panes under it around every time the
/// supply sagged.
pub const CONTENT_ROWS: u16 = 7;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_block(" Pi health ", app.accent, app.glyphs);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let health = &app.health;
    let mut lines: Vec<Line> = Vec::new();

    // ── temperature ──
    lines.push(field(
        "temp",
        match health.temp_c {
            Some(temp) => {
                let mut spans = vec![
                    Span::styled(
                        format!("{temp:>4.1}C"),
                        Style::default()
                            .fg(threshold_color(temp, TEMP_WARN_C, TEMP_HOT_C))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                ];
                // The same gradient meter the system pane uses, over the
                // throttle window rather than over 0-100: an SoC temperature
                // is only meaningful against the point it starts throttling
                // at, and "68% of nothing in particular" is not actionable.
                spans.extend(gauge::bar(
                    (temp - TEMP_METER_LO) / (TEMP_METER_HI - TEMP_METER_LO),
                    12,
                    app.glyphs,
                ));
                spans.push(Span::styled(
                    format!("  {TEMP_METER_LO:.0}-{TEMP_METER_HI:.0}"),
                    Style::default().fg(DIM),
                ));
                spans
            }
            None => vec![Span::styled("no thermal zone", Style::default().fg(DIM))],
        },
    ));

    // ── power and clock ──
    let volts = match health.volts {
        Some(v) => format!("{v:.4}V core"),
        None => "?V core".to_string(),
    };
    let clock = match (health.arm_mhz, health.max_mhz) {
        (Some(now), Some(max)) => format!("clock {now}/{max} MHz"),
        (Some(now), None) => format!("clock {now} MHz"),
        _ => "clock ?".to_string(),
    };
    lines.push(field(
        "power",
        vec![
            Span::raw(volts),
            Span::styled(format!("   {clock}"), Style::default().fg(DIM)),
        ],
    ));

    // ── the reason this pane exists ──
    lines.extend(throttle_lines(health.throttle));

    // ── disk ──
    lines.push(field(
        "disk",
        match health.disk {
            Some(disk) => vec![
                Span::raw(format!(
                    "{}/{}",
                    human_kb(disk.used_kb),
                    human_kb(disk.total_kb)
                )),
                Span::styled(
                    format!("  {:.0}%", disk.pct()),
                    Style::default().fg(threshold_color(disk.pct(), 80.0, 92.0)),
                ),
            ],
            None => vec![Span::styled("unavailable", Style::default().fg(DIM))],
        },
    ));

    lines.push(field(
        "io",
        vec![Span::raw(format!(
            "r {}   w {}",
            human_rate(health.io.read_bps),
            human_rate(health.io.write_bps)
        ))],
    ));

    lines.push(field(
        "api",
        vec![Span::styled(
            match app.classg.last_ok {
                Some(at) => format!("last good poll {}s ago", at.elapsed().as_secs()),
                None => "no successful poll yet".to_string(),
            },
            Style::default().fg(DIM),
        )],
    ));

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Renders the decoded throttle register on two rows: what is happening now,
/// and what has happened since boot.
///
/// One row was not enough. The Bash version concatenated both halves into a
/// single line and, once more than one bit was set, the sticky half ran off
/// the right edge of a 46-column pane — so a Pi that was under-volting *and*
/// had been throttled before showed only the first half of its own story.
/// Two rows also make the distinction unmissable, which matters: `0x50000`
/// with a clean low nibble means it already happened and you missed it, which
/// is a different problem from `0x50005` and needs to look different.
fn throttle_lines(throttle: Option<Throttle>) -> Vec<Line<'static>> {
    let Some(throttle) = throttle else {
        // Not "OK". The Bash version treated a missing vcgencmd as a zero
        // register and printed "clean since boot", which is a confident lie on
        // exactly the machines that cannot tell.
        return vec![
            field(
                "thrott",
                vec![Span::styled(
                    "unknown - no vcgencmd here",
                    Style::default().fg(DIM),
                )],
            ),
            field(
                "since",
                vec![Span::styled(
                    "so the sticky bits are unknown too",
                    Style::default().fg(DIM),
                )],
            ),
        ];
    };

    let now = throttle.now.labels(Tense::Now);
    let sticky = throttle.since_boot.labels(Tense::SinceBoot);

    let now_line = field(
        "thrott",
        if now.is_empty() {
            vec![
                Span::styled("OK", Style::default().fg(OK).add_modifier(Modifier::BOLD)),
                Span::styled("  nothing right now", Style::default().fg(DIM)),
            ]
        } else {
            vec![Span::styled(
                now.join(", "),
                Style::default().fg(BAD).add_modifier(Modifier::BOLD),
            )]
        },
    );

    let sticky_line = field(
        "since",
        if sticky.is_empty() {
            vec![Span::styled("clean since boot", Style::default().fg(DIM))]
        } else {
            vec![
                Span::styled(sticky.join(", "), Style::default().fg(WARN)),
                Span::styled(
                    format!("  (0x{:x})", throttle.raw),
                    Style::default().fg(DIM),
                ),
            ]
        },
    );

    vec![now_line, sticky_line]
}
