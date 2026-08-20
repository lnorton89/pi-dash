//! The ClassG pane: what the API on this box says about itself.
//!
//! Degraded, never fatal. An API that is not up is a normal state on a bare
//! Pi — you image the card, you plug in the radios, and you look at this
//! dashboard *before* you start the stack. So a failed poll produces a line
//! saying where it looked and how to start it, and every other pane carries on
//! sampling at its own rate.
//!
//! All network work happens on a background thread that posts snapshots back
//! through an mpsc channel, so a hung API cannot stall a redraw.
//!
//! # A third hand-copied mirror
//!
//! Everything below is a Rust transcription of Go structs in
//! `services/api/internal/{health,model,system,monitoring,capture}`. ClassG has
//! already been bitten twice by exactly this — `health.Sensor.optional` and
//! `model.Capture.error` were both being sent on every response and thrown away
//! by a hand-written TypeScript interface, which is why `scripts/check-mirrors.py`
//! exists. This is a third copy in a third language with no such check.
//!
//! `dead_code = "deny"` settles what to do about that, and settles it the
//! opposite way from the TypeScript: a field decoded here and never drawn does
//! not compile. So this file carries exactly what the pane renders, and adding
//! a field means finding it a column. The compiler cannot tell us a field is
//! missing, but it will not let one rot in silence either.
//!
//! What is carried follows the API's own rule — `Option` over a default that
//! lies. A disk with 0 bytes free renders as a plausible emergency; "unknown"
//! is the truth, and inventing the difference is the failure ADR-0003 exists to
//! prevent.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use serde::de::{self, Deserializer};
use serde::Deserialize;

/// `store.NormaliseLimit` on the API side accepts 1..=1000. Forty rows of
/// tracks is already more than anyone reads at a glance, and the pane only
/// ever renders what fits, so asking for more is bytes over the loopback for
/// nothing.
pub(crate) const MAX_ROWS: usize = 40;

/// Long enough for a Pi under load to answer, short enough that a wedged API
/// does not hold the poller past its next tick.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Polls between refreshes of the slow tier.
///
/// `/system` stats a filesystem and reads build info, `/captures` and
/// `/spectrum/sweeps` list stored records, and `/auth/me` answers a question
/// whose answer only changes when somebody logs in — none of which move at the
/// rate tracks and heartbeats do. At the default three-second cadence this is
/// every half-minute, which on a Pi is the difference between a dashboard you
/// can leave running and one you notice in the load average.
const SLOW_EVERY: u64 = 10;

/// Track states worth listing. `CLOSED` is deliberately absent: a closed track
/// is history, and on a busy afternoon enough of them accumulate to push every
/// live contact off the bottom of a pane that is only ever a few rows tall.
const LIVE_STATES: &str = "TENTATIVE,CONFIRMED,COASTING";

/// The session cookie `services/api/internal/httpapi/authapi.go` reads.
const SESSION_COOKIE: &str = "classg_session";

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// Accepts both timestamp encodings the ClassG bus carries.
///
/// `model.FlexTime` on the Go side decodes RFC3339 *and* the float epoch
/// seconds the Python sensor emits, so anything reading detections has to do
/// the same or it drops half the records it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlexTime(pub(crate) DateTime<Utc>);

impl FlexTime {
    /// Seconds between this timestamp and now. Negative when the record is
    /// stamped in the future, which happens when the Pi's clock has not
    /// caught up with NTP yet.
    pub(crate) fn age_secs(&self) -> i64 {
        (Utc::now() - self.0).num_seconds()
    }
}

impl<'de> Deserialize<'de> for FlexTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Epoch(f64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Epoch(secs) => {
                let nanos = (secs.fract() * 1e9).round() as u32;
                Utc.timestamp_opt(secs.trunc() as i64, nanos.min(999_999_999))
                    .single()
                    .map(FlexTime)
                    .ok_or_else(|| de::Error::custom("epoch timestamp out of range"))
            }
            Raw::Text(text) => DateTime::parse_from_rfc3339(&text)
                .map(|dt| FlexTime(dt.with_timezone(&Utc)))
                .map_err(de::Error::custom),
        }
    }
}

// ---------------------------------------------------------------------------
// The detection-class table
// ---------------------------------------------------------------------------

/// Short label for a detection class, mirroring
/// `services/ui/src/lib/detection-classes.ts`.
///
/// The letter alone is not a fact anyone can check. ClassG's own web app made
/// this call explicitly — "Class A (Remote ID) ×402 is a claim an operator can
/// check, and 94% is not" — and a dashboard that prints a bare `A` in a column
/// headed CLASS is asking the reader to have memorised a table from
/// `data-model.md`.
pub(crate) fn detection_class_label(code: &str) -> &'static str {
    match code {
        "A" => "Remote ID",
        "B" => "DJI DroneID",
        "C" => "OUI/SSID",
        "D" => "ADS-B",
        "E" => "Control link",
        "F" => "Analog FPV",
        "G" => "BLE RemoteID",
        "H" => "GNSS",
        _ => "",
    }
}

/// Evidence classes that corroborate an identification but never make one.
///
/// MIRRORS `corroboratingOnlyClasses` in `services/fusion/track.go`, by way of
/// `services/ui/src/features/tracks/tier.ts`. A class C hit means an OUI or an
/// SSID looked drone-like, which names whoever built the radio and not what is
/// flying it; D is manned traffic used only for suppression, and H is a noise
/// floor. Without this the pane draws a DJI-branded access point in the same
/// ink as a real Remote ID contact.
pub(crate) fn is_corroborating_only(class: &str) -> bool {
    matches!(class, "C" | "D" | "H")
}

