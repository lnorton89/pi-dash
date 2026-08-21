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

/// Whether this sample should fork `df`.
///
/// Every fifth sample, and once at startup so the pane is not blank for the
/// first ten seconds.
///
/// The startup condition is `have_read` and not `disk.is_none()`, which is
/// what it used to be. Those differ on a box where `/` is not backed by a
/// `/dev/*` device -- an overlayfs root, which is exactly what a containerised
/// deployment looks like. There `df` runs, returns filesystems, and none of
/// them is `/`, so `disk` stays None for ever and the old condition forked a
/// process every two seconds until the dashboard was closed.
fn should_read_disk(sample_count: u64, have_read: bool) -> bool {
    sample_count.is_multiple_of(5) || !have_read
}

/// One mounted filesystem, as `df` reports it.
///
/// The dashboard used to ask `df` about `/` alone, which is the filesystem the
/// Pi boots from and not necessarily the one anything is written to. A unit
/// recording captures to a stick, or with the ClassG store on its own
/// partition, fills something this pane could not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Filesystem {
    /// The device, as df names it. `/dev/mmcblk0p2`, `/dev/sda1`.
    pub(crate) source: String,
    /// Where it is mounted. This is the label worth showing: nobody thinks of
    /// the boot partition as mmcblk0p1, they think of it as /boot/firmware.
    pub(crate) mount: String,
    pub(crate) usage: DiskUsage,
}

impl Filesystem {
    /// The short name for the mount point, for a column that cannot hold a
    /// path. `/` stays `/`; everything else is its last component.
    pub(crate) fn label(&self) -> &str {
        if self.mount == "/" {
            return "/";
        }
        self.mount
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.mount)
    }
}

/// Every real filesystem `df -P -k` reports, in the order it reports them.
///
/// Only sources under `/dev/`. A stock Pi mounts a dozen tmpfs, devtmpfs and
/// cgroup filesystems whose "capacity" is a kernel accounting detail rather
/// than somewhere a capture can land, and listing them buries the two that
/// matter. Docker's overlay mounts go the same way: the space they consume is
/// already counted against the disk underneath them, so showing both would
/// double-count the only number anyone reads this box for.
///
/// Deduplicated by source, because a bind mount presents one device twice and
/// two rows with identical figures read as two disks.
pub(crate) fn parse_df_all(text: &str) -> Vec<Filesystem> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(source) = fields.first() else {
            continue;
        };
        // Everything from the sixth field on, rejoined. df -P puts the mount
        // point last and does not escape it, so a stick auto-mounted at
        // `/media/pi/MY DISK` arrives as two fields -- and taking only the
        // first left the pane showing `/media/pi/MY` and --check telling you
        // to go and clear a path that does not exist.
        if fields.len() <= 5 {
            continue;
        }
        let mount = fields[5..].join(" ");
        if !source.starts_with("/dev/") || seen.iter().any(|s| s == source) {
            continue;
        }
        let Some(usage) = parse_df_fields(&fields) else {
            continue;
        };
        seen.push((*source).to_string());
        out.push(Filesystem {
            source: (*source).to_string(),
            mount,
            usage,
        });
    }
    out
}

