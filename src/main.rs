//! pi-dash — a one-window terminal dashboard for the Raspberry Pi running
//! ClassG.
//!
//! It shows the three things a process monitor cannot tell you, plus a system
//! summary so you do not need one:
//!
//!   system   CPU, memory and the busiest processes, straight from /proc
//!   health   temperature, core voltage, ARM clock, and the throttle bits
//!   radios   per-interface throughput, monitor-mode state, USB radio presence
//!   classg   whether the detector is recording and what it can hear,
//!            from /health, /monitoring, /system, /tracks and /detections —
//!            degraded rather than fatal
//!
//! It is a rewrite of `classg/scripts/pi-dash.sh`, which orchestrated tmux
//! around a btop pane and three Bash readers. Everything is rendered by this
//! one process now: no tmux, no btop, no python3 for the API pane. What is
//! kept is the reason the Bash version read /proc, /sys and vcgencmd directly
//! rather than shelling out to iotop/nethogs/bandwhich — those all need root
//! or CAP_NET_ADMIN to say anything useful, none of them ship on a stock Pi
//! OS, and the numbers they would add are not the numbers that break this box.
//!
//! Set CLASSG_API if the API is not on the default port, and CLASSG_SESSION
//! if that API has authentication switched on — only /health and /auth/me are
//! public, and without a session the pane can show sensor state and nothing
//! else. It says so rather than drawing an empty track list.

// Tests assert by panicking: `expect` on a value a fixture just constructed is
// how a failed assertion reports itself, and `unwrap_used`/`expect_used` are
// denied crate-wide precisely so that never happens in the code that runs on
// the Pi. The expectation is scoped to `cfg(test)`, so the ordinary build of
// this binary — the one that ships — is still checked strictly.
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "a test asserts by panicking; the shipped binary is still checked"
    )
)]

mod app;
mod check;
mod config;
mod format;
mod localtoken;
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
use config::{load_config, CliOverrides, Config};
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
    /// is what to run over SSH when you want to read the whole picture.
    /// Always exits 0: a snapshot's job is to render, and it rendered.
    #[arg(long)]
    once: bool,

    /// Print one verdict line and exit 0 (ok), 1 (degraded) or 2 (down).
    ///
    /// The monitoring half of --once, for cron and CI. It judges what this
    /// dashboard can see and not only what /health says, because a healthy API
    /// on a Pi that is browning out or nearly out of disk is a detector with a
    /// date on it. The line is always printed; redirect stdout if you only
    /// want mail when something is wrong.
    #[arg(long)]
    check: bool,

    /// Print the resolved configuration and where each part came from.
    #[arg(long)]
    print_config: bool,
}

fn main() -> Result<std::process::ExitCode> {
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
        let mut stdout = io::stdout().lock();
        print_config(&config, &mut stdout)?;
        stdout.flush()?;
        return Ok(std::process::ExitCode::SUCCESS);
    }

    if cli.check {
        let mut stdout = io::stdout().lock();
        let code = check::run(&config, &mut stdout)?;
        stdout.flush()?;
        return Ok(code);
    }

    if cli.once {
        let mut stdout = io::stdout().lock();
        snapshot::print_once(&config, &mut stdout)?;
        stdout.flush()?;
        return Ok(std::process::ExitCode::SUCCESS);
    }

    let app = App::new(config);
    let mut terminal = enter_terminal()?;
    let result = run_app(&mut terminal, app);
    leave_terminal(&mut terminal)?;

    // Report after the terminal is back, so the message is not swallowed by
    // the alternate screen being torn down under it.
    result.map(|()| std::process::ExitCode::SUCCESS)
}