// ---------------------------------------------------------------------------
// GET /api/v1/health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SensorHealth {
    #[serde(default)]
    pub(crate) sensor_id: String,
    #[serde(default)]
    pub(crate) sensor_kind: String,
    #[serde(default)]
    pub(crate) healthy: bool,
    /// Hardware this unit may not have fitted. An optional sensor that has
    /// never reported is a supported build, not a fault.
    #[serde(default)]
    pub(crate) optional: bool,
    #[serde(default)]
    pub(crate) seconds_since_heartbeat: Option<i64>,
    #[serde(default)]
    pub(crate) detections_5m: u64,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct FusionHealth {
    #[serde(default)]
    pub(crate) connected: bool,
    #[serde(default)]
    pub(crate) configured: bool,
    #[serde(default)]
    pub(crate) last_message: Option<FlexTime>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct HealthResponse {
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) uptime_s: u64,
    #[serde(default)]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) sensors: Vec<SensorHealth>,
    #[serde(default)]
    pub(crate) fusion: Option<FusionHealth>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/monitoring
// ---------------------------------------------------------------------------

/// The recording switch.
///
/// This is the single most important thing the pane learned to ask for. ClassG
/// records continuously by default and the UI can pause ingestion; a paused
/// stack is *indistinguishable from a quiet sky* on every other endpoint —
/// sensors heartbeat happily, fusion stays connected, and the track list is
/// empty because detections are being discarded at the ingest boundary. Without
/// this the dashboard reports a healthy detector that is recording nothing.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct MonitoringState {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) since: Option<FlexTime>,
    /// Operator-supplied when pausing, so the pane can say *why* the sky is
    /// not being watched rather than only that it is not.
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default, rename = "discarded_while_paused")]
    pub(crate) discarded: u64,
}

// ---------------------------------------------------------------------------
// GET /api/v1/auth/me
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct AuthUser {
    #[serde(default)]
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) role: String,
}

