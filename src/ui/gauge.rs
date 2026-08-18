//! btop-style meters and history graphs.
//!
//! Three things separate a btop meter from the flat `[####....]` one this
//! replaced, and all three are cheap:
//!
//! - the fill is coloured *per cell* along a green→amber→red ramp, so where a
//!   bar ends is legible from across a room without reading the number;
//! - a partly-filled cell is drawn at eighth-of-a-cell precision rather than
//!   rounded to a whole one, which is what stops a 16-cell meter from moving
//!   in visible 6% jumps;
//! - the aggregate CPU gets a scrolling history graph, because a Pi pinned at
//!   100% and a Pi that spikes to 100% once a minute show the same instant
//!   number and are completely different problems.

use std::sync::OnceLock;

use ratatui::{
    style::{Color, Style},
    symbols::border,
    text::{Line, Span},
};

use super::DIM;

/// Which characters meters, graphs and borders are drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Glyphs {
    /// Block and braille drawing characters — the btop look.
    #[default]
    Unicode,
    /// Plain ASCII.
    ///
    /// The Pi's own HDMI console (`TERM=linux`) runs a framebuffer font with
    /// no box-drawing or braille glyphs, and every meter on it comes out as a
    /// row of replacement characters. This mode also swaps the pane borders,
    /// which have exactly the same problem: drawing ASCII meters inside a
    /// Unicode box, which is what this pane used to do, fixed half of it and
    /// left the frame broken.
    Ascii,
}

impl Glyphs {
    pub fn parse(name: &str) -> Glyphs {
        match name.trim().to_ascii_lowercase().as_str() {
            "ascii" | "plain" => Glyphs::Ascii,
            // Unknown values fall back rather than failing to start, the same
            // rule the theme name follows.
            _ => Glyphs::Unicode,
        }
    }

    pub fn border_set(self) -> border::Set {
        match self {
            Glyphs::Unicode => border::PLAIN,
            Glyphs::Ascii => ASCII_BORDER,
        }
    }

    /// The character a fully-filled meter cell uses.
    fn full(self) -> char {
        match self {
            Glyphs::Unicode => '█',
            Glyphs::Ascii => '#',
        }
    }

    /// The unfilled track behind a meter. Drawn, not left blank: an empty gap
    /// gives no sense of how much headroom is left.
    fn track(self) -> char {
        match self {
            Glyphs::Unicode => '░',
            Glyphs::Ascii => '.',
        }
    }

    /// A cell filled `eighths`/8 of the way across, for the single cell at the
    /// end of the fill. `0` is the bare track.
    fn partial(self, eighths: usize) -> char {
        match self {
            // U+258F..U+2588, one eighth through eight eighths.
            Glyphs::Unicode => match eighths {
                0 => self.track(),
                1 => '▏',
                2 => '▎',
                3 => '▍',
                4 => '▌',
                5 => '▋',
                6 => '▊',
                7 => '▉',
                _ => '█',
            },
            // ASCII has no sub-cell fill, so it rounds at the half.
            Glyphs::Ascii => {
                if eighths >= 4 {
                    '#'
                } else {
                    '.'
                }
            }
        }
    }
}

/// Single-line ASCII frame, for the framebuffer console.
const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// How many colours the terminal can be trusted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The sixteen ANSI colours, which is all the Linux framebuffer console
    /// has. The ramp is coarse but the hue progression still reads.
    Ansi16,
    /// The 256-colour cube, which is what everything reaching this box over
    /// SSH actually supports. Close enough to btop's truecolor gradient that
    /// you cannot tell at a glance.
    Cube256,
}

/// Decides the depth from the two variables that carry the answer. Split out
/// from the environment lookup so it can be tested without setting
/// process-wide state under a threaded test runner.
pub fn detect_depth(term: Option<&str>, colorterm: Option<&str>) -> Depth {
    // COLORTERM is only set by terminals that mean it, and anything claiming
    // truecolor certainly has the 256-colour cube underneath.
    if colorterm.is_some_and(|c| !c.trim().is_empty()) {
        return Depth::Cube256;
    }
    match term {
        Some(t) if t.contains("256color") || t.contains("direct") => Depth::Cube256,
        // Notably TERM=linux, the Pi's own console, lands here.
        _ => Depth::Ansi16,
    }
}

