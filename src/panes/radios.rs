//! Network interfaces and USB radios.
//!
//! Throughput comes from `/proc/net/dev` — one file read for every interface,
//! rather than two `/sys/class/net/*/statistics/*` reads each, which is what
//! the Bash version did. Link state and monitor mode still come from `/sys`
//! because there is no equivalent in `/proc/net/dev`.
//!
//! USB presence is read from `/sys/bus/usb/devices` instead of forking
//! `lsusb`, for the same reason the rest of this file avoids external tools:
//! `lsusb` is a fork per sample, and everything it prints for an unlabelled
//! adapter is already in sysfs. `lsusb` remains as a fallback for the odd
//! system where sysfs is not mounted the way we expect.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{command_output, read_trimmed};
use crate::config::is_ignored;

/// ARPHRD_IEEE80211_RADIOTAP. Reading `/sys/class/net/<if>/type` avoids a
/// dependency on `iw`, which is not installed on the target Pi, and reports
/// what the kernel actually has rather than what setup-monitor.sh last asked
/// for.
pub(crate) const ARPHRD_IEEE80211_RADIOTAP: u64 = 803;

/// Substrings in a USB device's manufacturer/product strings that mark it as
/// a radio even when its vendor ID is not on the list. Carried over from the
/// Bash version's regex; kept short because false positives here are cheap
/// and a missing adapter is not.
const RADIO_NAME_HINTS: [&str; 4] = ["mediatek", "rtl-sdr", "rtlsdr", "802.11"];

/// One row of `/proc/net/dev`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IfaceCounters {
    pub(crate) rx_bytes: u64,
    pub(crate) tx_bytes: u64,
}

/// Parses `/proc/net/dev`.
///
/// Split on the *first* colon, not on whitespace. The kernel pads the name
/// field to a fixed width, so once an interface has moved more than ~10 GB
/// the byte count runs straight into the colon (`eth0:12345678901`) and a
/// whitespace split silently loses the interface. Long interface names
/// (`enx00e04c680001`) do the same thing from the other side.
pub(crate) fn parse_net_dev(text: &str) -> Vec<(String, IfaceCounters)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name.contains('|') {
            continue; // the two header lines
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .map(|f| f.parse().unwrap_or(0))
            .collect();
        // rx: bytes packets errs drop fifo frame compressed multicast (8)
        // tx: bytes ... so transmitted bytes is field 9, index 8.
        let Some(&rx_bytes) = fields.first() else {
            continue;
        };
        let tx_bytes = fields.get(8).copied().unwrap_or(0);
        out.push((name.to_string(), IfaceCounters { rx_bytes, tx_bytes }));
    }
    out
}

/// Parses the channel out of `iw dev <if> info`.
pub(crate) fn parse_iw_channel(text: &str) -> Option<u32> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("channel "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WirelessMode {
    Monitor,
    Managed,
}

/// An interface as the pane displays it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Iface {
    pub(crate) name: String,
    /// `up`, `down`, `unknown`, … straight from `operstate`.
    pub(crate) state: String,
    pub(crate) rx_bps: f64,
    pub(crate) tx_bps: f64,
    /// `None` for a wired interface.
    pub(crate) mode: Option<WirelessMode>,
    pub(crate) channel: Option<u32>,
    /// Kernel module behind the interface — `mt7921u`, `brcmfmac`. This is
    /// the fact that tells two identical-looking `wlan*` entries apart, and
    /// the one you need when a monitor-mode capture is not producing frames:
    /// which of the adapters plugged into this Pi is which.
    pub(crate) driver: Option<String>,
}

/// A USB device the dashboard considers a radio.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct UsbRadio {
    /// `0bda:2838`.
    pub(crate) id: String,
    pub(crate) description: String,
}

/// True when a `vendor:product` ID and its description look like a radio.
/// Vendor IDs are compared case-insensitively because sysfs writes them
/// lowercase and `lsusb` has not always agreed.
pub(crate) fn is_radio(id: &str, description: &str, vendor_ids: &[impl AsRef<str>]) -> bool {
    let vendor = id.split(':').next().unwrap_or(id).to_ascii_lowercase();
    if vendor_ids.iter().any(|want| {
        want.as_ref()
            .trim_end_matches(':')
            .eq_ignore_ascii_case(&vendor)
    }) {
        return true;
    }
    let haystack = description.to_ascii_lowercase();
    RADIO_NAME_HINTS.iter().any(|hint| haystack.contains(hint))
}

