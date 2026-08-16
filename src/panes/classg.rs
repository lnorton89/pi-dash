//! The ClassG pane: `GET /api/v1/health`, `/tracks` and `/detections`.
//!
//! Degraded, never fatal. An API that is not up is a normal state on a bare
//! Pi — you image the card, you plug in the radios, and you look at this
//! dashboard *before* you start the stack. So a failed poll produces a line
//! saying where it looked and how to start it, and every other pane carries on
//! sampling at its own rate.
//!
//! All network work happens on a background thread that posts snapshots back
//! through an mpsc channel, so a hung API cannot stall a redraw.

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
pub const MAX_ROWS: usize = 40;

/// Long enough for a Pi under load to answer, short enough that a wedged API
/// does not hold the poller past its next tick.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Accepts both timestamp encodings the ClassG bus carries.
///
/// `model.FlexTime` on the Go side decodes RFC3339 *and* the float epoch
/// seconds the Python sensor emits, so anything reading detections has to do
/// the same or it drops half the records it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlexTime(pub DateTime<Utc>);

impl FlexTime {
    /// Seconds between this timestamp and now. Negative when the record is
    /// stamped in the future, which happens when the Pi's clock has not
    /// caught up with NTP yet.
    pub fn age_secs(&self) -> i64 {
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SensorHealth {
    #[serde(default)]
    pub sensor_id: String,
    #[serde(default)]
    pub sensor_kind: String,
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub seconds_since_heartbeat: Option<i64>,
    #[serde(default)]
    pub detections_5m: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FusionHealth {
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub last_message: Option<FlexTime>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HealthResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub uptime_s: u64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub sensors: Vec<SensorHealth>,
    #[serde(default)]
    pub fusion: Option<FusionHealth>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Identity {
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub model_hint: Option<String>,
    #[serde(default)]
    pub vendor_hint: Option<String>,
}

impl Identity {
    /// The most specific name the identity carries. Matches the Bash
    /// version's preference order — a model hint beats a vendor beats a bare
    /// serial — because that is the order of how much it tells an operator.
    pub fn label(&self) -> String {
        for candidate in [
            &self.model_hint,
            &self.vendor,
            &self.vendor_hint,
            &self.serial,
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
pub struct Track {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub last_seen: Option<FlexTime>,
    #[serde(default)]
    pub identity: Option<Identity>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TrackPage {
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub total: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Rf {
    #[serde(default)]
    pub channel: Option<u32>,
    #[serde(default)]
    pub rssi_dbm: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Detection {
    #[serde(default)]
    pub ts: Option<FlexTime>,
    #[serde(default)]
    pub sensor_kind: String,
    #[serde(default)]
    pub detection_class: String,
    #[serde(default)]
    pub rf: Option<Rf>,
    #[serde(default)]
    pub identity: Option<Identity>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DetectionPage {
    #[serde(default)]
    pub detections: Vec<Detection>,
    #[serde(default)]
    pub total: u64,
}

/// One poll's worth of results. `health` failing is what makes the pane
/// degraded; the two list endpoints failing on their own only costs their
/// section, exactly as in the Bash version.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub health: Option<HealthResponse>,
    pub error: Option<String>,
    pub tracks: Option<TrackPage>,
    pub detections: Option<DetectionPage>,
}

/// How many rows each list section can currently show. Written by the
/// drawing code, read by the poller, so a tall terminal fetches a real track
/// list and a short one does not fetch rows it will throw away.
#[derive(Debug, Default)]
pub struct RowHints {
    pub tracks: AtomicUsize,
    pub detections: AtomicUsize,
}

impl RowHints {
    fn limit(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::Relaxed).clamp(1, MAX_ROWS)
    }
}

pub struct ClassgPane {
    pub base: String,
    pub snapshot: Snapshot,
    /// When the last *successful* health poll landed, so the pane can show
    /// how stale it is rather than freezing on old numbers.
    pub last_ok: Option<Instant>,
    pub polls: u64,
    pub hints: Arc<RowHints>,
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
    pub fn spawn(base: String, interval: Duration) -> Self {
        let (tx, rx) = channel();
        let hints = Arc::new(RowHints::default());
        let shutdown = Arc::new(AtomicBool::new(false));

        let worker_base = base.clone();
        let worker_hints = Arc::clone(&hints);
        let worker_shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            poll_loop(worker_base, interval, &tx, &worker_hints, &worker_shutdown);
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
    pub fn drain(&mut self) -> bool {
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

    pub fn set_hints(&self, tracks: usize, detections: usize) {
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
    base: String,
    interval: Duration,
    tx: &Sender<Snapshot>,
    hints: &RowHints,
    shutdown: &AtomicBool,
) {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .user_agent(concat!("pi-dash/", env!("CARGO_PKG_VERSION")))
        .build();

    while !shutdown.load(Ordering::Relaxed) {
        let snapshot = fetch(
            &agent,
            &base,
            RowHints::limit(&hints.tracks),
            RowHints::limit(&hints.detections),
        );
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

/// One complete poll. Public so `--once` can take a snapshot without starting
/// a thread or a terminal.
pub fn fetch(
    agent: &ureq::Agent,
    base: &str,
    track_rows: usize,
    detection_rows: usize,
) -> Snapshot {
    let health: Result<HealthResponse, String> = get_json(agent, base, "/api/v1/health");
    match health {
        Err(error) => Snapshot {
            error: Some(error),
            ..Default::default()
        },
        Ok(health) => Snapshot {
            health: Some(health),
            error: None,
            // The list endpoints are best-effort: a store that is mid-migration
            // should cost you the track list, not the sensor verdict above it,
            // which is the thing you cannot afford to lose.
            tracks: get_json(
                agent,
                base,
                &format!("/api/v1/tracks?limit={}", track_rows.clamp(1, MAX_ROWS)),
            )
            .ok(),
            detections: get_json(
                agent,
                base,
                &format!(
                    "/api/v1/detections?limit={}",
                    detection_rows.clamp(1, MAX_ROWS)
                ),
            )
            .ok(),
        },
    }
}

fn get_json<T: serde::de::DeserializeOwned>(
    agent: &ureq::Agent,
    base: &str,
    path: &str,
) -> Result<T, String> {
    // This build has no TLS: the API is on loopback by default and linking a
    // TLS stack to reach 127.0.0.1 is not a trade worth making on a Pi. Say so
    // plainly rather than failing with a confusing transport error.
    if base.starts_with("https://") {
        return Err(format!(
            "https is not supported by this build — point CLASSG_API at http:// ({base})"
        ));
    }
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let response = agent.get(&url).call().map_err(|err| match err {
        ureq::Error::Status(code, _) => format!("HTTP {code} from {path}"),
        ureq::Error::Transport(transport) => transport.to_string(),
    })?;
    let body = response
        .into_string()
        .map_err(|err| format!("could not read {path}: {err}"))?;
    serde_json::from_str(&body).map_err(|err| format!("bad JSON from {path}: {err}"))
}

/// Builds the agent the poller uses. Shared with `--once`.
pub fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .user_agent(concat!("pi-dash/", env!("CARGO_PKG_VERSION")))
        .build()
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
            #[allow(dead_code)]
            ts: FlexTime,
        }
        assert!(serde_json::from_str::<Wrapper>(r#"{"ts":"yesterday"}"#).is_err());
    }

    #[test]
    fn tracks_decode_with_partial_identities() {
        let json = r#"{
            "tracks": [
                {"track_id":"t1","state":"CONFIRMED","confidence":0.82,
                 "last_seen":"2026-08-16T12:00:00Z",
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
    fn detections_decode_with_a_missing_rf_block() {
        let json = r#"{
            "detections": [
                {"ts":1786012496.25,"sensor_kind":"wifi","detection_class":"A",
                 "rf":{"channel":6,"rssi_dbm":-52.0}},
                {"ts":"2026-08-16T12:00:00Z","sensor_kind":"sdr",
                 "detection_class":"E"}
            ],
            "total": 2
        }"#;
        let page: DetectionPage = serde_json::from_str(json).expect("decoded");
        assert_eq!(page.detections[0].rf.as_ref().unwrap().channel, Some(6));
        assert!(page.detections[1].rf.is_none());
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

    #[test]
    fn an_https_base_url_explains_itself_instead_of_failing_obscurely() {
        let agent = build_agent();
        let result: Result<HealthResponse, String> =
            get_json(&agent, "https://pi.local:8081", "/api/v1/health");
        let message = result.expect_err("https must be refused");
        assert!(message.contains("https is not supported"), "got {message}");
    }

    #[test]
    fn an_unreachable_api_returns_an_error_snapshot_not_a_panic() {
        let agent = build_agent();
        // Port 1 on loopback: reserved, nothing will ever be listening.
        let snapshot = fetch(&agent, "http://127.0.0.1:1", 10, 10);
        assert!(snapshot.health.is_none());
        assert!(snapshot.error.is_some());
        assert!(snapshot.tracks.is_none());
    }
}
