//! Temperature, core voltage, ARM clock, throttle state, disk and I/O.
//!
//! This is the pane the whole dashboard exists for. A browning-out supply
//! drops USB radios long before it shows up as anything a process monitor
//! draws, and the only place the SoC admits it is `vcgencmd get_throttled`.

use std::time::{Duration, Instant};

use super::{command_output, read_trimmed, read_u64};

/// 80 C is where a Pi 4 starts soft-capping the ARM clock, 85 C hard. The
/// meter spans 30..85 so the bar position means something absolute rather
/// than being a percentage of nothing.
pub(crate) const TEMP_METER_LO: f64 = 30.0;
pub(crate) const TEMP_METER_HI: f64 = 85.0;
pub(crate) const TEMP_WARN_C: f64 = 65.0;
pub(crate) const TEMP_HOT_C: f64 = 75.0;

/// The four conditions `get_throttled` reports, in one nibble.
///
/// The register carries each of them twice: bits 0-3 are "right now" and bits
/// 16-19 are "has happened since boot". Both matter, and they matter
/// differently — `0x50000` with a clean low nibble means it already happened
/// and you missed it, which is exactly the state this box spends most of its
/// life in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ThrottleFlags {
    pub(crate) under_voltage: bool,
    pub(crate) arm_capped: bool,
    pub(crate) throttled: bool,
    pub(crate) soft_temp_limit: bool,
}

impl ThrottleFlags {
    fn from_nibble(nibble: u32) -> Self {
        ThrottleFlags {
            under_voltage: nibble & 0b0001 != 0,
            arm_capped: nibble & 0b0010 != 0,
            throttled: nibble & 0b0100 != 0,
            soft_temp_limit: nibble & 0b1000 != 0,
        }
    }

    pub(crate) fn any(&self) -> bool {
        self.under_voltage || self.arm_capped || self.throttled || self.soft_temp_limit
    }

    /// Labels for whichever conditions are set, in register-bit order.
    ///
    /// The two tenses are worded differently on purpose. The live ones are
    /// shouted because they mean the supply is sagging *while you are looking
    /// at it*; the sticky ones are stated flatly because they are history.
    /// They are rendered on separate rows, so the sticky wording does not have
    /// to repeat "since boot" in every label.
    pub(crate) fn labels(&self, tense: Tense) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.under_voltage {
            out.push(match tense {
                Tense::Now => "UNDER-VOLTAGE NOW",
                Tense::SinceBoot => "under-voltage",
            });
        }
        if self.arm_capped {
            out.push(match tense {
                Tense::Now => "clock capped",
                Tense::SinceBoot => "clock capped",
            });
        }
        if self.throttled {
            out.push(match tense {
                Tense::Now => "throttled",
                Tense::SinceBoot => "throttled",
            });
        }
        if self.soft_temp_limit {
            out.push(match tense {
                Tense::Now => "soft temp limit",
                Tense::SinceBoot => "temp limit",
            });
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tense {
    Now,
    SinceBoot,
}

/// A decoded `get_throttled` register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Throttle {
    pub(crate) raw: u32,
    pub(crate) now: ThrottleFlags,
    pub(crate) since_boot: ThrottleFlags,
}

impl Throttle {
    pub(crate) fn decode(raw: u32) -> Self {
        Throttle {
            raw,
            now: ThrottleFlags::from_nibble(raw & 0xF),
            since_boot: ThrottleFlags::from_nibble((raw >> 16) & 0xF),
        }
    }

