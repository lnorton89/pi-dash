//! Configuration, resolved from four sources.
//!
//! Precedence is command line > env > file > built-in default, extending the
//! Bash dashboard this replaces (which only had env and defaults). A fifth
//! origin, [`Origin::LocalAgent`], is not a tier: it marks a credential found
//! on this unit's own disk with nobody having configured anything at all.
//!
//! Three tiers were documented here, in the README, and in the sample config,
//! for as long as there were four. `--print-config` has been reporting
//! "(command line)" as a source the docs never mentioned. The file is optional in a
//! way `bbs-launcher`'s is not: this runs on a Pi that may have nothing but
//! the binary on it, and a dashboard that refuses to start because it cannot
//! find a TOML file it does not need is a dashboard you stop using.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

pub(crate) const DEFAULT_API: &str = "http://127.0.0.1:8081";
pub(crate) const DEFAULT_INTERVAL_SECS: f64 = 2.0;
pub(crate) const DEFAULT_API_INTERVAL_SECS: f64 = 3.0;

/// Vendor IDs worth calling a radio: MediaTek, Realtek (which is also where
/// the RTL-SDR lands), Ralink, TP-Link, Atheros, Great Scott.
///
/// Currently on the box this was written for: `0e8d:7961` (ALFA AWUS036AXML)
/// and `0bda:2838` (RTL-SDR Blog V4). Note that `0bda` is Realtek's whole
/// catalogue, so a Realtek card reader would match too — false positives here
/// are harmless, a missing adapter is not.
pub(crate) const DEFAULT_USB_VENDOR_IDS: [&str; 6] =
    ["0e8d", "0bda", "148f", "2357", "0cf3", "1d50"];

/// Interfaces the radios pane never shows. Container and virtual-bridge
/// plumbing is not a radio, and on a Pi running the ClassG stack in Docker
/// there is a lot of it.
pub(crate) const DEFAULT_IGNORE_INTERFACES: [&str; 6] =
    ["lo", "docker*", "br-*", "veth*", "virbr*", "tailscale*"];

/// The readers are built around a ~46-column body. Wider than about 60 and the
/// right-hand column is mostly padding, which on a large monitor means a third
/// of the screen showing nothing — so the split is a clamp, not a percentage,
/// and the system pane gets every column the readers cannot use.
pub(crate) const READER_MIN_COLS: u16 = 48;
pub(crate) const READER_MAX_COLS: u16 = 60;

/// Below this the two-column layout leaves both halves unreadable. The Bash
/// version put btop in its own tmux window at this point; here the dashboard
/// switches to showing one pane at a time.
pub(crate) const NARROW_COLS: u16 = 100;

/// The file form. Every field is optional so a partial file is valid — you
/// should be able to drop in three lines to move the API port without
/// restating defaults you do not care about.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigFile {
    #[serde(default)]
    pub(crate) dash: DashSection,
    #[serde(default)]
    pub(crate) radios: RadiosSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DashSection {
    /// Base URL of the ClassG API, without a trailing `/api/v1`.
    pub(crate) api: Option<String>,
    /// Seconds between local (`/proc`, `/sys`, `vcgencmd`) samples.
    pub(crate) interval_secs: Option<f64>,
    /// Seconds between ClassG API polls. Separate from `interval_secs`
    /// because it is the only sample that leaves the process.
    pub(crate) api_interval_secs: Option<f64>,
    /// Accent colour: any standard terminal colour name. Default `cyan`.
    pub(crate) theme: Option<String>,
    /// `unicode` (default) or `ascii`. See [`crate::ui::gauge::Glyphs`].
    pub(crate) glyphs: Option<String>,
    /// Rows of process table in the system pane. Default: fill the pane.
    pub(crate) processes: Option<usize>,
    /// Value of the ClassG `classg_session` cookie, for a unit that has
    /// authentication switched on. See [`Config::session`].
    pub(crate) session: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RadiosSection {
    /// Four-hex-digit USB vendor IDs treated as radios.
    pub(crate) usb_vendor_ids: Option<Vec<String>>,
    /// Interface names to hide. A trailing `*` globs a prefix.
    pub(crate) ignore_interfaces: Option<Vec<String>>,
}

/// Which tier of the precedence chain a resolved value actually came from.
///
/// `--print-config` has always promised to print "where each part came from"
/// and has only ever printed the file it found. That is the least useful half:
/// the question people have is why the dashboard is talking to the wrong port,
/// and the answer is nearly always a `CLASSG_API` still exported in the shell
/// they ran it from, which the config file cannot tell them about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Default,
    File,
    Env,
    Cli,
    /// Read off this unit's own disk, with nobody having configured anything.
    LocalAgent,
}

