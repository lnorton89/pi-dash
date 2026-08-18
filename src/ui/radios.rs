//! The radios pane: interfaces, monitor-mode state, and USB radio presence.
//!
//! Both tables here carry column headings. Without them the interface rows
//! were four unlabelled numbers and a word — `v253B  ^2.3K` needs a legend
//! nobody has, and `dorm` next to `unkn` reads as noise until something says
//! the column is a link state.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{pane_block, table_header, BAD, DIM, OK, WARN};
use crate::app::App;
use crate::format::{clip, human_rate_compact};
use crate::panes::radios::{Iface, WirelessMode};

/// Interface table columns. The pane is handed 48-60 columns by the layout,
/// so everything here is sized for the narrow end and the driver column is
/// what the wide end spends its slack on.
const NAME_W: usize = 8;
const STATE_W: usize = 6;
const RATE_W: usize = 8;
const MODE_W: usize = 8;
const CH_W: usize = 5;
/// Columns the table needs before a driver column is worth adding.
const DRIVER_MIN_COLS: usize = 12;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_block(" Radios & network ", app.accent, app.glyphs);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let radios = &app.radios;
    let width = inner.width as usize;
    let fixed = 2 + NAME_W + STATE_W + RATE_W * 2 + MODE_W + CH_W;
    let slack = width.saturating_sub(fixed);
    // A driver name in four columns is `bcm` — worse than no column, because
    // it looks like a value rather than a truncation.
    let driver_w = if slack >= DRIVER_MIN_COLS { slack } else { 0 };

    let mut lines: Vec<Line> = Vec::new();

    if radios.ifaces.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no interfaces (is this Linux?)",
            Style::default().fg(DIM),
        )));
    } else {
        lines.push(table_header(format!(
            "  {:<NAME_W$}{:<STATE_W$}{:<RATE_W$}{:<RATE_W$}{:<MODE_W$}{:<CH_W$}{}",
            "IFACE",
            "LINK",
            "RX",
            "TX",
            "MODE",
            "CH",
            if driver_w > 0 { "DRIVER" } else { "" }
        )));
        for iface in &radios.ifaces {
            lines.push(iface_row(iface, driver_w));
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  USB radios",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    if radios.usb.is_empty() {
        // A radio that vanishes off the bus is the documented degraded mode
        // (ClassG ADR-0003), so it gets a red line rather than an empty list
        // that reads the same as "not looked yet".
        lines.push(Line::from(vec![
            Span::styled("  none present", Style::default().fg(BAD)),
            Span::styled(" - adapters gone from the bus", Style::default().fg(DIM)),
        ]));
    } else {
        lines.push(table_header(format!("  {:<12}{}", "VID:PID", "DEVICE")));
        for device in &radios.usb {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<12}", device.id), Style::default().fg(DIM)),
                Span::raw(clip(&device.description, width.saturating_sub(14).max(1))),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn iface_row<'a>(iface: &Iface, driver_w: usize) -> Line<'a> {
    // `dormant` and `unknown` do not fit the column and are not worth widening
    // it for: a wireless interface in monitor mode reports `unknown` forever,
    // because there is no association to have an opinion about.
    let (state, state_color) = match iface.state.as_str() {
        "up" => ("up", OK),
        "down" => ("down", BAD),
        "dormant" => ("dorm", WARN),
        "unknown" => ("unkn", DIM),
        other => (other, DIM),
    };

    let mut spans = vec![
        Span::styled(
            format!("  {:<NAME_W$}", clip(&iface.name, NAME_W - 1)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<STATE_W$}", clip(state, STATE_W - 1)),
            Style::default().fg(state_color),
        ),
        Span::raw(format!("{:<RATE_W$}", human_rate_compact(iface.rx_bps))),
        Span::raw(format!("{:<RATE_W$}", human_rate_compact(iface.tx_bps))),
    ];

    match iface.mode {
        // Monitor is the one that has to be right for this project to work at
        // all, so it is the only mode drawn in colour.
        Some(WirelessMode::Monitor) => spans.push(Span::styled(
            format!("{:<MODE_W$}", "monitor"),
            Style::default().fg(OK).add_modifier(Modifier::BOLD),
        )),
        Some(WirelessMode::Managed) => spans.push(Span::styled(
            format!("{:<MODE_W$}", "managed"),
            Style::default().fg(DIM),
        )),
        // A wired interface has no mode and no channel, but the columns after
        // it still have to line up with the wireless rows above and below.
        None => spans.push(Span::raw(" ".repeat(MODE_W))),
    }

    spans.push(Span::styled(
        match iface.channel {
            Some(channel) => format!("{:<CH_W$}", channel),
            None => " ".repeat(CH_W),
        },
        Style::default().fg(DIM),
    ));

    if driver_w > 0 {
        spans.push(Span::styled(
            clip(
                iface.driver.as_deref().unwrap_or("-"),
                driver_w.saturating_sub(1).max(1),
            ),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}
