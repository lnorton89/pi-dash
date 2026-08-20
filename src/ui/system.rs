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

use super::gauge::{self, Glyphs, Ramp};
use super::{field, header_style, pane_block, push_if_fits, threshold_color, DIM, GUTTER};
use crate::app::App;
use crate::format::{human_kb, uptime};

/// The aggregate CPU and memory meters size themselves to the pane, within
/// these bounds. Below the minimum a meter is too coarse to read a trend off;
/// above the maximum it is just a long line that pushes the numbers away from
/// the label they belong to.
const METER_MIN: usize = 12;
const METER_MAX: usize = 24;

/// The COMMAND LINE heading, or nothing when that column was dropped.
fn cmdline_heading(cmdline_w: usize) -> &'static str {
    if cmdline_w > 0 {
        "COMMAND LINE"
    } else {
        ""
    }
}

/// Process table columns. The command name gets whatever is left.
const PID_W: usize = 8;
const MEM_W: usize = 7;
/// `999.9 ` — wide enough for a process pegging four cores.
const CPU_NUM_W: usize = 6;
const PROC_BAR_W: usize = 10;
/// Below this the per-process meter is dropped and the name keeps the room.
const PROC_BAR_MIN_COLS: usize = 66;
/// The comm column, once there is a command line beside it to be greedy.
const PROC_NAME_W: usize = 18;
/// Less command line than this shows a truncated executable path and none of
/// the arguments — which is the only part the COMMAND column beside it does
/// not already tell you, so below this the column is pure cost.
const PROC_CMDLINE_MIN: usize = 30;

/// Rows the history graph is allowed, and the point below which it is not
/// worth drawing at all. Two rows of braille is eight levels, which turns
/// every interesting shape into the same flat-topped block.
const GRAPH_MAX_ROWS: usize = 6;
const GRAPH_MIN_ROWS: usize = 3;
/// Process rows the graph will not eat into. A history graph above an empty
/// table answers the wrong question.
const PROC_ROWS_RESERVED: usize = 5;