impl Origin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Origin::Default => "built-in default",
            Origin::File => "config file",
            Origin::Env => "environment",
            Origin::Cli => "command line",
            Origin::LocalAgent => "local agent token on this unit",
        }
    }
}

/// Where each key was last written from. Anything never written is a default.
#[derive(Debug, Clone, Default)]
pub(crate) struct Origins(std::collections::BTreeMap<&'static str, Origin>);

impl Origins {
    fn set(&mut self, key: &'static str, origin: Origin) {
        self.0.insert(key, origin);
    }

    pub(crate) fn of(&self, key: &str) -> Origin {
        self.0.get(key).copied().unwrap_or(Origin::Default)
    }

    /// Stamps an origin without going through a load. `set` stays private so
    /// that provenance can only be claimed by the code that actually applied
    /// the value; this exists for tests that need a resolved config without a
    /// file or an environment to resolve it from.
    #[cfg(test)]
    pub(crate) fn set_for_test(&mut self, key: &'static str, origin: Origin) {
        self.set(key, origin);
    }
}

/// The resolved configuration the rest of the program reads.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) api: String,
    pub(crate) interval: std::time::Duration,
    pub(crate) api_interval: std::time::Duration,
    pub(crate) theme: String,
    /// Character set for meters, graphs and borders. Parsed into
    /// [`crate::ui::gauge::Glyphs`] once, at startup.
    pub(crate) glyphs: String,
    pub(crate) processes: Option<usize>,
    /// Session cookie for the ClassG API, or `None` for a unit with
    /// authentication off — which is the common case, since the API is on
    /// loopback and the dashboard runs on the same box.
    ///
    /// Only `/health` and `/auth/me` are public. Without a session every other
    /// endpoint answers 401 and the pane can show sensor state and nothing
    /// else, so it reports that in as many words rather than drawing an empty
    /// track list that looks like a quiet sky.
    ///
    /// A token, not a password: pi-dash never logs in and never holds a
    /// credential that could make a new session. Copy the cookie from a
    /// browser that is already signed in, or mint one with `classgctl`.
    pub(crate) session: Option<String>,
    /// The API's local-agent token, found on this host.
    ///
    /// Distinct from `session` on purpose: a session names a person and can do
    /// whatever that person can, while this names the machine and is viewer
    /// only. Nobody configures it -- the API writes it into the state
    /// directory it already shares with the host agents, mode 0640, and being
    /// able to read that file is the credential.
    ///
    /// `session` wins when both exist. Someone who exported CLASSG_SESSION
    /// meant it, and the usual reason is pointing this build at a different
    /// unit over the network, where a local file describes the wrong box.
    pub(crate) local_token: Option<String>,
    pub(crate) usb_vendor_ids: Vec<String>,
    pub(crate) ignore_interfaces: Vec<String>,
    /// Where the settings came from, for `--print-config` and the help pane.
    pub(crate) source: Option<PathBuf>,
    /// Per-key provenance, for `--print-config`.
    pub(crate) origins: Origins,
}

impl Config {
    /// The credential to talk to the API with, resolved from the two sources
    /// in their documented precedence.
    ///
    /// Here rather than at each call site because there are three of them now
    /// — the dashboard, `--once` and `--check` — and a fourth that forgot to
    /// apply the precedence would be a build that silently authenticates as
    /// somebody else.
    pub(crate) fn credential(&self) -> Option<crate::panes::classg::Credential> {
        crate::panes::classg::Credential::pick(self.session.clone(), self.local_token.clone())
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            api: DEFAULT_API.to_string(),
            interval: secs_to_duration(DEFAULT_INTERVAL_SECS, DEFAULT_INTERVAL_SECS),
            api_interval: secs_to_duration(DEFAULT_API_INTERVAL_SECS, DEFAULT_API_INTERVAL_SECS),
            theme: "cyan".to_string(),
            glyphs: "unicode".to_string(),
            processes: None,
            session: None,
            local_token: None,
            usb_vendor_ids: DEFAULT_USB_VENDOR_IDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ignore_interfaces: DEFAULT_IGNORE_INTERFACES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            source: None,
            origins: Origins::default(),
        }
    }
}