/// Parses one `lsusb` line:
/// `Bus 001 Device 004: ID 0bda:2838 Realtek Semiconductor Corp. RTL2838`.
pub(crate) fn parse_lsusb_line(line: &str) -> Option<UsbRadio> {
    let after_id = line.split(" ID ").nth(1)?;
    let mut parts = after_id.splitn(2, char::is_whitespace);
    let id = parts.next()?.trim().to_string();
    if !id.contains(':') {
        return None;
    }
    Some(UsbRadio {
        id,
        description: parts.next().unwrap_or("").trim().to_string(),
    })
}

/// Enumerates USB devices from sysfs. Entries without an `idVendor` are
/// interfaces rather than devices and are skipped, which is also what keeps
/// a single adapter from being listed once per endpoint.
fn read_usb_from_sysfs() -> Option<Vec<UsbRadio>> {
    let entries = std::fs::read_dir("/sys/bus/usb/devices").ok()?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(vendor), Some(product)) = (
            read_trimmed(path.join("idVendor")),
            read_trimmed(path.join("idProduct")),
        ) else {
            continue;
        };
        let manufacturer = read_trimmed(path.join("manufacturer")).unwrap_or_default();
        let name = read_trimmed(path.join("product")).unwrap_or_default();
        let description = [manufacturer, name]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        out.push(UsbRadio {
            id: format!("{vendor}:{product}"),
            description,
        });
    }
    out.sort();
    Some(out)
}

fn read_usb_from_lsusb() -> Vec<UsbRadio> {
    command_output("lsusb", &[])
        .map(|text| text.lines().filter_map(parse_lsusb_line).collect())
        .unwrap_or_default()
}

#[derive(Debug, Default)]
pub(crate) struct RadiosPane {
    prev: HashMap<String, IfaceCounters>,
    last_sample: Option<Instant>,
    sample_count: u64,
    usb_cache: Vec<UsbRadio>,
    channel_cache: HashMap<String, u32>,

    pub(crate) ifaces: Vec<Iface>,
    pub(crate) usb: Vec<UsbRadio>,
}

/// The driver behind an interface, from the `device/driver` symlink in sysfs.
///
/// A symlink read, not a `readlink` fork: the target is a path into
/// `/sys/bus/.../drivers/<name>` and only its last component is wanted. Absent
/// for virtual interfaces, which have no backing device at all.
fn read_driver(sys: &std::path::Path) -> Option<String> {
    let target = std::fs::read_link(sys.join("device/driver")).ok()?;
    Some(target.file_name()?.to_string_lossy().to_string())
}

impl RadiosPane {
    pub(crate) fn sample(&mut self, now: Instant, vendor_ids: &[String], ignore: &[String]) {
        let elapsed = self
            .last_sample
            .map(|then| now.saturating_duration_since(then))
            .unwrap_or_default();
        self.last_sample = Some(now);

        let counters = read_trimmed("/proc/net/dev")
            .map(|text| parse_net_dev(&text))
            .unwrap_or_default();

        let mut ifaces = Vec::new();
        for (name, current) in &counters {
            if is_ignored(name, ignore) {
                continue;
            }
            let (rx_bps, tx_bps) = match self.prev.get(name) {
                Some(prev) => (
                    rate(current.rx_bytes, prev.rx_bytes, elapsed),
                    rate(current.tx_bytes, prev.tx_bytes, elapsed),
                ),
                None => (0.0, 0.0),
            };

            let sys = std::path::Path::new("/sys/class/net").join(name);
            let state = read_trimmed(sys.join("operstate")).unwrap_or_else(|| "?".to_string());
            let wireless = sys.join("phy80211").exists() || sys.join("wireless").exists();
            let mode = wireless.then(|| {
                match read_trimmed(sys.join("type")).and_then(|t| t.parse::<u64>().ok()) {
                    Some(ARPHRD_IEEE80211_RADIOTAP) => WirelessMode::Monitor,
                    _ => WirelessMode::Managed,
                }
            });

            ifaces.push(Iface {
                name: name.clone(),
                state,
                rx_bps,
                tx_bps,
                mode,
                channel: self.channel_cache.get(name).copied(),
                driver: read_driver(&sys),
            });
        }
        self.prev = counters.into_iter().collect();
        self.ifaces = ifaces;

        // `iw` forks once per wireless interface and the channel only changes
        // when the hopper moves it, so refresh it on the same slow cadence as
        // the USB scan. It is optional: the Pi this targets does not have it
        // installed, and the pane reads fine without a channel column.
        if self.sample_count.is_multiple_of(5) {
            self.refresh_channels();
            // Re-apply what the refresh just learned. The rows above were
            // built from the cache as it stood a moment ago, so without this
            // every channel arrives one tick late — which on the very first
            // sample means the column is empty for the first two seconds of
            // every run, and after a hopper moves it means the pane shows the
            // old channel once more before catching up.
            for iface in self.ifaces.iter_mut() {
                iface.channel = self.channel_cache.get(&iface.name).copied();
            }
            self.usb_cache = read_usb_from_sysfs().unwrap_or_else(read_usb_from_lsusb);
        }
        self.usb = self
            .usb_cache
            .iter()
            .filter(|d| is_radio(&d.id, &d.description, vendor_ids))
            .cloned()
            .collect();

        self.sample_count = self.sample_count.wrapping_add(1);
    }

