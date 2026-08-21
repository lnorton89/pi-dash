//! Application state: the four panes, what has focus, and the sample clock.

use std::time::Instant;

use ratatui::style::Color;

use crate::config::Config;
use crate::panes::classg::ClassgPane;
use crate::panes::health::HealthPane;
use crate::panes::radios::RadiosPane;
use crate::panes::system::SystemPane;
use crate::ui::gauge::Glyphs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Help,
}

/// The panes, in the order `Tab` cycles them. Focus only changes what is
/// visible on a terminal too narrow for the two-column layout; in the wide
/// layout every pane is on screen at once and focus is irrelevant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    System,
    Health,
    Radios,
    Classg,
}

impl Pane {
    pub(crate) const ALL: [Pane; 4] = [Pane::System, Pane::Health, Pane::Radios, Pane::Classg];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Pane::System => "System",
            Pane::Health => "Pi health",
            Pane::Radios => "Radios & network",
            Pane::Classg => "ClassG",
        }
    }

    pub(crate) fn next(self) -> Pane {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub(crate) fn previous(self) -> Pane {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

pub(crate) struct App {
    pub(crate) config: Config,
    pub(crate) mode: Mode,
    pub(crate) focus: Pane,
    pub(crate) host: String,
    pub(crate) accent: Color,
    /// Resolved once here rather than parsed per frame: every meter, graph
    /// and pane border on screen asks for it.
    pub(crate) glyphs: Glyphs,
    pub(crate) system: SystemPane,
    pub(crate) health: HealthPane,
    pub(crate) radios: RadiosPane,
    pub(crate) classg: ClassgPane,
}

impl App {
    pub(crate) fn new(config: Config) -> Self {
        let classg =
            ClassgPane::spawn(config.api.clone(), config.credential(), config.api_interval);
        App {
            accent: color_from_str(&config.theme),
            glyphs: Glyphs::parse(&config.glyphs),
            host: hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "pi".to_string()),
            mode: Mode::Normal,
            focus: Pane::System,
            system: SystemPane::default(),
            health: HealthPane::default(),
            radios: RadiosPane::default(),
            classg,
            config,
        }
    }

    /// One round of local sampling. The ClassG pane is not touched here: it
    /// runs on its own clock in its own thread, so a slow API never delays
    /// the temperature reading.
    pub(crate) fn sample(&mut self, now: Instant) {
        self.system.sample(now);
        self.health.sample(now);
        self.radios.sample(
            now,
            &self.config.usb_vendor_ids,
            &self.config.ignore_interfaces,
        );
    }
}

/// Maps a colour name to a ratatui colour. Unknown names fall back to cyan
/// rather than erroring: a typo in the theme should not stop the dashboard.
pub(crate) fn color_from_str(name: &str) -> Color {
    match name.trim().to_ascii_lowercase().as_str() {
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "lightred" | "light_red" => Color::LightRed,
        "lightgreen" | "light_green" => Color::LightGreen,
        "lightyellow" | "light_yellow" => Color::LightYellow,
        "lightblue" | "light_blue" => Color::LightBlue,
        "lightmagenta" | "light_magenta" => Color::LightMagenta,
        "lightcyan" | "light_cyan" => Color::LightCyan,
        _ => Color::Cyan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycles_all_four_panes_and_wraps() {
        let mut pane = Pane::System;
        for _ in 0..Pane::ALL.len() {
            pane = pane.next();
        }
        assert_eq!(pane, Pane::System);
        assert_eq!(Pane::System.previous(), Pane::Classg);
        assert_eq!(Pane::Classg.next(), Pane::System);
    }

    #[test]
    fn an_unknown_theme_name_falls_back_rather_than_failing() {
        assert_eq!(color_from_str("green"), Color::Green);
        assert_eq!(color_from_str("  GREEN "), Color::Green);
        assert_eq!(color_from_str("chartreuse"), Color::Cyan);
        assert_eq!(color_from_str(""), Color::Cyan);
    }
}