/// Clamps a configured interval into something sane. A zero or negative
/// interval would spin the sample loop at full CPU on a box whose whole
/// problem is that it browns out under load.
///
/// The fallback is a parameter because there are two intervals with different
/// defaults. Hard-coding the local one meant `api_interval_secs = nan` in a
/// config file resolved to the LOCAL default of two seconds rather than the
/// API default of three -- a wrong value, quietly, from the one input that
/// should have been rejected.
fn secs_to_duration(secs: f64, fallback: f64) -> std::time::Duration {
    let clamped = if secs.is_finite() {
        secs.clamp(0.25, 3600.0)
    } else {
        fallback
    };
    std::time::Duration::from_secs_f64(clamped)
}

/// Overrides supplied on the command line, applied last of all.
#[derive(Debug, Default)]
pub(crate) struct CliOverrides {
    pub(crate) api: Option<String>,
    pub(crate) interval: Option<f64>,
}

pub(crate) fn load_config(override_path: Option<PathBuf>, cli: &CliOverrides) -> Result<Config> {
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
        config.origins.set("api", Origin::Cli);
    }
    if let Some(secs) = cli.interval {
        config.interval = secs_to_duration(secs, DEFAULT_INTERVAL_SECS);
        config.origins.set("interval", Origin::Cli);
    }

    config.api = config.api.trim_end_matches('/').to_string();
    // An empty token in a file is "not set", not "send an empty cookie".
    config.session = config
        .session
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());

    // Only when no session was configured. Looking for the file regardless
    // would mean a `CLASSG_SESSION=... pidash` aimed at another unit silently
    // preferring this one's credential the day someone reordered these two.
    if config.session.is_none() {
        config.local_token = crate::localtoken::discover();
        if config.local_token.is_some() {
            config.origins.set("session", Origin::LocalAgent);
        }
    }
    Ok(config)
}

fn apply_file(config: &mut Config, file: ConfigFile) {
    if let Some(api) = file.dash.api {
        config.api = api;
        config.origins.set("api", Origin::File);
    }
    if let Some(secs) = file.dash.interval_secs {
        config.interval = secs_to_duration(secs, DEFAULT_INTERVAL_SECS);
        config.origins.set("interval", Origin::File);
    }
    if let Some(secs) = file.dash.api_interval_secs {
        config.api_interval = secs_to_duration(secs, DEFAULT_API_INTERVAL_SECS);
        config.origins.set("api_interval", Origin::File);
    }
    if let Some(theme) = file.dash.theme {
        config.theme = theme;
        config.origins.set("theme", Origin::File);
    }
    if let Some(glyphs) = file.dash.glyphs {
        config.glyphs = glyphs;
        config.origins.set("glyphs", Origin::File);
    }
    if let Some(n) = file.dash.processes {
        config.processes = Some(n);
        config.origins.set("processes", Origin::File);
    }
    if let Some(session) = file.dash.session {
        config.session = Some(session);
        config.origins.set("session", Origin::File);
    }
    if let Some(ids) = file.radios.usb_vendor_ids {
        config.usb_vendor_ids = ids;
        config.origins.set("usb_vendor_ids", Origin::File);
    }
    if let Some(ifaces) = file.radios.ignore_interfaces {
        config.ignore_interfaces = ifaces;
        config.origins.set("ignore_interfaces", Origin::File);
    }
}

/// The two environment variables the Bash dashboard honoured, unchanged, so a
/// `CLASSG_API=... pidash` habit keeps working, plus `CLASSG_SESSION`.
fn apply_env(config: &mut Config) {
    if let Ok(api) = std::env::var("CLASSG_API") {
        if !api.trim().is_empty() {
            config.api = api.trim().to_string();
            config.origins.set("api", Origin::Env);
        }
    }
    if let Ok(raw) = std::env::var("CLASSG_DASH_INTERVAL") {
        // The Bash version fed this straight into `$(( ))`, so a perfectly
        // reasonable `0.5` made every tick emit an arithmetic syntax error and
        // silently zeroed the rate columns. Parse as a float and clamp.
        if let Ok(secs) = raw.trim().parse::<f64>() {
            config.interval = secs_to_duration(secs, DEFAULT_INTERVAL_SECS);
            config.origins.set("interval", Origin::Env);
        }
    }
    // Preferred over the file for the obvious reason: a session token is a
    // credential, and putting one in a TOML that sits next to the binary is
    // worse than exporting it for the length of a shell.
    if let Ok(session) = std::env::var("CLASSG_SESSION") {
        if !session.trim().is_empty() {
            config.session = Some(session.trim().to_string());
            config.origins.set("session", Origin::Env);
        }
    }
}