/// The resolved settings, and which tier of the precedence chain set each one.
///
/// The provenance column is the point. "api http://127.0.0.1:8081" does not
/// help somebody whose dashboard is talking to the wrong box; "(environment)"
/// next to it points straight at the `CLASSG_API` still exported in their
/// shell, which no amount of reading the config file would have revealed.
fn print_config(config: &Config, out: &mut impl Write) -> Result<()> {
    writeln!(
        out,
        "{:<12}{}",
        "config",
        config
            .source
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none found".to_string())
    )?;

    // The key the origin map is keyed by is not always the label worth
    // printing: `api_interval` is a field name, `api poll` is what it does.
    let mut row = |label: &str, key: &str, value: String| -> Result<()> {
        writeln!(
            out,
            "{label:<12}{value:<40}({})",
            config.origins.of(key).label()
        )?;
        Ok(())
    };
    row("api", "api", config.api.clone())?;
    row(
        "interval",
        "interval",
        format!("{:.2}s", config.interval.as_secs_f64()),
    )?;
    row(
        "api poll",
        "api_interval",
        format!("{:.2}s", config.api_interval.as_secs_f64()),
    )?;
    row("theme", "theme", config.theme.clone())?;
    row("glyphs", "glyphs", config.glyphs.clone())?;
    row(
        "processes",
        "processes",
        config
            .processes
            .map(|n| n.to_string())
            .unwrap_or_else(|| "fill the pane".to_string()),
    )?;
    // Never the token itself. This is the one command somebody pastes into an
    // issue when the dashboard will not talk to their API, and a session
    // cookie in that paste is a live credential for the whole unit.
    // Labelled for what it is rather than for the config key behind it: this
    // row reports a local-agent token as readily as a session, and calling
    // that "session" would be the one line here that misleads.
    row(
        "credential",
        "session",
        match (&config.session, &config.local_token) {
            (Some(_), _) => "set (session cookie)".to_string(),
            (None, Some(_)) => "set (local agent token on this unit)".to_string(),
            (None, None) => "not set".to_string(),
        },
    )?;
    row(
        "usb ids",
        "usb_vendor_ids",
        config.usb_vendor_ids.join(", "),
    )?;
    row(
        "ignore",
        "ignore_interfaces",
        config.ignore_interfaces.join(", "),
    )?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use config::Origin;

    fn rendered(config: &Config) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        print_config(config, &mut buffer).expect("print_config must not fail");
        String::from_utf8(buffer).expect("utf-8")
    }

    #[test]
    fn print_config_never_prints_the_credential_itself() {
        // This is the one command somebody pastes into an issue when the
        // dashboard will not talk to their API. A session cookie in that paste
        // is a live credential for the whole unit, and a local-agent token is
        // one for as long as the API stays up.
        let config = Config {
            session: Some("s3cr3t-session-cookie".to_string()),
            local_token: Some("s3cr3t-machine-token".to_string()),
            ..Config::default()
        };
        let text = rendered(&config);

        assert!(
            !text.contains("s3cr3t"),
            "a credential reached stdout:\n{text}"
        );
        assert!(text.contains("credential"), "{text}");
        assert!(text.contains("set (session cookie)"), "{text}");
    }

    #[test]
    fn print_config_says_which_credential_is_in_play() {
        // A session outranks a local token, so the row has to name the one
        // actually being sent rather than the one that happens to exist.
        let local_only = Config {
            local_token: Some("machine".to_string()),
            ..Config::default()
        };
        assert!(rendered(&local_only).contains("local agent token"));

        let both = Config {
            session: Some("human".to_string()),
            local_token: Some("machine".to_string()),
            ..Config::default()
        };
        assert!(rendered(&both).contains("session cookie"));
        assert!(!rendered(&both).contains("local agent token"));

        assert!(rendered(&Config::default()).contains("not set"));
    }

    #[test]
    fn print_config_names_the_tier_that_set_each_value() {
        // The entire point of the column: a dashboard pointed at the wrong box
        // is nearly always a CLASSG_API still exported in the shell, and no
        // amount of reading the config file reveals that.
        let mut config = Config {
            api: "http://pi.local:9000".to_string(),
            ..Config::default()
        };
        config.origins.set_for_test("api", Origin::Env);
        let text = rendered(&config);

        let api_row = text
            .lines()
            .find(|line| line.starts_with("api "))
            .expect("an api row");
        assert!(api_row.contains("http://pi.local:9000"), "{api_row}");
        assert!(api_row.ends_with("(environment)"), "{api_row}");

        // Untouched values still say where they came from.
        let theme_row = text
            .lines()
            .find(|line| line.starts_with("theme"))
            .expect("a theme row");
        assert!(theme_row.ends_with("(built-in default)"), "{theme_row}");
    }

    #[test]
    fn print_config_reports_no_file_rather_than_an_empty_path() {
        let text = rendered(&Config::default());
        assert!(text.starts_with("config      none found"), "{text}");
    }
}
