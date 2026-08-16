//! The system pane: CPU, memory, and the top processes.
//!
//! Meters are drawn with `#` and `.` rather than block-drawing characters.
//! This is often watched on the Pi's own HDMI console, where the kernel
//! framebuffer font has no box-drawing glyphs and every bar would come out as
//! a row of replacement characters.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{field, pane_block, threshold_color, DIM};
use crate::app::App;
use crate::format::{human_kb, meter, uptime};

/// Width of the aggregate CPU and memory meters.
const METER_WIDTH: usize = 16;
/// Width of each per-core meter. Narrow on purpose — four of these plus their
/// labels have to fit a pane that may only be 40 columns wide.
const CORE_METER_WIDTH: usize = 6;
/// Columns one per-core cell occupies, including its trailing gap.
const CORE_CELL_WIDTH: usize = CORE_METER_WIDTH + 12;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let pane = app;
    let block = pane_block(" System ", app.accent);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    if let Some(reason) = &pane.system.unavailable {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {reason}"),
                Style::default().fg(super::WARN),
            ))),
            inner,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let system = &pane.system;

    // ── aggregate CPU ──
    let cpu = system.cpu_pct.unwrap_or(0.0);
    lines.push(field(
        "cpu",
        vec![
            Span::styled(
                format!("[{}]", meter(cpu, 0.0, 100.0, METER_WIDTH)),
                Style::default().fg(threshold_color(cpu, 60.0, 85.0)),
            ),
            Span::styled(
                match system.cpu_pct {
                    Some(pct) => format!(" {pct:>3.0}%"),
                    // The first sample has no previous one to difference
                    // against; showing 0% there would be a lie you cannot
                    // distinguish from an idle box.
                    None => "   —".to_string(),
                },
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} cores", system.core_pct.len()),
                Style::default().fg(DIM),
            ),
        ],
    ));

    // ── per-core meters, packed as many to a row as the width allows ──
    let per_row = (inner.width as usize / CORE_CELL_WIDTH).max(1);
    for (row, chunk) in system.core_pct.chunks(per_row).enumerate() {
        let mut spans = vec![Span::raw("  ")];
        for (offset, pct) in chunk.iter().enumerate() {
            let index = row * per_row + offset;
            let value = pct.unwrap_or(0.0);
            // Two-digit padding so c8..c15 on a 16-core box do not shunt every
            // meter after them one column right.
            spans.push(Span::styled(
                format!("c{index:<2} "),
                Style::default().fg(DIM),
            ));
            spans.push(Span::styled(
                format!("[{}]", meter(value, 0.0, 100.0, CORE_METER_WIDTH)),
                Style::default().fg(threshold_color(value, 60.0, 85.0)),
            ));
            spans.push(Span::raw(format!("{value:>4.0}%  ")));
        }
        lines.push(Line::from(spans));
    }

    // ── memory ──
    let mem = &system.mem;
    let mem_pct = mem.used_pct();
    lines.push(field(
        "mem",
        vec![
            Span::styled(
                format!("[{}]", meter(mem_pct, 0.0, 100.0, METER_WIDTH)),
                Style::default().fg(threshold_color(mem_pct, 75.0, 90.0)),
            ),
            Span::styled(
                format!(" {mem_pct:>3.0}%"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}/{}", human_kb(mem.used_kb()), human_kb(mem.total_kb)),
                Style::default().fg(DIM),
            ),
        ],
    ));

    let swap = if mem.swap_total_kb == 0 {
        Span::styled("off", Style::default().fg(DIM))
    } else {
        // Swap in use on a Pi means the SD card is now in the latency path,
        // so it is called out rather than shown as another quiet meter.
        let used = mem.swap_used_kb();
        Span::styled(
            format!("{}/{}", human_kb(used), human_kb(mem.swap_total_kb)),
            Style::default().fg(if used > 0 { super::WARN } else { DIM }),
        )
    };
    lines.push(field(
        "swap",
        vec![
            swap,
            Span::styled(
                format!(
                    "   load {:.2} {:.2} {:.2}   up {}",
                    system.load[0],
                    system.load[1],
                    system.load[2],
                    uptime(system.uptime_secs)
                ),
                Style::default().fg(DIM),
            ),
        ],
    ));

    // ── process table ──
    lines.push(Line::default());
    let used = lines.len();
    let rows = (inner.height as usize)
        .saturating_sub(used + 1)
        .min(app.config.processes.unwrap_or(usize::MAX));
    if rows > 0 {
        lines.push(Line::from(Span::styled(
            format!("  {:<7}{:<7}{:>7}  {:<}", "PID", "CPU%", "MEM", "COMMAND"),
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )));
        // Only the busiest are listed. A Pi runs a couple of hundred mostly
        // idle processes and a full table is a scrolling wall you never read;
        // what you want to know is which one just woke up.
        for proc in system.procs.iter().take(rows) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<7}", proc.pid), Style::default().fg(DIM)),
                Span::styled(
                    format!("{:<7.1}", proc.cpu_pct),
                    Style::default().fg(threshold_color(proc.cpu_pct, 25.0, 75.0)),
                ),
                Span::raw(format!("{:>7}  ", human_kb(proc.rss_kb))),
                Span::styled(
                    proc.name.clone(),
                    Style::default().fg(if proc.state == 'D' {
                        // Uninterruptible sleep: on this box that is almost
                        // always the SD card, and it is worth spotting.
                        super::WARN
                    } else {
                        ratatui::style::Color::White
                    }),
                ),
            ]));
        }
        if system.procs.is_empty() {
            lines.push(Line::from(Span::styled(
                "  waiting for a second sample…",
                Style::default().fg(DIM),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