    fn refresh_channels(&mut self) {
        self.channel_cache.clear();
        for iface in self.ifaces.iter().filter(|i| i.mode.is_some()) {
            if let Some(channel) = command_output("iw", &["dev", &iface.name, "info"])
                .as_deref()
                .and_then(parse_iw_channel)
            {
                self.channel_cache.insert(iface.name.clone(), channel);
            }
        }
    }

    /// How many rows the pane needs, so the layout can pin it.
    ///
    /// Each table is a heading plus its rows, and an empty one collapses to
    /// the single line that says so — the "none present" warning takes a row
    /// exactly like a device would. Between them sit a blank line and the USB
    /// section title. Under-counting here does not scroll the pane, it clips
    /// it, and the row that goes is the one saying the adapters are gone.
    pub(crate) fn content_rows(&self) -> u16 {
        let table = |rows: usize| if rows == 0 { 1 } else { rows + 1 };
        (table(self.ifaces.len()) + 2 + table(self.usb.len())) as u16
    }
}

/// Turns a counter delta into a throughput.
///
/// An adapter that is re-plugged comes back with its counters at zero, so the
/// delta goes negative for exactly one sample; `saturating_sub` reports that
/// as 0 B/s rather than as a rate of several exabytes.
pub(crate) fn rate(current: u64, previous: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }
    current.saturating_sub(previous) as f64 / seconds
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:   12345      99    0    0    0     0          0         0    12345      99    0    0    0     0       0          0
  eth0: 987654    4321    0    2    0     0          0        11   123456     2222    0    0    0     0       0          0
 wlan1:       0       0    0    0    0     0          0         0        0        0    0    0    0     0       0          0