pub(crate) fn draw(frame: &mut Frame, area: Rect, app: &App) {
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
    let cores = system.core_pct.len();

    let mut lines: Vec<Line> = Vec::new();

    // ── CPU: history graph on the left, the core column on the right ──
    //
    // This is btop's CPU box, and it is the layout because it is the one that
    // answers both questions at once: the graph says what the box has been
    // doing, the column says which core is doing it. Stacking them, which is
    // what the first cut did, spends six rows on the graph and then repeats
    // the same information in a grid underneath.
    //
    // The graph is also the only part of the pane that redraws wholesale on a
    // sample, because the series shifts a column left. That is a sample-tick
    // cost, not a frame cost: the loop's other reason to redraw is the header
    // clock, and on those frames the graph diffs to nothing.
    let plan = cpu_layout(width, inner.height as usize, cores);
    let core_meter = plan.core_meter;

    if plan.side_by_side {
        let graph = gauge::area_graph(
            &system.cpu_history,
            plan.graph_width,
            plan.rows,
            glyphs,
            Ramp::Load,
        );
        // Row 0 is the aggregate, then one row per core, then the load line.
        // Any rows left over are graph-only.
        for (row, graph_row) in graph.into_iter().enumerate() {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(graph_row.spans);
            spans.push(Span::raw("  "));
            match row {
                0 => spans.extend(cpu_summary(system, glyphs, plan.core_meter)),
                r if r <= cores => spans.extend(core_row(system, r - 1, glyphs, plan)),
                r if r == cores + 1 => spans.push(Span::styled(
                    format!(
                        "load {:.2} {:.2} {:.2}",
                        system.load[0], system.load[1], system.load[2]
                    ),
                    Style::default().fg(DIM),
                )),
                _ => {}
            }
            lines.push(Line::from(spans));
        }
    } else {
        // Too narrow to sit side by side: graph across the top if it fits at
        // all, cores packed into a grid under it.
        if plan.rows > 0 && width > GUTTER + 1 {
            for row in gauge::area_graph(
                &system.cpu_history,
                width - GUTTER - 1,
                plan.rows,
                glyphs,
                Ramp::Load,
            ) {
                lines.push(field("", row.spans));
            }
        }

        let cpu = system.cpu_pct.unwrap_or(0.0);
        let mut cpu_spans = gauge::bar(cpu / 100.0, meter_width, glyphs, Ramp::Load);
        let cpu_value = cpu_value(system, glyphs);
        let mut used = GUTTER + meter_width + cpu_value.chars().count();
        cpu_spans.push(Span::styled(
            cpu_value,
            Style::default()
                .fg(threshold_color(cpu, 60.0, 85.0))
                .add_modifier(Modifier::BOLD),
        ));
        push_if_fits(&mut cpu_spans, &mut used, width, format!("  {cores} cores"));
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

        for (row, chunk) in system.core_pct.chunks(plan.per_row).enumerate() {
            let mut spans = vec![Span::raw("  ")];
            for (offset, pct) in chunk.iter().enumerate() {
                let index = row * plan.per_row + offset;
                let value = pct.unwrap_or(0.0);
                // Two-digit padding so c8..c15 on a 16-core box do not shunt
                // every meter after them one column right.
                spans.push(Span::styled(
                    format!("c{index:<2} "),
                    Style::default().fg(DIM),
                ));
                spans.extend(gauge::bar(value / 100.0, core_meter, glyphs, Ramp::Load));
                spans.push(Span::styled(
                    format!("{value:>4.0}%  "),
                    Style::default().fg(threshold_color(value, 60.0, 85.0)),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    // ── memory ──
    let mem = &system.mem;
    let mem_pct = mem.used_pct();
    let mut mem_spans = gauge::bar(mem_pct / 100.0, meter_width, glyphs, Ramp::Load);
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
    let mut cache_spans = gauge::bar(cache_pct / 100.0, meter_width, glyphs, Ramp::Cool);
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
        let cols = proc_columns(width);
        let (name_w, cmd_w, bar_w) = (cols.name, cols.cmdline, cols.bar);
        // CPU% is right-aligned over the numbers below it, not left over the
        // space before them.
        //
        // The column the table is ordered by is coloured rather than marked
        // with a glyph. Two orders that both put a big number at the top are
        // otherwise told apart only by staring at the rows — but a marker
        // character would have to come out of a column whose width is already
        // exactly what the numbers under it need, and any arrow worth reading
        // is outside ASCII, which is the one thing the framebuffer console
        // cannot draw.
        let sorted = system.sort.label();
        let heading = |text: &'static str| {
            let style = if text == sorted {
                header_style().fg(app.accent)
            } else {
                header_style()
            };
            Span::styled(text, style)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "  {:<PID_W$}{:<name_w$}{:<cmd_w$}",
                    "PID",
                    "COMMAND",
                    cmdline_heading(cmd_w)
                ),
                header_style(),
            ),
            // MEM is right-aligned, so its padding is a span of its own and
            // the word itself can carry a different colour from the gap.
            Span::styled(format!("{:>1$}", "", MEM_W - 3), header_style()),
            heading("MEM"),
            Span::styled("   ", header_style()),
            heading("CPU%"),
        ]));
        // Only the busiest are listed. A Pi runs a couple of hundred mostly
        // idle processes and a full table is a scrolling wall you never read;
        // what you want to know is which one just woke up.
        let shade = gauge::row_shade();
        for (index, proc) in system.procs.iter().take(rows).enumerate() {
            let mut spans = vec![
                Span::styled(format!("  {:<PID_W$}", proc.pid), Style::default().fg(DIM)),
                Span::styled(
                    // Clipped one short of the column so a name that fills it
                    // still leaves a gap. At the full width `dump1090-mutabil`
                    // ran straight into the command line beside it.
                    format!("{:<name_w$}", crate::format::clip(&proc.name, name_w - 1)),
                    Style::default().fg(if proc.state == 'D' {
                        // Uninterruptible sleep: on this box that is almost
                        // always the SD card, and it is worth spotting.
                        super::WARN
                    } else {
                        Color::White
                    }),
                ),
            ];
            if cmd_w > 0 {
                // A kernel thread has no cmdline at all. `ps` brackets the
                // comm in that case and so does this, rather than leaving a
                // blank column that reads as a failed read.
                let (text, style) = if proc.cmdline.is_empty() {
                    (format!("[{}]", proc.name), Style::default().fg(DIM))
                } else {
                    (proc.cmdline.clone(), Style::default().fg(Color::Gray))
                };
                spans.push(Span::styled(
                    format!("{:<cmd_w$}", crate::format::clip(&text, cmd_w - 1)),
                    style,
                ));
            }
            spans.push(Span::raw(format!("{:>MEM_W$}   ", human_kb(proc.rss_kb))));
            spans.push(Span::styled(
                format!("{:>5.1} ", proc.cpu_pct),
                Style::default().fg(threshold_color(proc.cpu_pct, 25.0, 75.0)),
            ));
            if bar_w > 0 {
                // Scaled against one core, which is what the number beside it
                // means. A four-thread build reads 380% and pegs the bar —
                // that is the honest reading, not a clipped one.
                spans.extend(gauge::bar(proc.cpu_pct / 100.0, bar_w, glyphs, Ramp::Load));
            }
            let mut line = Line::from(spans);
            // Alternating rows, as btop stripes its process list. Sixty rows
            // of same-coloured text is a wall the eye slides off; the stripe
            // is what lets you track one row's numbers across the columns.
            if let Some(shade) = shade.filter(|_| index % 2 == 1) {
                // The stripe has to reach the frame, so the row is padded out
                // to the pane. Styling the Line alone paints only the cells
                // its spans already cover, which stops at the last character.
                let drawn: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                if drawn < width {
                    line.spans.push(Span::raw(" ".repeat(width - drawn)));
                }
                line = line.style(Style::default().bg(shade));
            }
            lines.push(line);
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

/// How the CPU block lays itself out for a given pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CpuLayout {
    /// Graph beside the core column (btop's shape) rather than above a grid.
    pub(crate) side_by_side: bool,
    /// Rows the graph gets. Zero means no graph — there was not room for one
    /// worth looking at.
    pub(crate) rows: usize,
    /// Cells of graph, in the side-by-side layout.
    pub(crate) graph_width: usize,
    /// Width of each per-core meter.
    pub(crate) core_meter: usize,
    /// Cells of sparkline beside each core meter. Zero drops them.
    pub(crate) core_spark: usize,
    /// Cores per row, in the stacked fallback.
    pub(crate) per_row: usize,
}

/// Columns the core column spends on things that are not its meter:
/// `c12 ` in front, ` 100% ` behind.
const CORE_LABEL: usize = 4;
const CORE_TAIL: usize = 7;
const CORE_METER_MIN: usize = 6;
const CORE_METER_MAX: usize = 14;
/// Enough graph to be worth the columns it takes. Below this the pane falls
/// back to stacking, where the graph gets the pane's whole width.
const GRAPH_MIN_WIDTH: usize = 30;

/// Plans the CPU block.
///
/// The side-by-side form needs a pane wide enough for a real graph *and* a
/// core column, and short enough in cores that the column does not run past
/// the graph. Sixteen cores in a vertical list is 18 rows of CPU before the
/// memory meters start, which is not a summary any more — those fall back to
/// the grid.
pub(crate) fn cpu_layout(width: usize, height: usize, cores: usize) -> CpuLayout {
    let available = width.saturating_sub(2);
    let column = CORE_LABEL + CORE_METER_MAX + CORE_TAIL + CORE_SPARK + 2;
    // cpu + one row per core + load.
    let wanted_rows = cores + 2;

    let side_by_side = cores > 0
        && cores <= SIDE_BY_SIDE_MAX_CORES
        && available >= column + GRAPH_MIN_WIDTH
        // The block is as tall as the core column, so the same rows the
        // stacked layout has to find for a graph have to be found here.
        && height >= wanted_rows + FIXED_ROWS + PROC_ROWS_RESERVED;

    if side_by_side {
        return CpuLayout {
            side_by_side: true,
            rows: wanted_rows,
            graph_width: available - column,
            core_meter: CORE_METER_MAX,
            core_spark: CORE_SPARK,
            per_row: 1,
        };
    }

    let (per_row, core_meter) = core_grid(width, cores);
    let core_rows = if cores == 0 {
        0
    } else {
        cores.div_ceil(per_row)
    };
    CpuLayout {
        side_by_side: false,
        rows: stacked_graph_rows(height, core_rows),
        graph_width: width.saturating_sub(GUTTER + 1),
        core_meter,
        // A grid cell has no room for one, and the grid exists because the
        // pane was short of room.
        core_spark: 0,
        per_row,
    }
}

/// Cores past which the vertical column stops being a summary.
const SIDE_BY_SIDE_MAX_CORES: usize = 8;
const CORE_SPARK: usize = 8;
/// Non-CPU lines the pane always writes: mem, cache, swap, blank, table
/// header.
const FIXED_ROWS: usize = 5;

/// How many per-core meters fit on a grid row, and how wide each one gets.
///
/// Packs as many cores per row as the minimum cell allows, then spends the
/// leftover columns on widening the meters rather than on trailing space.
/// `per_row` is never zero, so callers can chunk by it.
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

/// Rows for the graph in the stacked layout. Zero when there is not enough
/// room for one worth looking at.
fn stacked_graph_rows(height: usize, core_rows: usize) -> usize {
    // The cpu line, the core grid, and the fixed lines below them.
    let fixed = 1 + core_rows + FIXED_ROWS;
    let spare = height.saturating_sub(fixed + PROC_ROWS_RESERVED);
    if spare < GRAPH_MIN_ROWS {
        0
    } else {
        spare.min(GRAPH_MAX_ROWS)
    }
}

/// The percentage that sits beside the aggregate meter.
fn cpu_value(system: &crate::panes::system::SystemPane, glyphs: Glyphs) -> String {
    match system.cpu_pct {
        Some(pct) => format!(" {pct:>3.0}%"),
        // The first sample has no previous one to difference against; showing
        // 0% there would be a lie you cannot distinguish from an idle box.
        None => match glyphs {
            Glyphs::Unicode => "   —".to_string(),
            Glyphs::Ascii => "   -".to_string(),
        },
    }
}

/// The `CPU  ████░░  20%` line at the head of the core column.
fn cpu_summary<'a>(
    system: &crate::panes::system::SystemPane,
    glyphs: Glyphs,
    meter: usize,
) -> Vec<Span<'a>> {
    let cpu = system.cpu_pct.unwrap_or(0.0);
    let mut spans = vec![Span::styled(
        "CPU ",
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    )];
    spans.extend(gauge::bar(cpu / 100.0, meter, glyphs, Ramp::Load));
    spans.push(Span::styled(
        cpu_value(system, glyphs),
        Style::default()
            .fg(threshold_color(cpu, 60.0, 85.0))
            .add_modifier(Modifier::BOLD),
    ));
    spans
}