fn depth() -> Depth {
    static DEPTH: OnceLock<Depth> = OnceLock::new();
    *DEPTH.get_or_init(|| {
        detect_depth(
            std::env::var("TERM").ok().as_deref(),
            std::env::var("COLORTERM").ok().as_deref(),
        )
    })
}

/// Green at `0.0`, amber in the middle, red at `1.0`. Out of range clamps.
pub fn ramp(t: f64) -> Color {
    ramp_at(depth(), t)
}

pub fn ramp_at(depth: Depth, t: f64) -> Color {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    match depth {
        Depth::Cube256 => {
            // Walk the 6×6×6 cube's green edge round to its red one: green up
            // to yellow, yellow down to red. Eleven stops, which across a 16-
            // to 40-cell meter is smooth enough that the steps do not show.
            const STOPS: [(u8, u8); 11] = [
                (0, 5),
                (1, 5),
                (2, 5),
                (3, 5),
                (4, 5),
                (5, 5),
                (5, 4),
                (5, 3),
                (5, 2),
                (5, 1),
                (5, 0),
            ];
            let index = ((t * (STOPS.len() - 1) as f64).round() as usize).min(STOPS.len() - 1);
            let (r, g) = STOPS[index];
            // Cube index is 16 + 36r + 6g + b, each component 0..=5.
            Color::Indexed(16 + 36 * r + 6 * g)
        }
        Depth::Ansi16 => {
            // Dark yellow (colour 3) is left out on purpose: on most consoles
            // it renders olive-brown, which reads as a dead column rather than
            // as a step between green and red.
            const STOPS: [Color; 5] = [
                Color::Green,
                Color::LightGreen,
                Color::LightYellow,
                Color::LightRed,
                Color::Red,
            ];
            let index = ((t * (STOPS.len() - 1) as f64).round() as usize).min(STOPS.len() - 1);
            STOPS[index]
        }
    }
}

/// Collects same-styled characters into runs, so a 40-cell meter emits a
/// handful of spans instead of forty. Forty style changes per row per frame is
/// forty escape sequences the Pi has to push down an SSH pipe every tick.
struct Runs<'a> {
    spans: Vec<Span<'a>>,
    text: String,
    color: Option<Color>,
}

impl<'a> Runs<'a> {
    fn new() -> Self {
        Runs {
            spans: Vec::new(),
            text: String::new(),
            color: None,
        }
    }

    fn push(&mut self, color: Color, ch: char) {
        if self.color != Some(color) {
            self.flush();
            self.color = Some(color);
        }
        self.text.push(ch);
    }

    fn flush(&mut self) {
        if !self.text.is_empty() {
            let color = self.color.unwrap_or(DIM);
            self.spans.push(Span::styled(
                std::mem::take(&mut self.text),
                Style::default().fg(color),
            ));
        }
    }

    fn finish(mut self) -> Vec<Span<'a>> {
        self.flush();
        self.spans
    }
}

/// A horizontal meter, gradient-filled, exactly `width` cells wide.
pub fn bar<'a>(frac: f64, width: usize, glyphs: Glyphs) -> Vec<Span<'a>> {
    if width == 0 {
        return Vec::new();
    }
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Eighths across the whole bar, so the fill can stop part-way into a cell.
    // A non-zero value always lights at least one eighth: rounding 0.4% of a
    // 16-cell meter down to nothing makes a quiet box and a stopped sampler
    // look identical.
    let mut eighths = (frac * (width * 8) as f64).round() as usize;
    if eighths == 0 && frac > 0.0 {
        eighths = 1;
    }
    let full = eighths / 8;
    let remainder = eighths % 8;

    let mut runs = Runs::new();
    for cell in 0..width {
        // Colour by position along the bar, not by the value: that is what
        // puts the red end of every meter on screen at the same place.
        let t = if width == 1 {
            frac
        } else {
            cell as f64 / (width - 1) as f64
        };
        let (color, ch) = if cell < full {
            (ramp(t), glyphs.full())
        } else if cell == full && remainder > 0 {
            (ramp(t), glyphs.partial(remainder))
        } else {
            (DIM, glyphs.track())
        };
        runs.push(color, ch);
    }
    runs.finish()
}

