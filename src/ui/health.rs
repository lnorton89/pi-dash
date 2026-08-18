//! The health pane: temperature, power, clock, throttle bits, disk and I/O.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::gauge;
use super::{field, pane_block, push_if_fits, threshold_color, BAD, DIM, GUTTER, OK, WARN};
use crate::app::App;
use crate::format::{human_kb, human_rate};
use crate::panes::health::{
    Tense, Throttle, TEMP_HOT_C, TEMP_METER_HI, TEMP_METER_LO, TEMP_WARN_C,
};

/// Lines this pane writes, and therefore the height the layout pins it to:
/// temp, volts, clock, throttle-now, throttle-since-boot, disk, io, api.
/// It is a constant rather than a measurement because the pane is laid out
/// before its content exists, and because a pane whose height changed with
/// its contents would shove the two panes under it around every time the
/// supply sagged.
pub const CONTENT_ROWS: u16 = 8;

/// The column every row right-aligns its value into, before its meter. Wide
/// enough for `0.850V`, `117.0G` and a four-digit clock.
const VALUE_W: usize = 6;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_block(" Pi health ", app.accent, app.glyphs);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let health = &app.health;
    let width = inner.width as usize;
    // Every meter in this pane is the same width and starts at the same
    // column, so the bars form one vertical band and the figures after them
    // line up. Sizing each row to its own value put the four meters at four
    // different columns and made the pane read as a ragged list.
    let meter = (width / 4).clamp(8, 12);
    let meter_at = GUTTER + VALUE_W + 1;
    let value = |text: String| {
        Span::styled(
            format!("{text:>VALUE_W$} "),
            Style::default().add_modifier(Modifier::BOLD),
        )
    };
    let mut lines: Vec<Line> = Vec::new();

    // ── temperature ──
    lines.push(field(
        "temp",
        match health.temp_c {
            Some(temp) => {
                let mut spans = vec![Span::styled(
                    format!("{:>VALUE_W$} ", format!("{temp:.1}C")),
                    Style::default()
                        .fg(threshold_color(temp, TEMP_WARN_C, TEMP_HOT_C))
                        .add_modifier(Modifier::BOLD),
                )];
                // The same gradient meter the system pane uses, over the
                // throttle window rather than over 0-100: an SoC temperature
                // is only meaningful against the point it starts throttling
                // at, and "68% of nothing in particular" is not actionable.
                spans.extend(gauge::bar(
                    (temp - TEMP_METER_LO) / (TEMP_METER_HI - TEMP_METER_LO),
                    meter,
                    app.glyphs,
                    gauge::Ramp::Load,
                ));
                let mut used = meter_at + meter;
                // The range is the throttle window, not 0-100, so it has to
                // say so: a bare 30-85 beside a bar is a pair of numbers with
                // no stated meaning.
                push_if_fits(
                    &mut spans,
                    &mut used,
                    width,
                    format!("  {TEMP_METER_LO:.0}-{TEMP_METER_HI:.0}C"),
                );
                push_if_fits(
                    &mut spans,
                    &mut used,
                    width,
                    format!("  {:.0}C to throttle", (TEMP_HOT_C - temp).max(0.0)),
                );
                spans
            }
            None => vec![Span::styled("no thermal zone", Style::default().fg(DIM))],
        },
    ));

    // ── core voltage ──
    //
    // Its own row rather than sharing one with the clock. They are unrelated
    // measurements, and putting them together meant neither had a label in the
    // gutter — the row read as an undifferentiated run of numbers.
    lines.push(field(
        "volts",
        match health.volts {
            Some(v) => vec![
                value(format!("{v:.3}V")),
                Span::styled("core", Style::default().fg(DIM)),
            ],
            None => vec![Span::styled(
                "unavailable - no vcgencmd",
                Style::default().fg(DIM),
            )],
        },
    ));

    // ── ARM clock ──
    lines.push(field(
        "clock",
        match (health.arm_mhz, health.max_mhz) {
            (Some(now), Some(max)) if max > 0 => {
                let frac = now as f64 / max as f64;
                let mut spans = vec![value(now.to_string())];
                // Cool, not load: a Pi clocked down is the governor idling,
                // which is the opposite of a problem. What you are watching for
                // is a clock pinned low while the box is busy, and that reads
                // off the bar's length either way.
                spans.extend(gauge::bar(frac, meter, app.glyphs, gauge::Ramp::Cool));
                let mut used = meter_at + meter;
                push_if_fits(
                    &mut spans,
                    &mut used,
                    width,
                    format!(" {:>3.0}%  of {max} MHz", frac * 100.0),
                );
                spans
            }
            (Some(now), _) => vec![Span::raw(format!("{now} MHz"))],
            _ => vec![Span::styled("unavailable", Style::default().fg(DIM))],
        },
    ));

    // ── the reason this pane exists ──
    lines.extend(throttle_lines(health.throttle));

    // ── disk ──
    lines.push(field(
        "disk",
        match health.disk {
            Some(disk) => {
                let pct = disk.pct();
                let mut spans = vec![value(human_kb(disk.used_kb))];
                spans.extend(gauge::bar(
                    pct / 100.0,
                    meter,
                    app.glyphs,
                    gauge::Ramp::Load,
                ));
                spans.push(Span::styled(
                    format!(" {pct:>3.0}%"),
                    Style::default().fg(threshold_color(pct, 80.0, 92.0)),
                ));
                let mut used = meter_at + meter + 5;
                // Free is the number you act on. "26% used" and "86G free"
                // answer different questions, and on a Pi about to write an
                // image it is the second one.
                push_if_fits(
                    &mut spans,
                    &mut used,
                    width,
                    format!(
                        "  {} free of {}",
                        human_kb(disk.total_kb.saturating_sub(disk.used_kb)),
                        human_kb(disk.total_kb)
                    ),
                );
                spans
            }
            None => vec![Span::styled(
                "unavailable - df did not run",
                Style::default().fg(DIM),
            )],
        },
    ));

    // Fixed-width so the two rates sit in columns, rather than the write
    // figure shifting every time the read one crosses a power of 1024.
    lines.push(field(
        "io",
        vec![
            Span::styled("read ", Style::default().fg(DIM)),
            Span::raw(format!("{:<11}", human_rate(health.io.read_bps))),
            Span::styled("write ", Style::default().fg(DIM)),
            Span::raw(human_rate(health.io.write_bps)),
        ],
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
