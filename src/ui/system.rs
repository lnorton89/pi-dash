//! The system pane: CPU, memory, and the top processes.
//!
//! Laid out the way btop lays out its CPU and memory boxes, because that is
//! the layout this pane replaced and the one the muscle memory is for: a
//! scrolling history graph above a gradient meter, per-core meters packed
//! into a grid under it, then the memory split, then the busiest processes.
//!
//! Everything drawn here goes through [`super::gauge`], so the whole pane
//! switches to ASCII — frame included — on a terminal whose font cannot do
//! block and braille characters. That is the Pi's own HDMI console, and it is
//! why this pane was flat ASCII to begin with.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::gauge::{self, Glyphs};
use super::{field, pane_block, threshold_color, DIM};
use crate::app::App;
use crate::format::{human_kb, uptime};

/// The aggregate CPU and memory meters size themselves to the pane, within
/// these bounds. Below the minimum a meter is too coarse to read a trend off;
/// above the maximum it is just a long line that pushes the numbers away from
/// the label they belong to.
const METER_MIN: usize = 12;
const METER_MAX: usize = 24;

/// Columns a per-core cell spends on things that are not its meter:
/// `c12 ` in front, ` 100%  ` behind.
const CORE_LABEL: usize = 4;
const CORE_TAIL: usize = 7;
const CORE_METER_MIN: usize = 6;
const CORE_METER_MAX: usize = 14;

/// Process table columns. The command name gets whatever is left.
const PID_W: usize = 8;
const MEM_W: usize = 7;
/// `999.9 ` — wide enough for a process pegging four cores.
const CPU_NUM_W: usize = 6;
const PROC_BAR_W: usize = 10;
/// Below this the per-process meter is dropped and the name keeps the room.
const PROC_BAR_MIN_COLS: usize = 66;

/// Rows the history graph is allowed, and the point below which it is not
/// worth drawing at all. Two rows of braille is eight levels, which turns
/// every interesting shape into the same flat-topped block.
const GRAPH_MAX_ROWS: usize = 6;
const GRAPH_MIN_ROWS: usize = 3;
/// Process rows the graph will not eat into. A history graph above an empty
/// table answers the wrong question.
const PROC_ROWS_RESERVED: usize = 5;