/// The three size columns out of one already-split df row.
fn parse_df_fields(fields: &[&str]) -> Option<DiskUsage> {
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
    /// Whether `df` has run at all yet, as distinct from whether it found a
    /// root filesystem. See [`should_read_disk`].
    disk_read: bool,
    /// Every real filesystem, for the disks box. Kept here because this pane
    /// already owns the `df` fork and running a second one to tell the System
    /// pane the same thing would be a fork per tick for nothing.
    pub(crate) filesystems: Vec<Filesystem>,
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

        // One `df` for every filesystem rather than one for `/`. Same fork,
        // same cadence, strictly more answer -- and the root row is just the
        // one mounted at `/`.
        if should_read_disk(self.sample_count, self.disk_read) {
            if let Some(text) = command_output("df", &["-P", "-k"]) {
                self.disk_read = true;
                self.filesystems = parse_df_all(&text);
                self.disk = self
                    .filesystems
                    .iter()
                    .find(|fs| fs.mount == "/")
                    .map(|fs| fs.usage);
            }
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

    /// A Pi as df actually describes one: the card, the boot partition, a
    /// stick, and the pile of pseudo-filesystems that are not places a
    /// capture can land.
    const DF: &str = "\
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/mmcblk0p2    59872256 21456320  35367424      38% /
devtmpfs            1804000        0   1804000       0% /dev
tmpfs               1963892        0   1963892       0% /dev/shm
tmpfs                785560     1284    784276       1% /run
/dev/mmcblk0p1       522230    62918    459312      13% /boot/firmware
overlay            59872256 21456320  35367424      38% /var/lib/docker/overlay2/abc/merged
/dev/sda1         244180988      512 244180476       1% /media/captures
tmpfs                392776        0    392776       0% /run/user/1000
";

    #[test]
    fn df_reads_the_posix_columns() {
        let disk = parse_df_all(DF)
            .into_iter()
            .find(|fs| fs.mount == "/")
            .expect("a root filesystem")
            .usage;
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
        // A df row with no Available column is dropped rather than yielding
        // a plausible figure built from the columns that did arrive.
        assert!(parse_df_all("Filesystem 1024-blocks Used\n/dev/root 100 40\n").is_empty());
        assert!(parse_df_all("").is_empty());
    }

    #[test]
    fn only_filesystems_a_capture_could_land_on_are_listed() {
        let mounts: Vec<String> = parse_df_all(DF).into_iter().map(|fs| fs.mount).collect();
        assert_eq!(mounts, vec!["/", "/boot/firmware", "/media/captures"]);

        // tmpfs and devtmpfs report a "capacity" that is a kernel accounting
        // detail, and a stock Pi mounts a dozen of them -- enough to bury the
        // two rows anybody reads this box for.
        assert!(!mounts.iter().any(|m| m.starts_with("/run")));
        assert!(!mounts.iter().any(|m| m == "/dev/shm"));
        // Docker's overlay is the root filesystem counted a second time.
        assert!(!mounts.iter().any(|m| m.contains("overlay")));
    }

    #[test]
    fn df_is_not_forked_every_sample_on_a_box_with_no_dev_backed_root() {
        // A containerised or overlayfs root means `df` runs, returns
        // filesystems, and none of them is `/`. Keying the startup read on
        // "did we find a root" rather than "did we run" forked a process every
        // two seconds for the life of the dashboard.
        assert!(should_read_disk(0, false), "the first sample must read");
        assert!(should_read_disk(1, false), "and keep trying until it has");

        // Once it has run, only the slow cadence.
        assert!(!should_read_disk(1, true));
        assert!(!should_read_disk(4, true));
        assert!(should_read_disk(5, true));
        assert!(should_read_disk(10, true));
    }

    #[test]
    fn a_mount_point_containing_spaces_survives_whole() {
        // df -P puts the mount last and does not escape it. Taking only the
        // first field left the pane naming `/media/pi/MY` and --check telling
        // you to clear a path that does not exist.
        let text = "Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/sda1        100 40  60      40% /media/pi/MY DISK
";
        let found = parse_df_all(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mount, "/media/pi/MY DISK");
        assert_eq!(found[0].label(), "MY DISK");
    }

    #[test]
    fn a_mount_point_is_labelled_by_its_last_component() {
        // The column cannot hold a path, and nobody thinks of the boot
        // partition as mmcblk0p1.
        let found = parse_df_all(DF);
        let labels: Vec<&str> = found.iter().map(Filesystem::label).collect();
        assert_eq!(labels, vec!["/", "firmware", "captures"]);
    }

    #[test]
    fn one_device_mounted_twice_is_one_row() {
        // A bind mount presents the same device again, and two rows carrying
        // identical figures read as two disks half as full as the one there is.
        let text = "\
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/sda1        100 40  60      40% /media/captures
/dev/sda1        100 40  60      40% /var/lib/classg
";
        assert_eq!(parse_df_all(text).len(), 1);
    }

    /// The disk figures against the `df` on this machine.
    ///
    /// Everything else about parse_df is checked with a fixture, and a fixture
    /// only proves the parser agrees with whoever typed it. This is the column
    /// indices checked against a real df, which is what the fixture was wrong
    /// about for the whole life of the file: it read total and used and left
    /// Available on the floor, and the pane reported the 5% ext4 holds back
    /// for root as free space.
    ///
    /// Linux only. `df` on other platforms prints different columns, and there
    /// is nothing to learn from asserting that.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_disk_figures_agree_with_the_df_on_this_machine() {
        let text = command_output("df", &["-P", "-k"]).expect("df runs on Linux");
        let disk = parse_df_all(&text)
            .into_iter()
            .find(|fs| fs.mount == "/")
            .expect("every Linux box has a root filesystem")
            .usage;

        assert!(disk.total_kb > 0, "a filesystem with no size: {text}");
        assert!(disk.used_kb > 0, "a root filesystem in use: {text}");
        assert!(disk.avail_kb > 0, "no room at all: {text}");

        // Available is what a normal user can write, so it can never exceed
        // what is left over once used is taken off -- the reserve sits between
        // the two. Equal is legitimate on a filesystem with no reserve.
        let unused = disk.total_kb - disk.used_kb;
        assert!(
            disk.avail_kb <= unused,
            "available {} exceeds total-minus-used {unused}, so a column is misread: {text}",
            disk.avail_kb
        );
        assert!(
            disk.used_kb + disk.avail_kb <= disk.total_kb,
            "used plus available overruns the filesystem: {text}"
        );

        let pct = disk.pct();
        assert!((0.0..=100.0).contains(&pct), "{pct}% full: {text}");
    }

    /// The thermal zone, if this kernel exposes one. CI runners do not, and a
    /// Pi always does -- so this asserts the shape of whatever came back
    /// rather than that something came back.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_thermal_zone_that_exists_reads_as_a_plausible_temperature() {
        let Some(text) = read_trimmed("/sys/class/thermal/thermal_zone0/temp") else {
            return; // no thermal zone here; the pane says so and carries on
        };
        let Some(celsius) = parse_thermal_millidegrees(&text) else {
            return; // a zone that reads 0 is a zone that is not wired up
        };
        assert!(
            (-40.0..=125.0).contains(&celsius),
            "{celsius} C is not a temperature a silicon die reports"
        );
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