/// Search order matches `bbs-launcher`: next to the binary, in the working
/// directory, then under `~/.config`. Returns `None` when none exist, which
/// is a normal, silent outcome.
pub(crate) fn find_config() -> Option<PathBuf> {
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
pub(crate) fn name_matches(name: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

pub(crate) fn is_ignored(name: &str, patterns: &[impl AsRef<str>]) -> bool {
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
        assert_eq!(
            config.interval,
            secs_to_duration(DEFAULT_INTERVAL_SECS, DEFAULT_INTERVAL_SECS)
        );
        assert_eq!(config.usb_vendor_ids.len(), DEFAULT_USB_VENDOR_IDS.len());
    }

    #[test]
    fn every_value_reports_which_tier_set_it() {
        let mut config = Config::default();
        // Nothing has been applied, so everything is a default -- including a
        // key nobody has ever heard of, which must not panic.
        assert_eq!(config.origins.of("api"), Origin::Default);
        assert_eq!(config.origins.of("nonsense"), Origin::Default);

        let file: ConfigFile =
            toml::from_str("[dash]\napi = \"http://pi.local:9000\"\ntheme = \"green\"\n").unwrap();
        apply_file(&mut config, file);
        assert_eq!(config.origins.of("api"), Origin::File);
        assert_eq!(config.origins.of("theme"), Origin::File);
        assert_eq!(config.origins.of("glyphs"), Origin::Default);

        // The command line is last and wins, and says so.
        config.api = "http://pi:1".to_string();
        config.origins.set("api", Origin::Cli);
        assert_eq!(config.origins.of("api"), Origin::Cli);
        assert_eq!(Origin::Cli.label(), "command line");
    }

    #[test]
    fn a_blank_session_in_a_file_resolves_to_no_session() {
        // Otherwise the poller sends `classg_session=` on every request and
        // the API answers 401 to a dashboard that never had a credential.
        let mut config = Config::default();
        let file: ConfigFile = toml::from_str("[dash]\nsession = \"   \"\n").unwrap();
        apply_file(&mut config, file);
        assert_eq!(config.session.as_deref(), Some("   "));
        // load_config normalises on the way out; do the same thing it does.
        config.session = config
            .session
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        assert!(config.session.is_none());
    }

    #[test]
    fn intervals_are_clamped_away_from_zero() {
        assert!(
            secs_to_duration(0.0, DEFAULT_INTERVAL_SECS) >= std::time::Duration::from_millis(250)
        );
        assert!(
            secs_to_duration(-4.0, DEFAULT_INTERVAL_SECS) >= std::time::Duration::from_millis(250)
        );

        // A value that is not a number falls back to the default for the
        // setting being resolved, not to whichever one happened to be written
        // into the helper.
        assert_eq!(
            secs_to_duration(f64::NAN, DEFAULT_API_INTERVAL_SECS),
            std::time::Duration::from_secs_f64(DEFAULT_API_INTERVAL_SECS)
        );
        assert_eq!(
            secs_to_duration(f64::INFINITY, DEFAULT_INTERVAL_SECS),
            std::time::Duration::from_secs_f64(DEFAULT_INTERVAL_SECS)
        );
        assert!(secs_to_duration(f64::NAN, DEFAULT_INTERVAL_SECS) > std::time::Duration::ZERO);
        assert_eq!(
            secs_to_duration(0.5, DEFAULT_INTERVAL_SECS),
            std::time::Duration::from_millis(500)
        );
    }

    /// A file only this test writes, removed when it goes out of scope.
    ///
    /// Named after the test rather than the process, so two of these running
    /// beside each other cannot pick the same path.
    struct TempConfig(PathBuf);

    impl TempConfig {
        fn new(name: &str, body: &str) -> TempConfig {
            let path = std::env::temp_dir().join(format!("pi-dash-{name}.toml"));
            std::fs::write(&path, body).expect("writing a temp config");
            TempConfig(path)
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_named_file_that_is_not_there_is_an_error() {
        // The asymmetry the module comment argues for: a file you asked for
        // by name and did not get is a mistake worth stopping on, and one
        // merely absent from the search path is not -- the dashboard is more
        // useful up with defaults than not up at all.
        let missing = std::env::temp_dir().join("pi-dash-no-such-file-here.toml");
        let _ = std::fs::remove_file(&missing);
        assert!(load_config(Some(missing), &CliOverrides::default()).is_err());

        // Whereas searching and finding nothing is fine, whatever is or is
        // not on this machine's search path.
        assert!(load_config(None, &CliOverrides::default()).is_ok());
    }

    #[test]
    fn a_named_file_that_does_not_parse_says_which_file() {
        let file = TempConfig::new("broken", "[dash]\napi = \n");
        let error = load_config(Some(file.0.clone()), &CliOverrides::default())
            .expect_err("a malformed file must not be ignored");
        let text = format!("{error:#}");
        assert!(
            text.contains("pi-dash-broken.toml"),
            "the error must name the file: {text}"
        );
    }

    #[test]
    fn a_named_file_is_applied_and_reported_as_the_source() {
        // Deliberately not asserting on `api`: CLASSG_API in the environment
        // of whoever runs this would beat the file and the test would be
        // reporting on their shell rather than on this code.
        let file = TempConfig::new(
            "applied",
            "[dash]\ntheme = \"green\"\nglyphs = \"ascii\"\nprocesses = 12\n",
        );
        let config = load_config(Some(file.0.clone()), &CliOverrides::default())
            .expect("a valid file loads");

        assert_eq!(config.theme, "green");
        assert_eq!(config.glyphs, "ascii");
        assert_eq!(config.processes, Some(12));
        assert_eq!(config.source.as_ref(), Some(&file.0));
        assert_eq!(config.origins.of("theme"), Origin::File);
        // Anything the file did not mention keeps its default and says so.
        assert_eq!(config.origins.of("api_interval"), Origin::Default);
        assert_eq!(
            config.api_interval,
            secs_to_duration(DEFAULT_API_INTERVAL_SECS, DEFAULT_API_INTERVAL_SECS)
        );
    }

    #[test]
    fn the_command_line_beats_the_file_and_the_environment_both() {
        // Applied last of all, which is what makes this assertion safe to
        // make about `api` when the other two are not.
        let file = TempConfig::new("overridden", "[dash]\napi = \"http://from-file:1\"\n");
        let config = load_config(
            Some(file.0.clone()),
            &CliOverrides {
                api: Some("http://from-cli:2".to_string()),
                interval: Some(0.5),
            },
        )
        .expect("loads");

        assert_eq!(config.api, "http://from-cli:2");
        assert_eq!(config.origins.of("api"), Origin::Cli);
        assert_eq!(config.interval, std::time::Duration::from_millis(500));
        assert_eq!(config.origins.of("interval"), Origin::Cli);
    }

    #[test]
    fn a_trailing_slash_on_the_api_is_removed_before_anything_uses_it() {
        // Every request appends an absolute path, so a base that keeps its
        // slash produces //api/v1/health -- which some proxies answer and
        // some redirect and none of it is worth finding out about.
        let file = TempConfig::new("slash", "[dash]\napi = \"http://pi.local:8081/\"\n");
        let config = load_config(Some(file.0.clone()), &CliOverrides::default()).expect("loads");
        if config.origins.of("api") == Origin::File {
            assert_eq!(config.api, "http://pi.local:8081");
        }
    }

    #[test]
    fn an_unusable_interval_falls_back_to_its_own_default_not_the_other_one() {
        // TOML has `nan` as a float literal, so this is reachable from a file
        // somebody wrote. The API poll defaults to three seconds and the local
        // sample to two; a shared fallback resolved the first to the second's
        // value -- a wrong number from the one input that should have been
        // rejected outright.
        let file = TempConfig::new(
            "nan-interval",
            "[dash]
api_interval_secs = nan
",
        );
        let config = load_config(Some(file.0.clone()), &CliOverrides::default()).expect("loads");
        assert_eq!(
            config.api_interval,
            secs_to_duration(DEFAULT_API_INTERVAL_SECS, DEFAULT_API_INTERVAL_SECS),
            "the API poll fell back to the local sample's default"
        );
    }

    #[test]
    fn a_blank_session_in_a_file_resolves_to_no_credential_at_all() {
        // An empty value means "not set". Sent as a cookie it would be a
        // session token that does not exist, and the API would answer 401 to
        // a dashboard that never had a credential to begin with.
        let file = TempConfig::new("blank-session", "[dash]\nsession = \"   \"\n");
        let config = load_config(Some(file.0.clone()), &CliOverrides::default()).expect("loads");
        assert!(config.session.is_none());
    }

    #[test]
    fn an_unreadable_interval_in_a_file_is_clamped_rather_than_obeyed() {
        // A zero interval would spin the sample loop at full CPU on a box
        // whose whole problem is that it browns out under load.
        let file = TempConfig::new("zero-interval", "[dash]\ninterval_secs = 0.0\n");
        let config = load_config(Some(file.0.clone()), &CliOverrides::default()).expect("loads");
        assert!(config.interval >= std::time::Duration::from_millis(250));
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