/// One `c0  ████░░  14%  ⣀⣠⣤` line of the core column.
fn core_row<'a>(
    system: &crate::panes::system::SystemPane,
    index: usize,
    glyphs: Glyphs,
    plan: CpuLayout,
) -> Vec<Span<'a>> {
    let value = system.core_pct.get(index).copied().flatten().unwrap_or(0.0);
    // Two-digit padding so c8..c15 do not shunt every meter one column right.
    let mut spans = vec![Span::styled(
        format!("c{index:<2} "),
        Style::default().fg(DIM),
    )];
    spans.extend(gauge::bar(
        value / 100.0,
        plan.core_meter,
        glyphs,
        Ramp::Load,
    ));
    spans.push(Span::styled(
        format!("{value:>4.0}% "),
        Style::default().fg(threshold_color(value, 60.0, 85.0)),
    ));
    if plan.core_spark > 0 {
        spans.push(Span::raw(" "));
        spans.extend(gauge::sparkline(
            system.core_history.get(index).map_or(&[][..], |h| &h[..]),
            plan.core_spark,
            glyphs,
            Ramp::Load,
        ));
    }
    spans
}

/// Widths for the process table's flexible columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcColumns {
    pub(crate) name: usize,
    /// Zero on a pane too narrow to carry a command line as well as a name.
    pub(crate) cmdline: usize,
    /// Zero on a pane too narrow for the per-process meter.
    pub(crate) bar: usize,
}

