//! Configuration, resolved from three sources.
//!
//! Precedence is env > file > built-in default, matching the Bash dashboard
//! this replaces (which only had env and defaults). The file is optional in a
//! way `bbs-launcher`'s is not: this runs on a Pi that may have nothing but
//! the binary on it, and a dashboard that refuses to start because it cannot
//! find a TOML file it does not need is a dashboard you stop using.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_API: &str = "http://127.0.0.1:8081";
pub const DEFAULT_INTERVAL_SECS: f64 = 2.0;
pub const DEFAULT_API_INTERVAL_SECS: f64 = 3.0;

/// Vendor IDs worth calling a radio: MediaTek, Realtek (which is also where
/// the RTL-SDR lands), Ralink, TP-Link, Atheros, Great Scott.
///
/// Currently on the box this was written for: `0e8d:7961` (ALFA AWUS036AXML)
/// and `0bda:2838` (RTL-SDR Blog V4). Note that `0bda` is Realtek's whole
/// catalogue, so a Realtek card reader would match too — false positives here
/// are harmless, a missing adapter is not.
pub const DEFAULT_USB_VENDOR_IDS: [&str; 6] = ["0e8d", "0bda", "148f", "2357", "0cf3", "1d50"];

/// Interfaces the radios pane never shows. Container and virtual-bridge
/// plumbing is not a radio, and on a Pi running the ClassG stack in Docker
/// there is a lot of it.
pub const DEFAULT_IGNORE_INTERFACES: [&str; 6] =
    ["lo", "docker*", "br-*", "veth*", "virbr*", "tailscale*"];

/// The readers are built around a ~46-column body. Wider than about 60 and the
/// right-hand column is mostly padding, which on a large monitor means a third
/// of the screen showing nothing — so the split is a clamp, not a percentage,
/// and the system pane gets every column the readers cannot use.
pub const READER_MIN_COLS: u16 = 48;
pub const READER_MAX_COLS: u16 = 60;

/// Below this the two-column layout leaves both halves unreadable. The Bash
/// version put btop in its own tmux window at this point; here the dashboard
/// switches to showing one pane at a time.
pub const NARROW_COLS: u16 = 100;

/// The file form. Every field is optional so a partial file is valid — you
/// should be able to drop in three lines to move the API port without
/// restating defaults you do not care about.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default)]
    pub dash: DashSection,
    #[serde(default)]
    pub radios: RadiosSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashSection {
    /// Base URL of the ClassG API, without a trailing `/api/v1`.
    pub api: Option<String>,
    /// Seconds between local (`/proc`, `/sys`, `vcgencmd`) samples.
    pub interval_secs: Option<f64>,
    /// Seconds between ClassG API polls. Separate from `interval_secs`
    /// because it is the only sample that leaves the process.
    pub api_interval_secs: Option<f64>,
    /// Accent colour: any standard terminal colour name. Default `cyan`.
    pub theme: Option<String>,
    /// Rows of process table in the system pane. Default: fill the pane.
    pub processes: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadiosSection {
    /// Four-hex-digit USB vendor IDs treated as radios.
    pub usb_vendor_ids: Option<Vec<String>>,
    /// Interface names to hide. A trailing `*` globs a prefix.
    pub ignore_interfaces: Option<Vec<String>>,
}

/// The resolved configuration the rest of the program reads.
#[derive(Debug, Clone)]
pub struct Config {
    pub api: String,
    pub interval: std::time::Duration,
    pub api_interval: std::time::Duration,
    pub theme: String,
    pub processes: Option<usize>,
    pub usb_vendor_ids: Vec<String>,
    pub ignore_interfaces: Vec<String>,
    /// Where the settings came from, for `--print-config` and the help pane.
    pub source: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            api: DEFAULT_API.to_string(),
            interval: secs_to_duration(DEFAULT_INTERVAL_SECS),
            api_interval: secs_to_duration(DEFAULT_API_INTERVAL_SECS),
            theme: "cyan".to_string(),
            processes: None,
            usb_vendor_ids: DEFAULT_USB_VENDOR_IDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ignore_interfaces: DEFAULT_IGNORE_INTERFACES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            source: None,
        }
    }
}

/// Clamps a configured interval into something sane. A zero or negative
/// interval would spin the sample loop at full CPU on a box whose whole
/// problem is that it browns out under load.
fn secs_to_duration(secs: f64) -> std::time::Duration {
    let clamped = if secs.is_finite() {
        secs.clamp(0.25, 3600.0)
    } else {
        DEFAULT_INTERVAL_SECS
    };
    std::time::Duration::from_secs_f64(clamped)
}

/// Overrides supplied on the command line, applied last of all.
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub api: Option<String>,
    pub interval: Option<f64>,
}