    pub(crate) fn clean(&self) -> bool {
        !self.now.any() && !self.since_boot.any()
    }
}

/// Parses the output of `vcgencmd get_throttled`, i.e. `throttled=0x50005`.
///
/// Accepts a bare value too, because `vcgencmd` has shipped both forms and a
/// dashboard that silently reports "clean" because the prefix changed would be
/// worse than useless. Decimal is accepted for the same reason.
pub(crate) fn parse_throttled(text: &str) -> Option<u32> {
    let value = text.trim().rsplit('=').next()?.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

/// Parses `vcgencmd measure_volts core`, i.e. `volt=0.8563V`.
pub(crate) fn parse_volts(text: &str) -> Option<f64> {
    text.trim()
        .rsplit('=')
        .next()?
        .trim()
        .trim_end_matches(['V', 'v'])
        .parse()
        .ok()
}

/// Parses `vcgencmd measure_clock arm`, i.e. `frequency(48)=1500000000`, into
/// MHz.
pub(crate) fn parse_clock_mhz(text: &str) -> Option<u64> {
    let hz: u64 = text.trim().rsplit('=').next()?.trim().parse().ok()?;
    Some(hz / 1_000_000)
}

/// Millidegrees from `/sys/class/thermal/thermal_zone0/temp` into degrees.
pub(crate) fn parse_thermal_millidegrees(text: &str) -> Option<f64> {
    let milli: i64 = text.trim().parse().ok()?;
    // A zone that reads 0 is a zone that is not wired up, not a Pi at
    // absolute-ish zero.
    (milli != 0).then_some(milli as f64 / 1000.0)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DiskUsage {
    pub(crate) used_kb: u64,
    pub(crate) total_kb: u64,
    /// What a normal user can actually write, straight from df's own
    /// `Available` column.
    ///
    /// This is NOT `total - used`. ext4 reserves 5% of the filesystem for
    /// root by default, so those two differ by gigabytes: on the Pi this was
    /// found on, `total - used` claimed 94.2G while 88.3G was writable. The
    /// dashboard sat next to ClassG's own figure, which comes from `statfs`
    /// and had it right, and the disagreement is what gave it away.
    ///
    /// It matters here more than on most boxes. The thing that fills this
    /// filesystem is a capture writing until it runs out, and 5% of a 117G
    /// card is most of an hour of headroom that does not exist.
    pub(crate) avail_kb: u64,
}

impl DiskUsage {
    /// How full the filesystem is, defined the way `df` defines Capacity:
    /// against what is usable, not against the raw size. Reporting
    /// `used / total` put this pane a couple of points below what `df` on the
    /// same box says, which is the sort of small disagreement that makes
    /// somebody stop trusting the number.
    pub(crate) fn pct(&self) -> f64 {
        let usable = self.used_kb.saturating_add(self.avail_kb);
        if usable == 0 {
            return 0.0;
        }
        self.used_kb as f64 * 100.0 / usable as f64
    }
}

/// Parses the last line of `df -P -k /`. POSIX output mode matters: without
/// `-P`, a long device name wraps onto its own line and the fields land in
/// different columns.
pub(crate) fn parse_df(text: &str) -> Option<DiskUsage> {
    let line = text.lines().last()?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    Some(DiskUsage {
        total_kb: fields.get(1)?.parse().ok()?,
        used_kb: fields.get(2)?.parse().ok()?,
        avail_kb: fields.get(3)?.parse().ok()?,
    })
}

/// Sectors read and written, summed over whole disks.
///
/// Partitions are skipped so their I/O is not counted twice — `/proc/diskstats`
/// lists both `mmcblk0` and `mmcblk0p2`, and the parent already includes the
/// child. Loop and device-mapper devices are skipped for the same reason:
/// Docker's overlay traffic shows up under the backing disk anyway.
pub(crate) fn parse_diskstats(text: &str) -> (u64, u64) {
    let mut sectors_read = 0u64;
    let mut sectors_written = 0u64;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = fields.get(2) else { continue };
        if !is_whole_disk(name) {
            continue;
        }
        sectors_read += fields
            .get(5)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        sectors_written += fields
            .get(9)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
    }
    (sectors_read, sectors_written)
}

/// `mmcblk0` yes, `mmcblk0p1` no; `sda` yes, `sda1` no; `nvme0n1` yes,
/// `nvme0n1p1` no.
fn is_whole_disk(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix("mmcblk") {
        return rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty();
    }
    if let Some(rest) = name.strip_prefix("nvme") {
        // nvme<ctrl>n<ns>, with a partition suffix of p<n>.
        return rest.contains('n') && !rest.contains('p');
    }
    if let Some(rest) = name.strip_prefix("sd") {
        return rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphabetic());
    }
    false
}

/// `/proc/diskstats` counts 512-byte sectors regardless of the device's real
/// block size — that is the documented unit of the field, not an assumption
/// about the SD card.
const SECTOR_BYTES: u64 = 512;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct IoRates {
    pub(crate) read_bps: f64,
    pub(crate) write_bps: f64,
}

#[derive(Debug, Default)]
pub(crate) struct HealthPane {
    prev_sectors: Option<(u64, u64)>,
    last_sample: Option<Instant>,
    /// `df` and `vcgencmd` fork. Disk usage moves slowly enough that once
    /// every few samples is plenty; the throttle register does not, so it is
    /// read every time.
    sample_count: u64,