/// Lays out the process table.
///
/// Things are dropped in the order they stop paying for themselves: the meter
/// first, because the number beside it says the same thing; the command line
/// next, because the name is the part you scan for. Whatever survives, the
/// greedy column absorbs the slack — a wide pane full of trailing space with
/// the numbers stranded at the far right is the layout this replaced.
fn proc_columns(width: usize) -> ProcColumns {
    // Two columns of indent, and one on the right so the longest row does not
    // run into the frame.
    let available = width.saturating_sub(3);
    let bar = if available >= PROC_BAR_MIN_COLS {
        PROC_BAR_W
    } else {
        0
    };
    let fixed = PID_W + MEM_W + 3 + CPU_NUM_W + bar;
    let flexible = available.saturating_sub(fixed);

    if flexible >= PROC_NAME_W + PROC_CMDLINE_MIN {
        ProcColumns {
            name: PROC_NAME_W,
            cmdline: flexible - PROC_NAME_W,
            bar,
        }
    } else {
        ProcColumns {
            name: flexible.max(8),
            cmdline: 0,
            bar,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_tall_pane_puts_the_graph_beside_the_cores() {
        let plan = cpu_layout(150, 40, 4);
        assert!(plan.side_by_side);
        // cpu + four cores + load.
        assert_eq!(plan.rows, 6);
        assert!(plan.graph_width >= GRAPH_MIN_WIDTH);
        assert_eq!(plan.core_spark, CORE_SPARK);
    }

    #[test]
    fn a_narrow_or_short_or_many_cored_pane_falls_back_to_the_grid() {
        // Not enough width for a graph *and* a column.
        assert!(!cpu_layout(60, 40, 4).side_by_side);
        // Enough width, but the block would leave no process table.
        assert!(!cpu_layout(150, 12, 4).side_by_side);
        // Sixteen cores in a vertical list is 18 rows before the memory
        // meters start, which is not a summary any more.
        assert!(!cpu_layout(150, 60, 16).side_by_side);
        // A machine reporting no cores at all must not claim the layout.
        assert!(!cpu_layout(150, 40, 0).side_by_side);
    }

    #[test]
    fn the_side_by_side_block_always_fits_the_pane_it_was_measured_for() {
        for width in 30..220usize {
            for height in 4..70usize {
                for cores in [0usize, 1, 2, 4, 8, 16, 64] {
                    let plan = cpu_layout(width, height, cores);
                    if !plan.side_by_side {
                        continue;
                    }
                    // Two of indent, the graph, two of gap, then everything
                    // `core_row` draws.
                    let column = CORE_LABEL + plan.core_meter + CORE_TAIL + plan.core_spark;
                    assert!(
                        2 + plan.graph_width + 2 + column <= width,
                        "{cores} cores at {width}x{height}: graph {} + column {column}",
                        plan.graph_width
                    );
                    // And the rows it claims leave the table its reservation.
                    assert!(
                        plan.rows + FIXED_ROWS + PROC_ROWS_RESERVED <= height,
                        "{cores} cores at {width}x{height}: {} rows",
                        plan.rows
                    );
                }
            }
        }
    }

    #[test]
    fn a_grid_row_never_overflows_the_pane_at_any_width() {
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
    fn the_stacked_graph_yields_to_the_process_table_before_anything_else() {
        // A short pane gets no graph at all rather than a two-row one.
        assert_eq!(stacked_graph_rows(14, 1), 0);
        assert_eq!(stacked_graph_rows(0, 4), 0);
        assert_eq!(stacked_graph_rows(44, 1), GRAPH_MAX_ROWS);
        for height in 0..80usize {
            for core_rows in 1..5usize {
                let graph = stacked_graph_rows(height, core_rows);
                assert!(graph == 0 || graph >= GRAPH_MIN_ROWS, "height {height}");
                assert!(graph <= GRAPH_MAX_ROWS);
                if graph > 0 {
                    // Whatever the graph takes, the reserved table rows and
                    // every fixed line still fit. This is the same arithmetic
                    // `draw` does when it sizes the table, so if the two ever
                    // drift the table gets a negative row count.
                    assert!(
                        graph + 1 + core_rows + FIXED_ROWS + PROC_ROWS_RESERVED <= height,
                        "graph {graph} at height {height} with {core_rows} core rows"
                    );
                }
            }
        }
    }

    #[test]
    fn the_table_sheds_columns_in_order_of_what_they_are_worth() {
        // Wide: meter, name and a greedy command line.
        let wide = proc_columns(140);
        assert_eq!(wide.bar, PROC_BAR_W);
        assert_eq!(wide.name, PROC_NAME_W);
        assert!(wide.cmdline >= PROC_CMDLINE_MIN);

        // Narrower: the command line goes and the name takes the slack, so
        // the columns never strand the numbers at the far right.
        let middling = proc_columns(80);
        assert_eq!(middling.cmdline, 0);
        assert!(middling.name > PROC_NAME_W);

        // Narrower still: the meter goes too.
        assert_eq!(proc_columns(50).bar, 0);
        // Even absurdly narrow, the name stays usable rather than zero.
        assert_eq!(proc_columns(0).name, 8);
    }

    #[test]
    fn a_process_row_fits_the_pane_it_was_measured_for() {
        for width in 20..220usize {
            let c = proc_columns(width);
            let row = 2 + PID_W + c.name + c.cmdline + MEM_W + 3 + CPU_NUM_W + c.bar;
            // A pane too narrow for even the fixed columns clips, which is
            // fine. What must not happen is a pane laying out more than it
            // has and stranding the CPU figure past the frame.
            if width >= 40 {
                assert!(row <= width, "row {row} overflows width {width}");
            }
        }
    }
}
