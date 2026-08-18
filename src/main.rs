//! pi-dash — a one-window terminal dashboard for the Raspberry Pi running
//! ClassG.
//!
//! It shows the three things a process monitor cannot tell you, plus a system
//! summary so you do not need one:
//!
//!   system   CPU, memory and the busiest processes, straight from /proc
//!   health   temperature, core voltage, ARM clock, and the throttle bits
//!   radios   per-interface throughput, monitor-mode state, USB radio presence
//!   classg   GET /api/v1/health and /tracks, degraded rather than fatal
//!
//! It is a rewrite of `classg/scripts/pi-dash.sh`, which orchestrated tmux
//! around a btop pane and three Bash readers. Everything is rendered by this
//! one process now: no tmux, no btop, no python3 for the API pane. What is
//! kept is the reason the Bash version read /proc, /sys and vcgencmd directly
//! rather than shelling out to iotop/nethogs/bandwhich — those all need root
//! or CAP_NET_ADMIN to say anything useful, none of them ship on a stock Pi
//! OS, and the numbers they would add are not the numbers that break this box.
//!
//! Set CLASSG_API if the API is not on the default port.

mod app;
mod config;
mod format;
mod panes;
mod run;
mod snapshot;
mod ui;

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::App;
use config::{load_config, CliOverrides};
use run::run_app;

#[derive(Parser, Debug)]
#[command(name = "pi-dash", version, about = "Terminal dashboard for the ClassG Pi", long_about = None)]
struct Cli {
    /// Config file to use instead of searching the default locations
    /// (binary directory, current directory, ~/.config/pi-dash).
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// ClassG API base URL. Overrides CLASSG_API.
    #[arg(long, value_name = "URL")]
    api: Option<String>,

    /// Seconds between local samples. Overrides CLASSG_DASH_INTERVAL.
    #[arg(short, long, value_name = "SECONDS")]
    interval: Option<f64>,

    /// Print one plain-text snapshot and exit. No terminal required, so this
    /// is what to run over SSH, from cron, or from a health check.
    #[arg(long)]
    once: bool,

    /// Print the resolved configuration and where each part came from.
    #[arg(long)]
    print_config: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config(
        cli.config,
        &CliOverrides {
            api: cli.api,
            interval: cli.interval,
        },
    )
    .context("failed to load configuration")?;

    if cli.print_config {
        println!(
            "config    {}",
            config
                .source
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "built-in defaults".to_string())
        );
        println!("api       {}", config.api);
        println!("interval  {:.2}s", config.interval.as_secs_f64());
        println!("api poll  {:.2}s", config.api_interval.as_secs_f64());
        println!("theme     {}", config.theme);
        println!("glyphs    {}", config.glyphs);
        println!("usb ids   {}", config.usb_vendor_ids.join(", "));
        println!("ignore    {}", config.ignore_interfaces.join(", "));
        return Ok(());
    }

    if cli.once {
        let mut stdout = io::stdout().lock();
        snapshot::print_once(&config, &mut stdout)?;
        stdout.flush()?;
        return Ok(());
    }

    let app = App::new(config);
    let mut terminal = enter_terminal()?;
    let result = run_app(&mut terminal, app);
    leave_terminal(&mut terminal)?;

    // Report after the terminal is back, so the message is not swallowed by
    // the alternate screen being torn down under it.
    result
}

fn enter_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    // Mouse capture is deliberately *not* enabled. This dashboard is mostly
    // read over SSH, and capturing the mouse takes click-drag text selection
    // away from the terminal — which is how you copy a throttle code or a
    // sensor's failure reason out of it to paste somewhere else.
    install_panic_hook();
    Terminal::new(CrosstermBackend::new(stdout)).context("could not start the terminal backend")
}

fn leave_terminal<W: Write>(terminal: &mut Terminal<CrosstermBackend<W>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Restores the terminal before a panic message is printed.
///
/// Nothing here is meant to panic, but a panic while raw mode is on and the
/// alternate screen is up leaves an SSH session with no echo and no cursor,
/// and the person on the other end has to guess that `reset` will fix it.
/// Insurance, not a substitute for handling errors.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        previous(info);
    }));
}