";

    #[test]
    fn net_dev_reads_rx_and_tx_bytes() {
        let parsed = parse_net_dev(NET_DEV);
        assert_eq!(parsed.len(), 3, "headers must not be parsed as interfaces");
        assert_eq!(parsed[0].0, "lo");
        assert_eq!(parsed[1].0, "eth0");
        assert_eq!(parsed[1].1.rx_bytes, 987_654);
        assert_eq!(parsed[1].1.tx_bytes, 123_456);
        assert_eq!(parsed[2].1, IfaceCounters::default());
    }

    #[test]
    fn a_byte_count_that_runs_into_the_colon_still_parses() {
        // What ~11 GB of capture traffic on wlan1 actually looks like: the
        // kernel's fixed-width name field is overrun and the count abuts the
        // colon. Splitting on whitespace loses this interface entirely.
        let text = "wlan1:11529215046068469 8 0 0 0 0 0 0 4096 8 0 0 0 0 0 0\n";
        let parsed = parse_net_dev(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "wlan1");
        assert_eq!(parsed[0].1.rx_bytes, 11_529_215_046_068_469);
        assert_eq!(parsed[0].1.tx_bytes, 4096);
    }

    #[test]
    fn a_long_interface_name_still_parses() {
        let text = " enx00e04c680001: 4096 1 0 0 0 0 0 0 8192 2 0 0 0 0 0 0\n";
        let parsed = parse_net_dev(text);
        assert_eq!(parsed[0].0, "enx00e04c680001");
        assert_eq!(parsed[0].1.tx_bytes, 8192);
    }

    #[test]
    fn a_truncated_line_does_not_panic_or_invent_a_tx_figure() {
        let parsed = parse_net_dev("eth0: 100 2 0\n");
        assert_eq!(parsed[0].1.rx_bytes, 100);
        assert_eq!(parsed[0].1.tx_bytes, 0);
        assert!(parse_net_dev("").is_empty());
        assert!(parse_net_dev("no colon here\n").is_empty());
    }

    #[test]
    fn rates_use_measured_elapsed_time_and_clamp_a_counter_reset() {
        assert_eq!(rate(3072, 1024, Duration::from_secs(2)), 1024.0);
        // Adapter re-plugged: the counter restarted below the previous value.
        assert_eq!(rate(10, 1_000_000, Duration::from_secs(2)), 0.0);
        // The very first sample has no interval at all.
        assert_eq!(rate(3072, 1024, Duration::ZERO), 0.0);
    }

    #[test]
    fn the_pi_s_actual_adapters_are_recognised_by_vendor_id() {
        let ids = crate::config::DEFAULT_USB_VENDOR_IDS.map(str::to_string);
        assert!(is_radio("0e8d:7961", "MediaTek Inc. Wireless_Device", &ids));
        assert!(is_radio("0bda:2838", "Realtek RTL2838 DVB-T", &ids));
        assert!(is_radio("148f:5370", "Ralink RT5370", &ids));
        assert!(is_radio("2357:0109", "TP-Link TL-WN823N", &ids));
        assert!(is_radio("0cf3:9271", "Qualcomm Atheros AR9271", &ids));
        assert!(is_radio("1d50:6089", "OpenMoko HackRF One", &ids));
    }

    #[test]
    fn vendor_matching_is_case_insensitive_and_tolerates_a_trailing_colon() {
        // The Bash list was written as `0e8d:` and someone will paste that in.
        let ids = ["0E8D:".to_string()];
        assert!(is_radio("0e8d:7961", "", &ids));
        assert!(is_radio("0E8D:7961", "", &ids));
    }

    #[test]
    fn everyday_usb_devices_are_not_radios() {
        let ids = crate::config::DEFAULT_USB_VENDOR_IDS.map(str::to_string);
        assert!(!is_radio(
            "1d6b:0002",
            "Linux Foundation 2.0 root hub",
            &ids
        ));
        assert!(!is_radio("046d:c52b", "Logitech Unifying Receiver", &ids));
        assert!(!is_radio("05e3:0610", "Genesys Logic Hub", &ids));
    }

    #[test]
    fn an_unlisted_vendor_still_matches_on_its_product_string() {
        let ids: [String; 0] = [];
        assert!(is_radio("ffff:0001", "Nooelec NESDR RTL-SDR", &ids));
        assert!(is_radio("ffff:0002", "Generic 802.11 adapter", &ids));
        assert!(!is_radio("ffff:0003", "Some Vendor Webcam", &ids));
    }

    #[test]
    fn lsusb_lines_split_on_the_id_marker() {
        let device = parse_lsusb_line(
            "Bus 001 Device 004: ID 0bda:2838 Realtek Semiconductor Corp. RTL2838 DVB-T",
        )
        .expect("parsed");
        assert_eq!(device.id, "0bda:2838");
        assert_eq!(
            device.description,
            "Realtek Semiconductor Corp. RTL2838 DVB-T"
        );

        // A device with no description string at all.
        let bare = parse_lsusb_line("Bus 001 Device 002: ID 0e8d:7961").expect("parsed");
        assert_eq!(bare.id, "0e8d:7961");
        assert_eq!(bare.description, "");

        assert!(parse_lsusb_line("").is_none());
        assert!(parse_lsusb_line("Bus 001 Device 002: ID nonsense").is_none());
    }

    #[test]
    fn iw_channel_is_read_from_the_info_block() {
        let text = "\
Interface wlan1
\tifindex 4
\twdev 0x2
\ttype monitor
\tchannel 6 (2437 MHz), width: 20 MHz, center1: 2437 MHz
\ttxpower 20.00 dBm
";
        assert_eq!(parse_iw_channel(text), Some(6));
        assert_eq!(parse_iw_channel("Interface wlan0\n\ttype managed\n"), None);
    }
}