/// The left gutter [`field`] establishes: two spaces plus a seven-column
/// label. The graph lines up with it so the graph and the meters below share
/// one left edge instead of looking like two unrelated widgets.
const GUTTER: usize = 9;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let system = &app.system;
    let glyphs = app.glyphs;

    let mut block = pane_block(" System ", app.accent, app.glyphs);
    // Uptime rides in the top-right of the frame, as it does in btop. It is
    // the one number here that never changes meaningfully between frames, so
    // it costs nothing to put it where it is out of the way. Only when the
    // pane is wide enough that it cannot collide with the title.
    if area.width >= 44 && system.unavailable.is_none() {
        block = block.title_top(
            Line::from(Span::styled(
                format!(" up {} ", uptime(system.uptime_secs)),
                Style::default().fg(DIM),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if let Some(reason) = &system.unavailable {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {reason}"),
                Style::default().fg(super::WARN),
            ))),
            inner,
        );
        return;
    }

    let width = inner.width as usize;
    let meter_width = (width / 3).clamp(METER_MIN, METER_MAX).min(width);
    let (per_row, core_meter) = core_grid(width, system.core_pct.len());
    let core_rows = system.core_pct.len().div_ceil(per_row);

    let mut lines: Vec<Line> = Vec::new();

    // ── CPU history ──
    //
    // This is the only part of the pane that redraws wholesale on a sample,
    // because the whole series shifts one column left. That is a sample-tick
    // cost, not a frame cost: the loop's other reason to redraw is the header
    // clock, and on those frames the graph diffs to nothing.
    let graph_rows = graph_rows(inner.height as usize, core_rows);
    if graph_rows > 0 && width > GUTTER + 1 {
        for row in gauge::area_graph(&system.cpu_history, width - GUTTER - 1, graph_rows, glyphs) {
            lines.push(field("", row.spans));
        }
    }

    // ── aggregate CPU ──
    let cpu = system.cpu_pct.unwrap_or(0.0);
    let mut cpu_spans = gauge::bar(cpu / 100.0, meter_width, glyphs);
    let cpu_value = match system.cpu_pct {
        Some(pct) => format!(" {pct:>3.0}%"),
        // The first sample has no previous one to difference against; showing
        // 0% there would be a lie you cannot distinguish from an idle box.
        None => match glyphs {
            Glyphs::Unicode => "   —".to_string(),
            Glyphs::Ascii => "   -".to_string(),
        },
    };
    let mut used = GUTTER + meter_width + cpu_value.chars().count();
    cpu_spans.push(Span::styled(
        cpu_value,
        Style::default()
            .fg(threshold_color(cpu, 60.0, 85.0))
            .add_modifier(Modifier::BOLD),
    ));
    push_if_fits(
        &mut cpu_spans,
        &mut used,
        width,
        format!("  {} cores", system.core_pct.len()),
    );
    push_if_fits(
        &mut cpu_spans,
        &mut used,
        width,
        format!(
            "   load {:.2} {:.2} {:.2}",
            system.load[0], system.load[1], system.load[2],
        ),
    );
    lines.push(field("cpu", cpu_spans));

    // ── per-core meters, packed as many to a row as the width allows ──
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
            spans.extend(gauge::bar(value / 100.0, core_meter, glyphs));
            spans.push(Span::styled(
                format!("{value:>4.0}%  "),
                Style::default().fg(threshold_color(value, 60.0, 85.0)),
            ));
        }
        lines.push(Line::from(spans));
    }

    // ── memory ──
    let mem = &system.mem;
    let mem_pct = mem.used_pct();
    let mut mem_spans = gauge::bar(mem_pct / 100.0, meter_width, glyphs);
    let mut used = GUTTER + meter_width + 5;
    mem_spans.push(Span::styled(
        format!(" {mem_pct:>3.0}%"),
        Style::default()
            .fg(threshold_color(mem_pct, 75.0, 90.0))
            .add_modifier(Modifier::BOLD),
    ));
    push_if_fits(
        &mut mem_spans,
        &mut used,
        width,
        format!("  {}/{}", human_kb(mem.used_kb()), human_kb(mem.total_kb)),
    );
    lines.push(field("mem", mem_spans));

    // Buffers and page cache get their own meter rather than being folded into
    // the used figure. On a Pi running the ClassG stack in Docker this is
    // routinely a third of RAM, and it is the part that will be given back
    // under pressure — which is exactly the question you are asking when you
    // look at a memory bar that is nearly full.
    let cache_kb = mem.buffers_kb.saturating_add(mem.cached_kb);
    let cache_pct = if mem.total_kb == 0 {
        0.0
    } else {
        cache_kb as f64 * 100.0 / mem.total_kb as f64
    };
    let mut cache_spans = gauge::bar(cache_pct / 100.0, meter_width, glyphs);
    let mut used = GUTTER + meter_width + 5;
    cache_spans.push(Span::styled(
        format!(" {cache_pct:>3.0}%"),
        Style::default().fg(DIM),
    ));
    let cache_size = human_kb(cache_kb);
    if !push_if_fits(
        &mut cache_spans,
        &mut used,
        width,
        format!("  {cache_size} reclaimable"),
    ) {
        push_if_fits(
            &mut cache_spans,
            &mut used,
            width,
            format!("  {cache_size}"),
        );
    }
    lines.push(field("cache", cache_spans));

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
    let mut used = GUTTER + swap.content.chars().count();
    let mut swap_spans = vec![swap];
    if !push_if_fits(
        &mut swap_spans,
        &mut used,
        width,
        format!(
            "   {} tasks, {} running",
            system.task_count, system.runnable
        ),
    ) {
        push_if_fits(
            &mut swap_spans,
            &mut used,
            width,
            format!("   {} tasks", system.task_count),
        );
    }
    lines.push(field("swap", swap_spans));

    // ── process table ──
    lines.push(Line::default());
    let rows = (inner.height as usize)
        .saturating_sub(lines.len() + 1)
        .min(app.config.processes.unwrap_or(usize::MAX));
    if rows > 0 {
        let (name_w, bar_w) = proc_columns(width);
        lines.push(Line::from(Span::styled(
            // CPU% is right-aligned over the numbers below it, not left over
            // the space before them.
            format!(
                "  {:<PID_W$}{:<name_w$}{:>MEM_W$}   {:>5}",
                "PID", "COMMAND", "MEM", "CPU%"
            ),
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )));
        // Only the busiest are listed. A Pi runs a couple of hundred mostly
        // idle processes and a full table is a scrolling wall you never read;
        // what you want to know is which one just woke up.
        for proc in system.procs.iter().take(rows) {
            let mut spans = vec![
                Span::styled(format!("  {:<PID_W$}", proc.pid), Style::default().fg(DIM)),
                Span::styled(
                    format!("{:<name_w$}", crate::format::clip(&proc.name, name_w)),
                    Style::default().fg(if proc.state == 'D' {
                        // Uninterruptible sleep: on this box that is almost
                        // always the SD card, and it is worth spotting.
                        super::WARN
                    } else {
                        Color::White
                    }),
                ),
                Span::raw(format!("{:>MEM_W$}   ", human_kb(proc.rss_kb))),
                Span::styled(
                    format!("{:>5.1} ", proc.cpu_pct),
                    Style::default().fg(threshold_color(proc.cpu_pct, 25.0, 75.0)),
                ),
            ];
            if bar_w > 0 {
                // Scaled against one core, which is what the number beside it
                // means. A four-thread build reads 380% and pegs the bar —
                // that is the honest reading, not a clipped one.
                spans.extend(gauge::bar(proc.cpu_pct / 100.0, bar_w, glyphs));
            }
            lines.push(Line::from(spans));
        }
        if system.procs.is_empty() {
            lines.push(Line::from(Span::styled(
                match glyphs {
                    Glyphs::Unicode => "  waiting for a second sample…",
                    Glyphs::Ascii => "  waiting for a second sample...",
                },
                Style::default().fg(DIM),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Appends a dim suffix, but only if the line still has room for all of it.
///
/// The pane renders without wrapping, so a tail that does not fit is not
/// wrapped — it is sliced wherever the pane ends, which is how a 45-column
/// pane produced `1.9G reclaimab` and a load average cut off after the word
/// `load`. Returns whether it fitted, so a caller can offer a shorter form.
fn push_if_fits<'a>(
    spans: &mut Vec<Span<'a>>,
    used: &mut usize,
    width: usize,
    text: String,
) -> bool {
    let len = text.chars().count();
    if *used + len > width {
        return false;
    }
    *used += len;
    spans.push(Span::styled(text, Style::default().fg(DIM)));
    true
}

/// How many per-core meters fit on a row, and how wide each one gets.
///
/// Packs as many cores per row as the minimum cell allows, then spends the
/// leftover columns on widening the meters rather than on trailing space.
/// Returns `(per_row, meter_width)`; `per_row` is never zero, so callers can
/// chunk by it.
fn core_grid(width: usize, cores: usize) -> (usize, usize) {
    let available = width.saturating_sub(2);
    let min_cell = CORE_LABEL + CORE_METER_MIN + CORE_TAIL;
    let fits = (available / min_cell.max(1)).max(1);
    let per_row = if cores == 0 { 1 } else { fits.min(cores) };
    let meter = (available / per_row)
        .saturating_sub(CORE_LABEL + CORE_TAIL)
        .clamp(CORE_METER_MIN, CORE_METER_MAX);
    (per_row, meter)
}

/// Rows to give the history graph, given the pane height and how many rows the
/// core grid will take. Zero when there is not enough room for a graph worth
/// looking at.
fn graph_rows(height: usize, core_rows: usize) -> usize {
    // cpu + cores + mem + cache + swap + blank + table header.
    let fixed = 6 + core_rows;
    let spare = height.saturating_sub(fixed + PROC_ROWS_RESERVED);
    if spare < GRAPH_MIN_ROWS {
        0
    } else {
        spare.min(GRAPH_MAX_ROWS)
    }
}

/// `(name_width, cpu_bar_width)` for the process table. The name column
/// absorbs the slack, and the per-process meter is the first thing dropped
/// when the pane gets narrow — a truncated command name costs you more.
fn proc_columns(width: usize) -> (usize, usize) {
    // Two columns of indent, and one on the right so the longest row — which
    // is every row, because the name column absorbs the slack — does not run
    // into the frame.
    let available = width.saturating_sub(3);
    let bar = if available >= PROC_BAR_MIN_COLS {
        PROC_BAR_W
    } else {
        0
    };
    let name = available
        .saturating_sub(PID_W + MEM_W + 3 + CPU_NUM_W + bar)
        .max(8);
    (name, bar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_cores_share_one_row_and_split_the_slack() {
        let (per_row, meter) = core_grid(80, 4);
        assert_eq!(per_row, 4);
        assert!((CORE_METER_MIN..=CORE_METER_MAX).contains(&meter));
        // Whatever the split, a row of cells must fit inside the pane.
        assert!(per_row * (CORE_LABEL + meter + CORE_TAIL) <= 78);
    }

    #[test]
    fn a_core_row_never_overflows_the_pane_at_any_width() {
        for width in 8..200usize {
            for cores in [0usize, 1, 2, 4, 8, 16, 64] {
                let (per_row, meter) = core_grid(width, cores);
                assert!(per_row >= 1, "per_row must be chunkable at {width}/{cores}");
                assert!(per_row <= cores.max(1));
                // The minimum meter can still overflow a pane too narrow for
                // even one cell — that clips, which is fine. What must not
                // happen is a wide pane laying out more than it can hold.
                if width >= CORE_LABEL + CORE_METER_MIN + CORE_TAIL + 2 {
                    assert!(
                        per_row * (CORE_LABEL + meter + CORE_TAIL) <= width - 2,
                        "{cores} cores at width {width}: {per_row}x{meter}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_graph_yields_to_the_process_table_before_it_yields_anything_else() {
        // A short pane gets no graph at all rather than a two-row one.
        assert_eq!(graph_rows(14, 1), 0);
        assert_eq!(graph_rows(0, 4), 0);
        // A tall one gets the full graph and still has table rows left.
        assert_eq!(graph_rows(44, 1), GRAPH_MAX_ROWS);
        for height in 0..80usize {
            for core_rows in 1..5usize {
                let graph = graph_rows(height, core_rows);
                assert!(graph == 0 || graph >= GRAPH_MIN_ROWS, "height {height}");
                assert!(graph <= GRAPH_MAX_ROWS);
                if graph > 0 {
                    // Whatever the graph takes, the reserved table rows and
                    // every fixed line still fit. This is the same arithmetic
                    // `draw` does when it sizes the table, so if the two ever
                    // drift the table gets a negative row count.
                    assert!(
                        graph + 6 + core_rows + PROC_ROWS_RESERVED <= height,
                        "graph {graph} at height {height} with {core_rows} core rows"
                    );
                }
            }
        }
    }

    #[test]
    fn the_process_meter_is_dropped_before_the_command_name_is_squeezed() {
        let (name, bar) = proc_columns(100);
        assert_eq!(bar, PROC_BAR_W);
        assert!(name > 30, "a wide pane should spend the slack on the name");
        let (narrow_name, narrow_bar) = proc_columns(50);
        assert_eq!(narrow_bar, 0);
        assert!(narrow_name >= 8);
        // Even absurdly narrow, the name column stays usable rather than zero.
        assert_eq!(proc_columns(0).0, 8);
    }

    #[test]
    fn a_process_row_fits_the_pane_it_was_measured_for() {
        for width in PROC_BAR_MIN_COLS + 2..160usize {
            let (name, bar) = proc_columns(width);
            let row = 2 + PID_W + name + MEM_W + 3 + CPU_NUM_W + bar;
            assert!(row <= width, "row {row} overflows width {width}");
        }
    }
}
