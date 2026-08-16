//! The radios pane: interfaces, monitor-mode state, and USB radio presence.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{pane_block, BAD, DIM, OK};
use crate::app::App;
use crate::format::{clip, human_rate_compact};
use crate::panes::radios::WirelessMode;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_block(" Radios & network ", app.accent);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let radios = &app.radios;
    let mut lines: Vec<Line> = Vec::new();

    if radios.ifaces.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no interfaces (is this Linux?)",
            Style::default().fg(DIM),
        )));
    }

    for iface in &radios.ifaces {
        let mut spans = vec![
            Span::raw(format!("  {:<8}", clip(&iface.name, 8))),
            Span::styled(
                format!("{:<4}", clip(&iface.state, 4)),
                Style::default().fg(if iface.is_up() { OK } else { BAD }),
            ),
            // v is received, ^ is sent — one glyph each, because the pane is
            // 46 columns wide and "rx"/"tx" cost four of them per row.
            Span::styled(
                format!(" v{:<7}", human_rate_compact(iface.rx_bps)),
                Style::default().fg(DIM),
            ),
            Span::styled(
                format!("^{:<7}", human_rate_compact(iface.tx_bps)),
                Style::default().fg(DIM),
            ),
        ];
        match iface.mode {
            Some(WirelessMode::Monitor) => spans.push(Span::styled(
                "monitor",
                Style::default().fg(OK).add_modifier(Modifier::BOLD),
            )),
            Some(WirelessMode::Managed) => {
                spans.push(Span::styled("managed", Style::default().fg(DIM)))
            }
            None => {}
        }
        if let Some(channel) = iface.channel {
            spans.push(Span::styled(
                format!(" ch{channel}"),
                Style::default().fg(DIM),
            ));
        }
        lines.push(Line::from(spans));
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
        for device in &radios.usb {
            lines.push(Line::from(vec![
                Span::styled(format!("  {}  ", device.id), Style::default().fg(DIM)),
                Span::styled(
                    clip(&device.description, inner.width.saturating_sub(14) as usize),
                    Style::default().fg(DIM),
                ),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