/// Public by design on the API side: its whole job is telling a client whether
/// a login screen, a setup screen or the app is the right thing to draw. Here
/// it answers the same question for a pane — an empty track list because
/// nothing is flying and an empty track list because every viewer endpoint
/// returned 401 look identical, and only this distinguishes them.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct AuthState {
    #[serde(default)]
    pub(crate) authenticated: bool,
    #[serde(default)]
    pub(crate) auth_enabled: bool,
    #[serde(default)]
    pub(crate) setup_required: bool,
    #[serde(default)]
    pub(crate) user: Option<AuthUser>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/system
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SystemBuild {
    #[serde(default)]
    pub(crate) version: String,
    /// Empty in container builds: `.dockerignore` excludes `.git`, so the
    /// toolchain has no VCS to stamp.
    #[serde(default)]
    pub(crate) revision: Option<String>,
    #[serde(default)]
    pub(crate) revision_dirty: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SystemRuntime {
    #[serde(default)]
    pub(crate) store: String,
    #[serde(default)]
    pub(crate) turso_sync_configured: bool,
    #[serde(default)]
    pub(crate) containerised: bool,
}

/// Every figure is an `Option` because the API's own rule is that a value it
/// could not read is null with a reason, never a zero. A disk with 0 bytes free
/// renders as a plausible emergency; "unavailable" is the truth.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SystemHost {
    #[serde(default)]
    pub(crate) disk_path: String,
    #[serde(default)]
    pub(crate) disk_total_bytes: Option<u64>,
    #[serde(default)]
    pub(crate) disk_free_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SystemInfo {
    #[serde(default)]
    pub(crate) build: SystemBuild,
    #[serde(default)]
    pub(crate) runtime: SystemRuntime,
    #[serde(default)]
    pub(crate) host: SystemHost,
}

impl SystemInfo {
    /// `0.4.1+a1b2c3d` / `0.4.1+a1b2c3d-dirty`. The revision is what tells you
    /// whether the binary running on the Pi is the one you last deployed.
    pub(crate) fn build_label(&self) -> String {
        let mut label = self.build.version.clone();
        if let Some(revision) = self.build.revision.as_deref().filter(|r| !r.is_empty()) {
            label.push('+');
            label.extend(revision.chars().take(7));
            if self.build.revision_dirty {
                label.push_str("-dirty");
            }
        }
        label
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/captures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct CaptureAnalysis {
    #[serde(default)]
    pub(crate) analyzed: bool,
    #[serde(default)]
    pub(crate) drone_transmitters: u64,
}

/// One pcap. A capture takes the monitor interface exclusively for its
/// duration, so a running one explains a Wi-Fi sensor that has gone quiet, and
/// `error` explains a failed one — the field the web app spent a release
/// discarding, which is the whole reason this file decodes more than it draws.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Capture {
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) started_at: Option<FlexTime>,
    #[serde(default)]
    pub(crate) iface: String,
    #[serde(default)]
    pub(crate) channel: u32,
    #[serde(default)]
    pub(crate) duration_s: u64,
    #[serde(default)]
    pub(crate) frame_count: u64,
    #[serde(default)]
    pub(crate) label: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) analysis: Option<CaptureAnalysis>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct CapturePage {
    #[serde(default)]
    pub(crate) captures: Vec<Capture>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/spectrum/sweeps
// ---------------------------------------------------------------------------

/// One band, measured once. A sweep borrows the SDR from dump1090 for its
/// duration (ADR-0008), so a running one is the answer to "why has ADS-B
/// stopped" and belongs on the same screen as the sensor that went quiet.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SpectrumSweep {
    #[serde(default)]
    pub(crate) band: String,
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) started_at: Option<FlexTime>,
    #[serde(default)]
    pub(crate) peak_dbfs: Option<f64>,
    /// Steps that read short. Non-zero means the band was not fully covered
    /// and the trace has genuine holes in it.
    #[serde(default)]
    pub(crate) short_reads: u64,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SweepPage {
    #[serde(default)]
    pub(crate) sweeps: Vec<SpectrumSweep>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/tracks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Identity {
    #[serde(default)]
    pub(crate) serial: Option<String>,
    #[serde(default)]
    pub(crate) vendor: Option<String>,
    #[serde(default)]
    pub(crate) model_hint: Option<String>,
    #[serde(default)]
    pub(crate) vendor_hint: Option<String>,
    /// Detections carry one `mac`; tracks carry the `macs` they have merged.
    #[serde(default)]
    pub(crate) mac: Option<String>,
    #[serde(default)]
    pub(crate) macs: Option<Vec<String>>,
}

impl Identity {
    /// The most specific name the identity carries. Matches the Bash
    /// version's preference order — a model hint beats a vendor beats a bare
    /// serial — because that is the order of how much it tells an operator.
    ///
    /// A MAC is last of all and only if nothing else spoke: it identifies a
    /// radio rather than an aircraft, and under MAC randomisation not even
    /// that for long. It still beats printing "unknown" at somebody who could
    /// match it against what they are looking at in `iw`.
    pub(crate) fn label(&self) -> String {
        let first_mac = self
            .macs
            .as_ref()
            .and_then(|macs| macs.first())
            .cloned()
            .or_else(|| self.mac.clone());
        for candidate in [
            &self.model_hint,
            &self.vendor,
            &self.vendor_hint,
            &self.serial,
            &first_mac,
        ] {
            if let Some(value) = candidate.as_deref().map(str::trim) {
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
        "unknown".to_string()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Evidence {
    #[serde(default)]
    pub(crate) class: String,
    #[serde(default)]
    pub(crate) count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Position {
    #[serde(default)]
    pub(crate) height_agl_m: Option<f64>,
    #[serde(default)]
    pub(crate) alt_geodetic_m: Option<f64>,
    #[serde(default)]
    pub(crate) speed_mps: Option<f64>,
}

impl Position {
    /// Height above ground if the aircraft reported it, geodetic altitude
    /// otherwise. The two are not interchangeable, so the caller is told which
    /// one it got and can label it.
    pub(crate) fn altitude(&self) -> Option<(f64, &'static str)> {
        if let Some(agl) = self.height_agl_m {
            return Some((agl, "agl"));
        }
        self.alt_geodetic_m.map(|alt| (alt, "alt"))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Track {
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) confidence: f64,
    #[serde(default)]
    pub(crate) first_seen: Option<FlexTime>,
    #[serde(default)]
    pub(crate) last_seen: Option<FlexTime>,
    #[serde(default)]
    pub(crate) detection_count: u64,
    #[serde(default)]
    pub(crate) identity: Option<Identity>,
    #[serde(default)]
    pub(crate) evidence: Vec<Evidence>,
    #[serde(default)]
    pub(crate) current: Option<Position>,
    #[serde(default)]
    pub(crate) rssi_dbm: Option<f64>,
    #[serde(default)]
    pub(crate) adsb_correlated: bool,
}

impl Track {
    /// Whether anything has identified this contact as an aircraft, as opposed
    /// to merely being consistent with one.
    ///
    /// No evidence at all is missing data, not weak data — fusion attaches some
    /// to every track it builds. Absence therefore defers to fusion rather than
    /// demoting, which matches `tier.ts` and deliberately *not* fusion's own
    /// `identified()`: that one decides whether to promote a track, so absence
    /// must not promote; this decides whether to show one as identified, and
    /// demoting on absence would hide a real aircraft whenever a response
    /// arrives trimmed.
    pub(crate) fn identified(&self) -> bool {
        if self.evidence.is_empty() {
            return true;
        }
        self.evidence
            .iter()
            .any(|e| !is_corroborating_only(&e.class))
    }

    /// The strongest piece of evidence, with how many times it was seen:
    /// `A x402`. Identifying classes win over corroborating ones however often
    /// the corroborating one repeated — a beacon at 10 Hz outnumbers a Remote
    /// ID broadcast within seconds, and it is still the Remote ID that says
    /// what is flying.
    pub(crate) fn evidence_summary(&self) -> String {
        let strongest = self
            .evidence
            .iter()
            .filter(|e| !e.class.is_empty())
            .max_by_key(|e| (!is_corroborating_only(&e.class), e.count));
        match strongest {
            Some(e) => format!("{}x{}", e.class, crate::format::compact_count(e.count)),
            None => String::new(),
        }
    }

    /// The evidence classes behind this track, as sorted letters: `AB`, `C`.
    pub(crate) fn evidence_classes(&self) -> String {
        let mut classes: Vec<&str> = self
            .evidence
            .iter()
            .map(|e| e.class.as_str())
            .filter(|c| !c.is_empty())
            .collect();
        classes.sort_unstable();
        classes.dedup();
        classes.concat()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct TrackPage {
    #[serde(default)]
    pub(crate) tracks: Vec<Track>,
    #[serde(default)]
    pub(crate) total: u64,
}

// ---------------------------------------------------------------------------
// GET /api/v1/detections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Rf {
    #[serde(default)]
    pub(crate) freq_hz: Option<i64>,
    #[serde(default)]
    pub(crate) channel: Option<u32>,
    #[serde(default)]
    pub(crate) rssi_dbm: Option<f64>,
}

impl Rf {
    /// `ch6` for something the Wi-Fi sensor names by channel, `915M` for
    /// something the SDR only knows by frequency. One column, because a pane
    /// this narrow cannot afford two and no detection ever fills both.
    ///
    /// Always megahertz, never a rounded `1.1G`. Every band this system cares
    /// about above a gigahertz is four digits of megahertz -- 1090 for ADS-B,
    /// 1200 and 1300 for analog FPV -- and one decimal place of gigahertz maps
    /// 1090 and 1150 onto the same string. `1090M` is also what anyone working
    /// on the radio would say out loud. Four digits plus the unit still fits
    /// the column, and nothing here reaches ten gigahertz.
    pub(crate) fn tuning(&self) -> Option<String> {
        if let Some(channel) = self.channel {
            return Some(format!("ch{channel}"));
        }
        Some(format!("{:.0}M", self.freq_hz? as f64 / 1e6))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Adsb {
    #[serde(default)]
    pub(crate) icao: String,
    #[serde(default)]
    pub(crate) callsign: Option<String>,
}

impl Adsb {
    /// A callsign if the aircraft broadcast one, the ICAO hex otherwise.
    pub(crate) fn label(&self) -> String {
        match self.callsign.as_deref().map(str::trim) {
            Some(callsign) if !callsign.is_empty() => callsign.to_string(),
            _ => self.icao.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct SignalFeatures {
    #[serde(default)]
    pub(crate) protocol_hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct Detection {
    #[serde(default)]
    pub(crate) ts: Option<FlexTime>,
    #[serde(default)]
    pub(crate) sensor_id: String,
    #[serde(default)]
    pub(crate) sensor_kind: String,
    #[serde(default)]
    pub(crate) detection_class: String,
    #[serde(default)]
    pub(crate) rf: Option<Rf>,
    #[serde(default)]
    pub(crate) identity: Option<Identity>,
    #[serde(default)]
    pub(crate) adsb: Option<Adsb>,
    #[serde(default)]
    pub(crate) signal_features: Option<SignalFeatures>,
}

impl Detection {
    /// The best short name for what was heard: a callsign for manned traffic,
    /// an identity for a drone, a protocol hint for a bare control link.
    pub(crate) fn label(&self) -> String {
        if let Some(adsb) = &self.adsb {
            let label = adsb.label();
            if !label.is_empty() {
                return label;
            }
        }
        if let Some(identity) = &self.identity {
            let label = identity.label();
            if label != "unknown" {
                return label;
            }
        }
        self.signal_features
            .as_ref()
            .and_then(|f| f.protocol_hint.clone())
            .filter(|hint| !hint.is_empty())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct DetectionPage {
    #[serde(default)]
    pub(crate) detections: Vec<Detection>,
    #[serde(default)]
    pub(crate) total: u64,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct ApiErrorEnvelope {
    error: ApiError,
}

/// The contract's single error envelope, from `services/api/internal/apierr`.
///
/// Every non-2xx response in the service goes through it, so parsing it once
/// means a 404 from a typo'd path and a 401 from a missing session both arrive
/// here as a sentence somebody wrote for a human to read.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct ApiError {
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) message: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FetchError {
    pub(crate) message: String,
    /// The contract's error code when the API answered in its own words.
    /// `None` for a transport failure, where nothing answered at all.
    pub(crate) code: Option<String>,
}

impl FetchError {
    fn transport(message: String) -> Self {
        FetchError {
            message,
            code: None,
        }
    }

    /// Whether this is the API refusing us rather than failing. These are the
    /// codes that mean "the endpoint is fine, you are not logged in", and they
    /// want saying once at the top of the pane rather than three times over as
    /// three empty sections.
    fn is_refusal(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some("unauthenticated" | "forbidden" | "privileges_required" | "setup_required")
        )
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// The slow tier, kept between polls.
///
/// Held separately from [`Snapshot`] so that refreshing it every tenth poll
/// does not make the pane blink these sections in and out nine times out of
/// ten.
#[derive(Debug, Clone, Default)]
pub(crate) struct Slow {
    pub(crate) auth: Option<AuthState>,
    pub(crate) system: Option<SystemInfo>,
    pub(crate) captures: Option<CapturePage>,
    pub(crate) sweeps: Option<SweepPage>,
}

/// One poll's worth of results. `health` failing is what makes the pane
/// degraded; every other endpoint failing on its own only costs its own
/// section, exactly as in the Bash version.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) health: Option<HealthResponse>,
    pub(crate) error: Option<String>,
    pub(crate) monitoring: Option<MonitoringState>,
    pub(crate) tracks: Option<TrackPage>,
    pub(crate) detections: Option<DetectionPage>,
    pub(crate) slow: Slow,
    /// Set when a viewer-level endpoint refused the poller. An empty track
    /// list because nothing is flying and an empty one because every request
    /// 401s are the same picture; this is the caption that tells them apart.
    pub(crate) denied: Option<String>,
}

impl Snapshot {
    /// The running capture, if one is. At most one can run at a time — the
    /// monitor interface is a single exclusive resource and the capture
    /// manager holds a `busy` flag to keep it that way.
    pub(crate) fn running_capture(&self) -> Option<&Capture> {
        self.slow
            .captures
            .as_ref()?
            .captures
            .iter()
            .find(|c| c.state == "running")
    }

    /// The most recent capture, running or not, so a failure states its reason
    /// instead of scrolling away unread.
    ///
    /// Picked by start time rather than by position in the list: the API does
    /// not promise an order, and "the newest one" is the claim being made.
    pub(crate) fn latest_capture(&self) -> Option<&Capture> {
        self.slow
            .captures
            .as_ref()?
            .captures
            .iter()
            .max_by_key(|c| c.started_at.map(|ts| ts.0))
    }

    pub(crate) fn running_sweep(&self) -> Option<&SpectrumSweep> {
        self.slow
            .sweeps
            .as_ref()?
            .sweeps
            .iter()
            .find(|s| s.state == "running")
    }

    pub(crate) fn latest_sweep(&self) -> Option<&SpectrumSweep> {
        self.slow
            .sweeps
            .as_ref()?
            .sweeps
            .iter()
            .max_by_key(|s| s.started_at.map(|ts| ts.0))
    }
}

/// How many rows each list section can currently show. Written by the
/// drawing code, read by the poller, so a tall terminal fetches a real track
/// list and a short one does not fetch rows it will throw away.
#[derive(Debug, Default)]
pub(crate) struct RowHints {
    pub(crate) tracks: AtomicUsize,
    pub(crate) detections: AtomicUsize,
}

impl RowHints {
    fn limit(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::Relaxed).clamp(1, MAX_ROWS)
    }
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// Everything needed to talk to one ClassG API.
///
/// Shared by the poller thread and by `--once`, so the two cannot drift apart
/// on which endpoints they ask for or on how they authenticate.
pub(crate) struct Client {
    agent: ureq::Agent,
    base: String,
    credential: Option<Credential>,
}

/// How this process proves who it is to the API.
///
/// One enum rather than two Options, because the precedence between them is a
/// decision and not an accident: a session is a person's, a local token is this
/// machine's, and only one of them is ever sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credential {
    /// A session cookie copied from a browser, or set in the config file. Can
    /// do whatever the person it belongs to can.
    Session(String),
    /// The local-agent token the API wrote into this unit's state directory.
    /// Viewer only, and nobody had to paste anything.
    Local(String),
}

impl Credential {
    /// Picks the credential to use, discarding blanks.
    ///
    /// An empty string in a config file means "not set", not "send an empty
    /// cookie" -- which the API would read as a session token that does not
    /// exist and answer 401 to, on every poll, for ever.
    pub(crate) fn pick(session: Option<String>, local: Option<String>) -> Option<Credential> {
        let clean = |v: Option<String>| v.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        if let Some(token) = clean(session) {
            return Some(Credential::Session(token));
        }
        clean(local).map(Credential::Local)
    }
}

impl Client {
    pub(crate) fn new(base: String, credential: Option<Credential>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(READ_TIMEOUT)
            .user_agent(concat!("pi-dash/", env!("CARGO_PKG_VERSION")))
            .build();
        Client {
            agent,
            base,
            credential,
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, FetchError> {
        // This build has no TLS: the API is on loopback by default and linking
        // a TLS stack to reach 127.0.0.1 is not a trade worth making on a Pi.
        // Say so plainly rather than failing with a confusing transport error.
        if self.base.starts_with("https://") {
            return Err(FetchError::transport(format!(
                "https is not supported by this build — point CLASSG_API at http:// ({})",
                self.base
            )));
        }
        let url = format!("{}{}", self.base.trim_end_matches('/'), path);
        let mut request = self.agent.get(&url);
        match &self.credential {
            Some(Credential::Session(token)) => {
                request = request.set("Cookie", &format!("{SESSION_COOKIE}={token}"));
            }
            // Bearer, not a cookie: this is not a session and the API does not
            // look for it in the cookie jar. See internal/auth/localagent.go.
            Some(Credential::Local(token)) => {
                request = request.set("Authorization", &format!("Bearer {token}"));
            }
            None => {}
        }

        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                // Every non-2xx in the service carries the contract envelope,
                // so prefer the API's own sentence over a bare status number.
                let body = response.into_string().unwrap_or_default();
                return Err(match serde_json::from_str::<ApiErrorEnvelope>(&body) {
                    Ok(envelope) => FetchError {
                        message: envelope.error.message,
                        code: Some(envelope.error.code),
                    },
                    Err(_) => FetchError::transport(format!("HTTP {code} from {path}")),
                });
            }
            Err(ureq::Error::Transport(transport)) => {
                return Err(FetchError::transport(transport.to_string()))
            }
        };

        let body = response
            .into_string()
            .map_err(|err| FetchError::transport(format!("could not read {path}: {err}")))?;
        serde_json::from_str(&body)
            .map_err(|err| FetchError::transport(format!("bad JSON from {path}: {err}")))
    }

    /// One complete poll.
    ///
    /// `previous` carries the slow tier forward on the nine polls in ten that
    /// do not refresh it. Pass `Slow::default()` alongside `refresh_slow` to
    /// force a full fetch, which is what `--once` wants.
    pub(crate) fn poll(
        &self,
        track_rows: usize,
        detection_rows: usize,
        refresh_slow: bool,
        previous: &Slow,
    ) -> Snapshot {
        let health = match self.get_json::<HealthResponse>("/api/v1/health") {
            // /health is open on the API side, so a failure here is the whole
            // service being unreachable and there is nothing else worth asking.
            Err(error) => {
                return Snapshot {
                    error: Some(error.message),
                    ..Default::default()
                }
            }
            Ok(health) => health,
        };

        let mut denied: Option<String> = None;
        // Records the first refusal and swallows every later one: three
        // sections each reporting "log in to continue" says it no better than
        // one line at the top does.
        let mut note = |error: FetchError| {
            if error.is_refusal() && denied.is_none() {
                denied = Some(error.message);
            }
        };

        let tracks = self
            .get_json(&format!(
                "/api/v1/tracks?state={LIVE_STATES}&limit={}",
                track_rows.clamp(1, MAX_ROWS)
            ))
            .map_err(&mut note)
            .ok();
        let detections = self
            .get_json(&format!(
                "/api/v1/detections?limit={}",
                detection_rows.clamp(1, MAX_ROWS)
            ))
            .map_err(&mut note)
            .ok();
        let monitoring = self.get_json("/api/v1/monitoring").map_err(&mut note).ok();

        let slow = if refresh_slow {
            Slow {
                // /auth/me is open and is what explains every refusal above, so
                // it is asked for even when nothing else answered.
                auth: self.get_json("/api/v1/auth/me").ok(),
                system: self.get_json("/api/v1/system").map_err(&mut note).ok(),
                captures: self.get_json("/api/v1/captures").map_err(&mut note).ok(),
                sweeps: self
                    .get_json("/api/v1/spectrum/sweeps?limit=5")
                    .map_err(&mut note)
                    .ok(),
            }
        } else {
            previous.clone()
        };

        Snapshot {
            health: Some(health),
            error: None,
            monitoring,
            tracks,
            detections,
            slow,
            denied,
        }
    }
}

// ---------------------------------------------------------------------------
// The pane
// ---------------------------------------------------------------------------

pub(crate) struct ClassgPane {
    pub(crate) base: String,
    pub(crate) snapshot: Snapshot,
    /// When the last *successful* health poll landed, so the pane can show
    /// how stale it is rather than freezing on old numbers.
    pub(crate) last_ok: Option<Instant>,
    pub(crate) polls: u64,
    pub(crate) hints: Arc<RowHints>,
    shutdown: Arc<AtomicBool>,
    rx: Receiver<Snapshot>,
}

impl std::fmt::Debug for ClassgPane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassgPane")
            .field("base", &self.base)
            .field("polls", &self.polls)
            .finish_non_exhaustive()
    }
}

impl ClassgPane {
    /// Spawns the poller. Returns immediately; the first snapshot arrives on
    /// the channel a moment later, and until then the pane says "connecting".
    pub(crate) fn spawn(base: String, credential: Option<Credential>, interval: Duration) -> Self {
        let (tx, rx) = channel();
        let hints = Arc::new(RowHints::default());
        let shutdown = Arc::new(AtomicBool::new(false));

        let worker_base = base.clone();
        let worker_hints = Arc::clone(&hints);
        let worker_shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            let client = Client::new(worker_base, credential);
            poll_loop(&client, interval, &tx, &worker_hints, &worker_shutdown);
        });

        ClassgPane {
            base,
            snapshot: Snapshot::default(),
            last_ok: None,
            polls: 0,
            hints,
            shutdown,
            rx,
        }
    }

    /// Drains whatever the poller has posted. Only the newest snapshot is
    /// kept: if a redraw was blocked for a while there is no value in
    /// replaying the ones nobody saw.
    pub(crate) fn drain(&mut self) -> bool {
        let mut updated = false;
        while let Ok(snapshot) = self.rx.try_recv() {
            if snapshot.health.is_some() {
                self.last_ok = Some(Instant::now());
            }
            self.snapshot = snapshot;
            self.polls += 1;
            updated = true;
        }
        updated
    }

    pub(crate) fn set_hints(&self, tracks: usize, detections: usize) {
        self.hints.tracks.store(tracks, Ordering::Relaxed);
        self.hints.detections.store(detections, Ordering::Relaxed);
    }
}

impl Drop for ClassgPane {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

fn poll_loop(
    client: &Client,
    interval: Duration,
    tx: &Sender<Snapshot>,
    hints: &RowHints,
    shutdown: &AtomicBool,
) {
    let mut polls: u64 = 0;
    let mut slow = Slow::default();

    while !shutdown.load(Ordering::Relaxed) {
        // Zero is a multiple of everything, so the first poll fetches the slow
        // tier too: a dashboard that waits half a minute before it can say the
        // API wants a login is one you have already stopped reading.
        let snapshot = client.poll(
            RowHints::limit(&hints.tracks),
            RowHints::limit(&hints.detections),
            polls.is_multiple_of(SLOW_EVERY),
            &slow,
        );
        slow = snapshot.slow.clone();
        polls = polls.wrapping_add(1);

        if tx.send(snapshot).is_err() {
            return; // the UI went away
        }
        // Sleep in slices so quitting does not wait out a whole interval.
        let mut slept = Duration::ZERO;
        while slept < interval && !shutdown.load(Ordering::Relaxed) {
            let slice = Duration::from_millis(100).min(interval - slept);
            std::thread::sleep(slice);
            slept += slice;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_decodes_the_shape_the_api_documents() {
        let json = r#"{
            "status": "degraded",
            "uptime_s": 7412,
            "version": "0.4.1",
            "sensors": [
                {"sensor_id":"wifi-1","sensor_kind":"wifi","healthy":true,
                 "last_heartbeat":"2026-08-16T10:00:00Z",
                 "seconds_since_heartbeat":3,"detections_5m":128},
                {"sensor_id":"sdr-1","sensor_kind":"sdr","healthy":false,
                 "seconds_since_heartbeat":null,"detections_5m":0,
                 "reason":"rtl_sdr: device not found"}
            ],
            "fusion": {"connected": true, "configured": true,
                       "last_message": "2026-08-16T10:00:00Z"}
        }"#;
        let health: HealthResponse = serde_json::from_str(json).expect("decoded");
        assert_eq!(health.status, "degraded");
        assert_eq!(health.sensors.len(), 2);
        assert_eq!(health.sensors[0].seconds_since_heartbeat, Some(3));
        assert_eq!(health.sensors[1].seconds_since_heartbeat, None);
        assert_eq!(
            health.sensors[1].reason.as_deref(),
            Some("rtl_sdr: device not found")
        );
        assert!(health.fusion.expect("fusion").connected);
    }

    #[test]
    fn a_minimal_health_body_still_decodes() {
        // Every field defaulted: an older API, or one still starting up.
        let health: HealthResponse = serde_json::from_str("{}").expect("decoded");
        assert_eq!(health.status, "");
        assert!(health.sensors.is_empty());
        assert!(health.fusion.is_none());
    }

    #[test]
    fn timestamps_decode_from_rfc3339_and_from_float_epoch_seconds() {
        #[derive(Deserialize)]
        struct Wrapper {
            ts: FlexTime,
        }
        // 2026-08-16T12:34:56Z.
        const WHEN: i64 = 1_786_883_696;
        let text: Wrapper =
            serde_json::from_str(r#"{"ts":"2026-08-16T12:34:56Z"}"#).expect("rfc3339");
        let epoch: Wrapper = serde_json::from_str(r#"{"ts":1786883696.0}"#).expect("epoch");
        assert_eq!(text.ts.0.timestamp(), WHEN);
        assert_eq!(epoch.ts.0.timestamp(), WHEN);

        // Offsets, and integers rather than floats, both appear on the bus.
        let offset: Wrapper =
            serde_json::from_str(r#"{"ts":"2026-08-16T14:34:56+02:00"}"#).expect("offset");
        assert_eq!(offset.ts.0.timestamp(), WHEN);
        let integer: Wrapper = serde_json::from_str(r#"{"ts":1786883696}"#).expect("integer");
        assert_eq!(integer.ts.0.timestamp(), WHEN);

        // Sub-second precision survives: the Python sensor stamps floats.
        let fractional: Wrapper =
            serde_json::from_str(r#"{"ts":1786883696.25}"#).expect("fractional");
        assert_eq!(fractional.ts.0.timestamp_subsec_millis(), 250);
    }

    #[test]
    fn an_undecodable_timestamp_is_an_error_not_a_silent_epoch_zero() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[expect(dead_code, reason = "decoded into, never read")]
            ts: FlexTime,
        }
        assert!(serde_json::from_str::<Wrapper>(r#"{"ts":"yesterday"}"#).is_err());
    }

    #[test]
    fn tracks_decode_with_partial_identities() {
        let json = r#"{
            "tracks": [
                {"track_id":"t1","state":"CONFIRMED","confidence":0.82,
                 "last_seen":"2026-08-16T12:00:00Z","detection_count":402,
                 "adsb_correlated":false,
                 "identity":{"model_hint":"Mavic 3","vendor":"DJI"}},
                {"track_id":"t2","state":"TENTATIVE","confidence":0.31,
                 "identity":{"serial":"1581F"}},
                {"track_id":"t3","state":"COASTING","confidence":0.5}
            ],
            "total": 3
        }"#;
        let page: TrackPage = serde_json::from_str(json).expect("decoded");
        assert_eq!(page.total, 3);
        assert_eq!(page.tracks[0].identity.as_ref().unwrap().label(), "Mavic 3");
        assert_eq!(page.tracks[0].detection_count, 402);
        assert_eq!(page.tracks[1].identity.as_ref().unwrap().label(), "1581F");
        assert!(page.tracks[2].identity.is_none());
    }

    #[test]
    fn identity_prefers_the_most_specific_label_and_ignores_blanks() {
        let identity = Identity {
            model_hint: Some("  ".to_string()),
            vendor: Some("DJI".to_string()),
            ..Default::default()
        };
        assert_eq!(identity.label(), "DJI");
        assert_eq!(Identity::default().label(), "unknown");
    }

    #[test]
    fn a_mac_names_a_contact_only_when_nothing_better_did() {
        // A MAC identifies a radio, not an aircraft, so it must never win over
        // a model hint.
        let macs = Identity {
            macs: Some(vec!["26:37:12:aa:bb:cc".to_string()]),
            ..Default::default()
        };
        assert_eq!(macs.label(), "26:37:12:aa:bb:cc");
        let named = Identity {
            macs: Some(vec!["26:37:12:aa:bb:cc".to_string()]),
            model_hint: Some("Mavic 3".to_string()),
            ..Default::default()
        };
        assert_eq!(named.label(), "Mavic 3");
    }

    #[test]
    fn a_track_built_only_from_corroborating_evidence_is_not_identified() {
        // The 2026-08-17 case from services/fusion/track.go: a DJI-OUI access
        // point on ch149 that sat beside a real Remote ID track for a full
        // CloseAfter window, indistinguishable at a glance.
        let oui_only = Track {
            evidence: vec![Evidence {
                class: "C".to_string(),
                count: 140,
            }],
            ..Default::default()
        };
        assert!(!oui_only.identified());

        let remote_id = Track {
            evidence: vec![
                Evidence {
                    class: "C".to_string(),
                    count: 140,
                },
                Evidence {
                    class: "A".to_string(),
                    count: 12,
                },
            ],
            ..Default::default()
        };
        assert!(remote_id.identified());
        assert_eq!(remote_id.evidence_classes(), "AC");
        // The OUI hit outnumbers the Remote ID broadcast more than ten to one
        // and still loses: it is not what identified the aircraft.
        assert_eq!(remote_id.evidence_summary(), "Ax12");
        assert_eq!(oui_only.evidence_summary(), "Cx140");
        assert_eq!(Track::default().evidence_summary(), "");

        // Absence is missing data, not weak data: fusion attaches evidence to
        // every track it builds, so a trimmed response must not demote.
        assert!(Track::default().identified());
    }

    #[test]
    fn detections_decode_with_a_missing_rf_block() {
        let json = r#"{
            "detections": [
                {"ts":1786012496.25,"sensor_id":"wifi-1","sensor_kind":"wifi",
                 "detection_class":"A",
                 "rf":{"channel":6,"rssi_dbm":-52.0,"snr_db":18.5,
                       "freq_hz":2437000000}},
                {"ts":"2026-08-16T12:00:00Z","sensor_kind":"sdr",
                 "detection_class":"D",
                 "adsb":{"icao":"a1b2c3","callsign":"N172SP","alt_ft":3500}}
            ],
            "total": 2
        }"#;
        let page: DetectionPage = serde_json::from_str(json).expect("decoded");
        let rf = page.detections[0].rf.as_ref().unwrap();
        assert_eq!(rf.channel, Some(6));
        assert_eq!(page.detections[0].sensor_id, "wifi-1");
        // snr_db is in the fixture and is deliberately not a field: an unknown
        // key must be ignored rather than fail the whole page, or one additive
        // change on the API side blanks this pane.
        assert!(page.detections[1].rf.is_none());
        // Manned traffic names itself; that is the whole value of a class D.
        assert_eq!(page.detections[1].label(), "N172SP");
    }

    #[test]
    fn tuning_reads_as_a_channel_or_a_frequency_but_never_as_both() {
        let wifi = Rf {
            channel: Some(149),
            freq_hz: Some(5_745_000_000),
            ..Default::default()
        };
        assert_eq!(wifi.tuning().as_deref(), Some("ch149"));
        let ism = Rf {
            freq_hz: Some(915_000_000),
            ..Default::default()
        };
        assert_eq!(ism.tuning().as_deref(), Some("915M"));
        assert_eq!(Rf::default().tuning(), None);
    }

    #[test]
    fn the_bands_above_a_gigahertz_stay_told_apart() {
        // A single decimal place of gigahertz put ADS-B at 1.1G and analog FPV
        // at 1.2G, and mapped 1090 and 1150 onto the same string. These are
        // four digits of megahertz and they stay that way.
        let tuning = |hz: i64| {
            Rf {
                freq_hz: Some(hz),
                ..Default::default()
            }
            .tuning()
        };
        assert_eq!(tuning(1_090_000_000).as_deref(), Some("1090M"));
        assert_eq!(tuning(1_200_000_000).as_deref(), Some("1200M"));
        assert_eq!(tuning(1_300_000_000).as_deref(), Some("1300M"));
        assert_eq!(tuning(5_745_000_000).as_deref(), Some("5745M"));
        // Five characters, which is what the TUNE column was built for.
        for hz in [433_000_000, 1_090_000_000, 5_745_000_000] {
            let text = tuning(hz).expect("a frequency");
            assert!(text.len() <= 5, "{text} will not fit the column");
        }
    }

    #[test]
    fn an_adsb_hit_with_no_callsign_falls_back_to_the_icao_hex() {
        let detection: Detection = serde_json::from_str(
            r#"{"detection_class":"D","adsb":{"icao":"a1b2c3","callsign":""}}"#,
        )
        .expect("decoded");
        assert_eq!(detection.label(), "a1b2c3");
    }

    #[test]
    fn the_detection_class_table_matches_the_one_the_web_app_shows() {
        assert_eq!(detection_class_label("A"), "Remote ID");
        assert_eq!(detection_class_label("D"), "ADS-B");
        assert_eq!(detection_class_label("H"), "GNSS");
        // An unknown class must render as nothing rather than as a guess: a
        // class the API grew after this build shipped is not class A.
        assert_eq!(detection_class_label("Z"), "");
        // The corroborating set, mirrored from fusion.
        for class in ["C", "D", "H"] {
            assert!(is_corroborating_only(class), "{class} corroborates only");
        }
        for class in ["A", "B", "E", "F", "G"] {
            assert!(!is_corroborating_only(class), "{class} identifies");
        }
    }

    #[test]
    fn monitoring_decodes_a_pause_with_its_reason_and_its_toll() {
        let state: MonitoringState = serde_json::from_str(
            r#"{"enabled":false,"since":"2026-08-16T12:00:00Z",
                "reason":"known local flight","discarded_while_paused":1204}"#,
        )
        .expect("decoded");
        assert!(!state.enabled);
        assert_eq!(state.discarded, 1204);
        assert_eq!(state.reason.as_deref(), Some("known local flight"));
    }

    #[test]
    fn system_reports_a_build_you_can_match_against_a_deploy() {
        let info: SystemInfo = serde_json::from_str(
            r#"{"build":{"version":"0.4.1","revision":"a1b2c3d4e5f6","revision_dirty":true},
                "runtime":{"store":"libsql","turso_sync_configured":false},
                "host":{"disk_path":"/var/lib/classg","disk_total_bytes":31000000000,
                        "disk_free_bytes":12400000000}}"#,
        )
        .expect("decoded");
        assert_eq!(info.build_label(), "0.4.1+a1b2c3d-dirty");
        assert_eq!(info.host.disk_free_bytes, Some(12_400_000_000));

        // A container build has no VCS stamp, and an empty revision must not
        // render as a bare `+`.
        let containerised: SystemInfo =
            serde_json::from_str(r#"{"build":{"version":"0.4.1","revision":""}}"#)
                .expect("decoded");
        assert_eq!(containerised.build_label(), "0.4.1");
    }

    #[test]
    fn a_failed_capture_keeps_the_reason_it_failed() {
        // The field ClassG's own web app spent a release discarding.
        let page: CapturePage = serde_json::from_str(
            r#"{"captures":[{"capture_id":"c1","state":"failed","iface":"wlan1",
                             "channel":6,"duration_s":60,
                             "error":"tcpdump: wlan1: No such device"}]}"#,
        )
        .expect("decoded");
        assert_eq!(
            page.captures[0].error.as_deref(),
            Some("tcpdump: wlan1: No such device")
        );
    }

    #[test]
    fn an_https_base_url_explains_itself_instead_of_failing_obscurely() {
        let client = Client::new("https://pi.local:8081".to_string(), None);
        let result: Result<HealthResponse, FetchError> = client.get_json("/api/v1/health");
        let error = result.expect_err("https must be refused");
        assert!(
            error.message.contains("https is not supported"),
            "got {}",
            error.message
        );
    }

    #[test]
    fn an_unreachable_api_returns_an_error_snapshot_not_a_panic() {
        // Port 1 on loopback: reserved, nothing will ever be listening.
        let client = Client::new("http://127.0.0.1:1".to_string(), None);
        let snapshot = client.poll(10, 10, true, &Slow::default());
        assert!(snapshot.health.is_none());
        assert!(snapshot.error.is_some());
        assert!(snapshot.tracks.is_none());
        assert!(
            snapshot.denied.is_none(),
            "nothing refused us; nothing answered at all"
        );
    }

    #[test]
    fn a_blank_session_token_is_treated_as_no_token_at_all() {
        // An empty value in a config file means "not set". Sent as a cookie it
        // would be a session token that does not exist, and the API would
        // answer 401 to a poller that never had credentials to begin with.
        assert_eq!(Credential::pick(Some("   ".to_string()), None), None);
        assert_eq!(Credential::pick(Some(String::new()), None), None);
        assert_eq!(
            Credential::pick(Some("abc123".to_string()), None),
            Some(Credential::Session("abc123".to_string()))
        );
    }

    #[test]
    fn a_session_beats_the_local_token() {
        // Someone who exported CLASSG_SESSION meant it, and the usual reason
        // is pointing this build at a DIFFERENT unit -- where the token lying
        // on this disk describes the wrong box entirely.
        assert_eq!(
            Credential::pick(Some("human".to_string()), Some("machine".to_string())),
            Some(Credential::Session("human".to_string()))
        );
        // A blank session is not a session, so the local token still wins.
        assert_eq!(
            Credential::pick(Some("  ".to_string()), Some("machine".to_string())),
            Some(Credential::Local("machine".to_string()))
        );
        assert_eq!(
            Credential::pick(None, Some("machine".to_string())),
            Some(Credential::Local("machine".to_string()))
        );
        assert_eq!(Credential::pick(None, None), None);
    }

    #[test]
    fn a_refusal_is_told_apart_from_a_failure() {
        let refused = FetchError {
            message: "log in to continue".to_string(),
            code: Some("unauthenticated".to_string()),
        };
        assert!(refused.is_refusal());
        let broken = FetchError {
            message: "listing tracks failed".to_string(),
            code: Some("internal".to_string()),
        };
        assert!(!broken.is_refusal());
        assert!(!FetchError::transport("connection refused".to_string()).is_refusal());
    }

    #[test]
    fn the_error_envelope_is_read_rather_than_the_status_number() {
        let envelope: ApiErrorEnvelope = serde_json::from_str(
            r#"{"error":{"code":"unauthenticated","message":"log in to continue"}}"#,
        )
        .expect("decoded");
        assert_eq!(envelope.error.code, "unauthenticated");
        assert_eq!(envelope.error.message, "log in to continue");
    }

    #[test]
    fn row_hints_are_clamped_into_the_range_the_api_accepts() {
        let hints = RowHints::default();
        assert_eq!(RowHints::limit(&hints.tracks), 1, "zero would be rejected");
        hints.tracks.store(9999, Ordering::Relaxed);
        assert_eq!(RowHints::limit(&hints.tracks), MAX_ROWS);
        hints.tracks.store(12, Ordering::Relaxed);
        assert_eq!(RowHints::limit(&hints.tracks), 12);
    }
}