pub fn load_config(override_path: Option<PathBuf>, cli: &CliOverrides) -> Result<Config> {
    let explicit = override_path.is_some();
    let path = match override_path {
        Some(p) => Some(p),
        None => find_config(),
    };

    let mut config = Config::default();

    if let Some(path) = path {
        // An explicitly named file that does not parse is an error worth
        // stopping for; one merely found on the search path is not, because
        // the dashboard is more useful up with defaults than not up at all.
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let parsed: ConfigFile = toml::from_str(&text)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                apply_file(&mut config, parsed);
                config.source = Some(path);
            }
            Err(err) if explicit => {
                return Err(err).with_context(|| format!("failed to read {}", path.display()));
            }
            Err(_) => {}
        }
    }

    apply_env(&mut config);

    if let Some(api) = &cli.api {
        config.api = api.clone();
    }
    if let Some(secs) = cli.interval {
        config.interval = secs_to_duration(secs);
    }

    config.api = config.api.trim_end_matches('/').to_string();
    Ok(config)
}

fn apply_file(config: &mut Config, file: ConfigFile) {
    if let Some(api) = file.dash.api {
        config.api = api;
    }
    if let Some(secs) = file.dash.interval_secs {
        config.interval = secs_to_duration(secs);
    }
    if let Some(secs) = file.dash.api_interval_secs {
        config.api_interval = secs_to_duration(secs);
    }
    if let Some(theme) = file.dash.theme {
        config.theme = theme;
    }
    if let Some(n) = file.dash.processes {
        config.processes = Some(n);
    }
    if let Some(ids) = file.radios.usb_vendor_ids {
        config.usb_vendor_ids = ids;
    }
    if let Some(ifaces) = file.radios.ignore_interfaces {
        config.ignore_interfaces = ifaces;
    }
}

/// The two environment variables the Bash dashboard honoured, unchanged, so a
/// `CLASSG_API=... pidash` habit keeps working.
fn apply_env(config: &mut Config) {
    if let Ok(api) = std::env::var("CLASSG_API") {
        if !api.trim().is_empty() {
            config.api = api.trim().to_string();
        }
    }
    if let Ok(raw) = std::env::var("CLASSG_DASH_INTERVAL") {
        // The Bash version fed this straight into `$(( ))`, so a perfectly
        // reasonable `0.5` made every tick emit an arithmetic syntax error and
        // silently zeroed the rate columns. Parse as a float and clamp.
        if let Ok(secs) = raw.trim().parse::<f64>() {
            config.interval = secs_to_duration(secs);
        }
    }
}

/// Search order matches `bbs-launcher`: next to the binary, in the working
/// directory, then under `~/.config`. Returns `None` when none exist, which
/// is a normal, silent outcome.
pub fn find_config() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("pi-dash.toml"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("pi-dash.toml"));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".config").join("pi-dash").join("pi-dash.toml"));
    }

    candidates.into_iter().find(|p| p.exists())
}

/// Matches an interface or device name against one of the ignore patterns.
/// Only a trailing `*` is special — full globbing would be a dependency and a
/// surprise, and every real pattern here is a prefix.
pub fn name_matches(name: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

pub fn is_ignored(name: &str, patterns: &[impl AsRef<str>]) -> bool {
    patterns.iter().any(|p| name_matches(name, p.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_stand_alone_without_a_file() {
        let config = load_config(None, &CliOverrides::default());
        // Whatever is on this machine's search path, loading must succeed.
        assert!(config.is_ok());
    }

    #[test]
    fn a_partial_file_only_overrides_what_it_mentions() {
        let mut config = Config::default();
        let file: ConfigFile = toml::from_str("[dash]\napi = \"http://pi.local:9000\"\n").unwrap();
        apply_file(&mut config, file);
        assert_eq!(config.api, "http://pi.local:9000");
        assert_eq!(config.interval, secs_to_duration(DEFAULT_INTERVAL_SECS));
        assert_eq!(config.usb_vendor_ids.len(), DEFAULT_USB_VENDOR_IDS.len());
    }

    #[test]
    fn intervals_are_clamped_away_from_zero() {
        assert!(secs_to_duration(0.0) >= std::time::Duration::from_millis(250));
        assert!(secs_to_duration(-4.0) >= std::time::Duration::from_millis(250));
        assert!(secs_to_duration(f64::NAN) > std::time::Duration::ZERO);
        assert_eq!(secs_to_duration(0.5), std::time::Duration::from_millis(500));
    }

    #[test]
    fn ignore_patterns_glob_only_a_trailing_star() {
        let patterns = DEFAULT_IGNORE_INTERFACES;
        assert!(is_ignored("lo", &patterns));
        assert!(is_ignored("docker0", &patterns));
        assert!(is_ignored("br-1a2b3c", &patterns));
        assert!(is_ignored("veth9f2", &patterns));
        assert!(!is_ignored("wlan1", &patterns));
        assert!(!is_ignored("eth0", &patterns));
        // "lo" is exact, so a real interface that starts with it stays.
        assert!(!is_ignored("lorawan0", &patterns));
    }

    #[test]
    fn unknown_keys_in_the_file_are_an_error_not_a_silent_typo() {
        let parsed: Result<ConfigFile, _> = toml::from_str("[dash]\nintervall_secs = 2\n");
        assert!(parsed.is_err());
    }
}