/// Bit numbering inside a braille cell. Dots run 1-2-3-7 down the left column
/// and 4-5-6-8 down the right, which is *not* the order the bits sit in, so
/// the mapping has to be spelled out.
const BRAILLE_LEFT: [u8; 4] = [0, 1, 2, 6];
const BRAILLE_RIGHT: [u8; 4] = [3, 4, 5, 7];

/// A scrolling area graph of `samples` (each `0.0..=1.0`), newest on the
/// right, drawn into `rows` lines of `width` cells.
///
/// Braille packs two samples per column and four levels per row, so a 60-cell
/// graph six rows tall holds 120 samples at 24 levels — four minutes at a
/// two-second cadence, which is the window in which you can still connect a
/// spike to whatever you just ran. ASCII gets one sample per column and two
/// levels per row (`-` for the half, `#` for the whole) and is correspondingly
/// coarse; that is the price of the console font.
///
/// Colour follows height, not the current value, so the same absolute load is
/// always the same colour.
pub fn area_graph<'a>(samples: &[f64], width: usize, rows: usize, glyphs: Glyphs) -> Vec<Line<'a>> {
    if width == 0 || rows == 0 {
        return Vec::new();
    }
    let (per_cell, per_row) = match glyphs {
        Glyphs::Unicode => (2, 4),
        Glyphs::Ascii => (1, 2),
    };
    let columns = width * per_cell;
    let levels = rows * per_row;

    // Right-align: a dashboard three samples old shows three columns at the
    // right and bare track to the left, rather than stretching three points
    // across the pane and implying a history it has not collected.
    let start = samples.len().saturating_sub(columns);
    let visible = &samples[start..];
    let pad = columns - visible.len();
    let level_at = |column: usize| -> Option<usize> {
        let value = *visible.get(column.checked_sub(pad)?)?;
        let value = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(if value > 0.0 {
            ((value * levels as f64).round() as usize).clamp(1, levels)
        } else {
            0
        })
    };

    (0..rows)
        .map(|row| {
            // Rows run top-down; levels run bottom-up.
            let from_bottom = rows - 1 - row;
            let lit = ramp((from_bottom as f64 + 0.5) / rows as f64);
            let mut runs = Runs::new();

            for cell in 0..width {
                let (color, ch) = match glyphs {
                    Glyphs::Unicode => {
                        let base = from_bottom * 4;
                        let mut bits: u8 = 0;
                        for (half, dots) in [BRAILLE_LEFT, BRAILLE_RIGHT].into_iter().enumerate() {
                            let Some(level) = level_at(cell * per_cell + half) else {
                                continue;
                            };
                            for (offset, bit) in dots.iter().enumerate() {
                                // dots[0] is the top of the cell, so it stands
                                // for the highest of the four levels this row
                                // covers.
                                if level > base + (3 - offset) {
                                    bits |= 1 << bit;
                                }
                            }
                        }
                        // U+2800 is blank braille and every pattern is an
                        // offset from it. A truly blank cell would punch a hole
                        // in the graph, so nothing-lit falls back to the track.
                        if bits == 0 {
                            (DIM, '⡀')
                        } else {
                            (lit, char::from_u32(0x2800 + bits as u32).unwrap_or('⣿'))
                        }
                    }
                    Glyphs::Ascii => {
                        let base = from_bottom * per_row;
                        match level_at(cell) {
                            Some(level) if level > base + 1 => (lit, '#'),
                            // Half a cell. Worth the extra character: without
                            // it a six-row ASCII graph has six distinguishable
                            // heights and every shape looks like a staircase.
                            Some(level) if level > base => (lit, '-'),
                            _ => (DIM, '.'),
                        }
                    }
                };
                runs.push(color, ch);
            }
            Line::from(runs.finish())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The visible characters of a span list, ignoring colour.
    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn graph_text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn a_bar_is_always_exactly_its_width() {
        for frac in [-1.0, 0.0, 0.37, 0.999, 1.0, 4.0, f64::NAN] {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                assert_eq!(
                    text(&bar(frac, 16, glyphs)).chars().count(),
                    16,
                    "frac {frac} in {glyphs:?}"
                );
            }
        }
        assert!(bar(0.5, 0, Glyphs::Unicode).is_empty());
    }

    #[test]
    fn an_empty_bar_is_track_and_a_full_one_is_fill() {
        assert_eq!(text(&bar(0.0, 8, Glyphs::Ascii)), "........");
        assert_eq!(text(&bar(1.0, 8, Glyphs::Ascii)), "########");
        assert_eq!(text(&bar(0.0, 4, Glyphs::Unicode)), "░░░░");
        assert_eq!(text(&bar(1.0, 4, Glyphs::Unicode)), "████");
    }

    #[test]
    fn a_partial_cell_lands_between_empty_and_full() {
        // Three and a half cells of eight.
        assert_eq!(text(&bar(3.5 / 8.0, 8, Glyphs::Unicode)), "███▌░░░░");
        // ASCII has no half-cell, so it rounds at the middle rather than
        // dropping the remainder.
        assert_eq!(text(&bar(3.5 / 8.0, 8, Glyphs::Ascii)), "####....");
        assert_eq!(text(&bar(3.2 / 8.0, 8, Glyphs::Ascii)), "###.....");
    }

    #[test]
    fn a_small_but_non_zero_value_still_shows_something() {
        // 0.4% of a 16-cell bar is under a sixteenth of a cell. Rounding it
        // away makes a busy-but-quiet box look like a stopped sampler.
        let drawn = text(&bar(0.004, 16, Glyphs::Unicode));
        assert!(drawn.starts_with('▏'), "got {drawn}");
        assert_eq!(
            text(&bar(0.0, 16, Glyphs::Unicode)).chars().next(),
            Some('░')
        );
    }

    #[test]
    fn the_ramp_runs_green_to_red_and_clamps() {
        assert_eq!(ramp_at(Depth::Ansi16, 0.0), Color::Green);
        assert_eq!(ramp_at(Depth::Ansi16, 1.0), Color::Red);
        assert_eq!(ramp_at(Depth::Ansi16, -5.0), Color::Green);
        assert_eq!(ramp_at(Depth::Ansi16, 5.0), Color::Red);
        assert_eq!(ramp_at(Depth::Ansi16, f64::NAN), Color::Green);
        // The ends of the cube's edge walk: pure green, then pure red.
        assert_eq!(ramp_at(Depth::Cube256, 0.0), Color::Indexed(16 + 6 * 5));
        assert_eq!(ramp_at(Depth::Cube256, 1.0), Color::Indexed(16 + 36 * 5));
        assert_eq!(ramp_at(Depth::Cube256, 2.0), Color::Indexed(16 + 36 * 5));
    }

    #[test]
    fn the_pi_console_is_detected_as_sixteen_colour() {
        assert_eq!(detect_depth(Some("linux"), None), Depth::Ansi16);
        assert_eq!(detect_depth(Some("xterm"), None), Depth::Ansi16);
        assert_eq!(detect_depth(None, None), Depth::Ansi16);
        assert_eq!(detect_depth(Some("xterm-256color"), None), Depth::Cube256);
        assert_eq!(
            detect_depth(Some("linux"), Some("truecolor")),
            Depth::Cube256
        );
        // An exported-but-empty COLORTERM is not a claim of anything.
        assert_eq!(detect_depth(Some("linux"), Some("")), Depth::Ansi16);
    }

    #[test]
    fn unknown_glyph_names_fall_back_rather_than_failing() {
        assert_eq!(Glyphs::parse("ascii"), Glyphs::Ascii);
        assert_eq!(Glyphs::parse("  ASCII "), Glyphs::Ascii);
        assert_eq!(Glyphs::parse("unicode"), Glyphs::Unicode);
        assert_eq!(Glyphs::parse("braille"), Glyphs::Unicode);
        assert_eq!(Glyphs::parse(""), Glyphs::Unicode);
        // The ASCII frame must not smuggle a box-drawing glyph back in.
        let set = Glyphs::Ascii.border_set();
        for piece in [
            set.top_left,
            set.top_right,
            set.bottom_left,
            set.bottom_right,
            set.vertical_left,
            set.vertical_right,
            set.horizontal_top,
            set.horizontal_bottom,
        ] {
            assert!(piece.is_ascii(), "{piece:?} is not ASCII");
        }
    }

    #[test]
    fn a_graph_is_exactly_its_width_and_height() {
        let samples: Vec<f64> = (0..500).map(|i| (i % 100) as f64 / 100.0).collect();
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let lines = area_graph(&samples, 20, 4, glyphs);
            assert_eq!(lines.len(), 4);
            for line in graph_text(&lines) {
                assert_eq!(line.chars().count(), 20, "{glyphs:?}: {line}");
            }
        }
        assert!(area_graph(&samples, 0, 4, Glyphs::Unicode).is_empty());
        assert!(area_graph(&samples, 20, 0, Glyphs::Unicode).is_empty());
    }

    #[test]
    fn a_graph_with_no_samples_yet_draws_bare_track() {
        assert_eq!(
            graph_text(&area_graph(&[], 6, 2, Glyphs::Ascii)),
            ["......"; 2]
        );
        assert_eq!(graph_text(&area_graph(&[], 3, 1, Glyphs::Unicode)), ["⡀⡀⡀"]);
    }

    #[test]
    fn history_is_right_aligned_so_a_fresh_start_grows_inward() {
        // Two samples in a six-column graph: four columns of track, then the
        // data. Stretching two points across the pane would imply a history
        // that has not been collected yet.
        assert_eq!(
            graph_text(&area_graph(&[1.0, 1.0], 6, 1, Glyphs::Ascii)),
            ["....##"]
        );
    }

    #[test]
    fn a_full_column_fills_every_row_and_an_idle_one_fills_none() {
        assert_eq!(
            graph_text(&area_graph(&[1.0; 8], 4, 3, Glyphs::Ascii)),
            ["####"; 3]
        );
        assert_eq!(
            graph_text(&area_graph(&[0.0; 8], 4, 3, Glyphs::Ascii)),
            ["...."; 3]
        );
        // Braille: a full column is the all-dots cell.
        assert_eq!(
            graph_text(&area_graph(&[1.0; 8], 2, 2, Glyphs::Unicode)),
            ["⣿⣿"; 2]
        );
    }

    #[test]
    fn ascii_resolves_half_a_row() {
        // Three levels of a four-level (two-row) graph: one full row and a
        // half above it.
        assert_eq!(
            graph_text(&area_graph(&[0.75; 4], 3, 2, Glyphs::Ascii)),
            ["---", "###"]
        );
    }

    #[test]
    fn the_graph_fills_from_the_bottom_up() {
        // Half height in a four-row graph: the bottom two rows carry it.
        let lines = graph_text(&area_graph(&[0.5; 8], 4, 4, Glyphs::Ascii));
        assert_eq!(lines[0], "....", "the top row must be empty at 50%");
        assert_eq!(lines[1], "....");
        assert_eq!(lines[2], "####");
        assert_eq!(lines[3], "####");
    }

    #[test]
    fn a_barely_busy_sample_lights_the_bottom_dot_row_only() {
        // 1% over a 24-level braille graph rounds to zero levels, and an
        // apparently-idle graph on a box that is not idle is the one lie a
        // history graph must not tell.
        let lines = graph_text(&area_graph(&[0.01; 16], 8, 6, Glyphs::Unicode));
        assert_ne!(lines[5], "⡀".repeat(8), "the bottom row must show it");
        assert_eq!(lines[4], "⡀".repeat(8), "nothing above it may light");
    }
}