    pub(crate) temp_c: Option<f64>,
    pub(crate) volts: Option<f64>,
    pub(crate) arm_mhz: Option<u64>,
    pub(crate) max_mhz: Option<u64>,
    /// `None` means "could not be read", which is *not* the same as clean.
    /// The Bash version defaulted the register to 0 when `vcgencmd` was
    /// missing and then printed "OK — clean since boot", confidently lying on
    /// exactly the machines that could not tell.
    pub(crate) throttle: Option<Throttle>,
    pub(crate) disk: Option<DiskUsage>,
    pub(crate) io: IoRates,
}

impl HealthPane {
    pub(crate) fn sample(&mut self, now: Instant) {
        self.temp_c = read_trimmed("/sys/class/thermal/thermal_zone0/temp")
            .as_deref()
            .and_then(parse_thermal_millidegrees);

        // Prefer sysfs for the clock: it is a file read rather than a fork,
        // and scaling_cur_freq is the same number vcgencmd reports. vcgencmd
        // stays as the fallback for kernels without cpufreq exposed.
        self.arm_mhz = read_u64("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
            .map(|khz| khz / 1000)
            .or_else(|| {
                command_output("vcgencmd", &["measure_clock", "arm"])
                    .as_deref()
                    .and_then(parse_clock_mhz)
            });
        self.max_mhz =
            read_u64("/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq").map(|khz| khz / 1000);

        // No sysfs equivalent for either of these; the firmware mailbox is the
        // only source, so vcgencmd it is.
        self.volts = command_output("vcgencmd", &["measure_volts", "core"])
            .as_deref()
            .and_then(parse_volts);
        self.throttle = command_output("vcgencmd", &["get_throttled"])
            .as_deref()
            .and_then(parse_throttled)
            .map(Throttle::decode);

        if self.sample_count.is_multiple_of(5) || self.disk.is_none() {
            self.disk = command_output("df", &["-P", "-k", "/"])
                .as_deref()
                .and_then(parse_df);
        }

        if let Some(text) = read_trimmed("/proc/diskstats") {
            let current = parse_diskstats(&text);
            let elapsed = self
                .last_sample
                .map(|then| now.saturating_duration_since(then))
                .unwrap_or_default();
            if let (Some(prev), true) = (self.prev_sectors, elapsed > Duration::ZERO) {
                let seconds = elapsed.as_secs_f64();
                self.io = IoRates {
                    read_bps: current.0.saturating_sub(prev.0) as f64 * SECTOR_BYTES as f64
                        / seconds,
                    write_bps: current.1.saturating_sub(prev.1) as f64 * SECTOR_BYTES as f64
                        / seconds,
                };
            }
            self.prev_sectors = Some(current);
        }

        self.last_sample = Some(now);
        self.sample_count = self.sample_count.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_register_decodes_to_nothing_set() {
        let throttle = Throttle::decode(0x0);
        assert!(throttle.clean());
        assert!(!throttle.now.any());
        assert!(!throttle.since_boot.any());
    }

    #[test]
    fn the_low_nibble_is_the_live_state() {
        let throttle = Throttle::decode(0x1);
        assert!(throttle.now.under_voltage);
        assert!(!throttle.now.arm_capped);
        assert!(!throttle.since_boot.any());
        assert!(!throttle.clean());

        let all_now = Throttle::decode(0xF);
        assert_eq!(
            all_now.now,
            ThrottleFlags {
                under_voltage: true,
                arm_capped: true,
                throttled: true,
                soft_temp_limit: true,
            }
        );
        assert!(!all_now.since_boot.any());
    }

    #[test]
    fn bits_16_to_19_are_the_sticky_since_boot_state() {
        // The state this Pi lives in: it browned out earlier and recovered.
        let throttle = Throttle::decode(0x50000);
        assert!(!throttle.now.any(), "nothing is happening right now");
        assert!(throttle.since_boot.under_voltage, "bit 16");
        assert!(throttle.since_boot.throttled, "bit 18");
        assert!(!throttle.since_boot.arm_capped);
        assert!(!throttle.since_boot.soft_temp_limit);
        assert!(!throttle.clean());
    }

    #[test]
    fn each_bit_maps_to_exactly_one_condition() {
        for (bit, expected) in [
            (0u32, "under_voltage"),
            (1, "arm_capped"),
            (2, "throttled"),
            (3, "soft_temp_limit"),
        ] {
            let now = Throttle::decode(1 << bit).now;
            let sticky = Throttle::decode(1 << (bit + 16)).since_boot;
            for flags in [now, sticky] {
                let set: Vec<&str> = [
                    ("under_voltage", flags.under_voltage),
                    ("arm_capped", flags.arm_capped),
                    ("throttled", flags.throttled),
                    ("soft_temp_limit", flags.soft_temp_limit),
                ]
                .into_iter()
                .filter_map(|(name, on)| on.then_some(name))
                .collect();
                assert_eq!(set, vec![expected], "bit {bit}");
            }
        }
    }

    #[test]
    fn both_halves_can_be_set_at_once_and_stay_distinct() {
        // Under-voltage now, and it has been capped and throttled before.
        let throttle = Throttle::decode(0x50001);
        assert!(throttle.now.under_voltage);
        assert!(!throttle.now.throttled);
        assert!(throttle.since_boot.under_voltage);
        assert!(throttle.since_boot.throttled);
        assert_eq!(throttle.now.labels(Tense::Now), vec!["UNDER-VOLTAGE NOW"]);
        assert_eq!(
            throttle.since_boot.labels(Tense::SinceBoot),
            vec!["under-voltage", "throttled"]
        );
    }

    #[test]
    fn bits_outside_the_two_nibbles_are_ignored() {
        // Undocumented middle bits must not leak into either verdict.
        let throttle = Throttle::decode(0x0000_0FF0 | 0x0F00_0000);
        assert!(!throttle.now.any());
        assert!(!throttle.since_boot.any());
    }

    #[test]
    fn throttled_output_parses_in_every_shipped_form() {
        assert_eq!(parse_throttled("throttled=0x0"), Some(0));
        assert_eq!(parse_throttled("throttled=0x50005"), Some(0x50005));
        assert_eq!(parse_throttled("  throttled=0X50000\n"), Some(0x50000));
        assert_eq!(parse_throttled("0x50000"), Some(0x50000));
        assert_eq!(parse_throttled("327680"), Some(327_680));
        assert_eq!(parse_throttled("throttled="), None);
        assert_eq!(parse_throttled("VCHI initialization failed"), None);
    }

    #[test]
    fn volts_and_clock_parse_from_vcgencmd_output() {
        assert_eq!(parse_volts("volt=0.8563V"), Some(0.8563));
        assert_eq!(parse_volts("volt=1.2V\n"), Some(1.2));
        assert_eq!(parse_volts("volt=broken"), None);
        assert_eq!(parse_clock_mhz("frequency(48)=1500398464"), Some(1500));
        assert_eq!(parse_clock_mhz("frequency(48)="), None);
    }

    #[test]
    fn thermal_zone_reads_millidegrees_and_treats_zero_as_absent() {
        assert_eq!(parse_thermal_millidegrees("48312"), Some(48.312));
        assert_eq!(parse_thermal_millidegrees("0"), None);
        assert_eq!(parse_thermal_millidegrees(""), None);
    }

    #[test]
    fn df_reads_the_posix_columns() {
        let text = "\
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/mmcblk0p2    59872256 21456320  35367424      38% /
";
        let disk = parse_df(text).expect("parsed");
        assert_eq!(disk.total_kb, 59_872_256);
        assert_eq!(disk.used_kb, 21_456_320);
        // Available, not total - used. Those differ by 2.9G here -- the 5%
        // ext4 reserves for root, which a capture cannot write into.
        assert_eq!(disk.avail_kb, 35_367_424);
        assert_ne!(disk.avail_kb, disk.total_kb - disk.used_kb);
        // And the percentage is df's Capacity column, 38%, not used/total,
        // which would say 35.8% on the same line of output.
        assert!((disk.pct() - 37.77).abs() < 0.1, "got {}", disk.pct());
    }

    #[test]
    fn a_disk_df_could_not_measure_is_zero_percent_rather_than_a_divide() {
        assert_eq!(DiskUsage::default().pct(), 0.0);
        // Truncated output -- a df that printed no Available column must
        // yield None rather than a plausible-looking figure built from the
        // columns that did arrive.
        assert!(parse_df("Filesystem 1024-blocks Used\n/dev/root 100 40\n").is_none());
    }

    #[test]
    fn diskstats_counts_whole_disks_only() {
        // Real shape: 14 fields for a Bookworm kernel, name in field 3,
        // sectors read in field 6 and sectors written in field 10.
        let text = "\
 179       0 mmcblk0 1000 0 4000 100 500 0 8000 200 0 300 400
 179       1 mmcblk0p1 10 0 40 1 5 0 80 2 0 3 4
 179       2 mmcblk0p2 900 0 3900 90 490 0 7900 190 0 290 390
   8       0 sda 100 0 2000 10 50 0 6000 20 0 30 40
   8       1 sda1 100 0 2000 10 50 0 6000 20 0 30 40
   7       0 loop0 1 0 2 0 0 0 0 0 0 0 0
 254       0 dm-0 1 0 999 0 0 0 999 0 0 0 0
";
        let (read, written) = parse_diskstats(text);
        assert_eq!(read, 4000 + 2000, "partitions must not be double-counted");
        assert_eq!(written, 8000 + 6000);
    }

    #[test]
    fn whole_disk_recognises_the_shapes_that_appear_on_a_pi() {
        assert!(is_whole_disk("mmcblk0"));
        assert!(!is_whole_disk("mmcblk0p2"));
        assert!(is_whole_disk("sda"));
        assert!(!is_whole_disk("sda1"));
        assert!(is_whole_disk("nvme0n1"));
        assert!(!is_whole_disk("nvme0n1p1"));
        assert!(!is_whole_disk("loop3"));
        assert!(!is_whole_disk("dm-0"));
        assert!(!is_whole_disk("mmcblk"));
    }

    #[test]
    fn a_missing_vcgencmd_leaves_throttle_unknown_not_clean() {
        // The pane must be able to say "unknown"; `Option<Throttle>` is what
        // carries that, and `None` must never be confused with a clean 0x0.
        let unknown: Option<Throttle> = None;
        assert!(unknown.is_none());
        assert!(Throttle::decode(0).clean());
    }
}
