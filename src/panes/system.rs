//! CPU, memory and processes — the pane that replaces btop.
//!
//! Dropping the btop dependency is the point of the rewrite: it is another
//! `apt install` on a box that may be freshly imaged, it owns a whole tmux
//! pane, and everything it shows that matters here comes out of three files.
//! What it is *not* is a reimplementation of btop — no graphs, no tree view,
//! no process management. A CPU meter per core, the memory split, and the top
//! consumers is the summary you actually read when something is wrong.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{page_size, read_trimmed};

/// `/proc` reports CPU time in USER_HZ, which Linux fixes at 100 for the
/// procfs ABI regardless of the kernel's internal CONFIG_HZ. The aggregate
/// percentages cancel it out; the per-process ones do not, so it is named.
const USER_HZ: f64 = 100.0;

/// One row of `/proc/stat`'s `cpu`/`cpuN` lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    /// Idle *and* iowait. A Pi waiting on a slow SD card is not busy, and
    /// counting iowait as load makes every write burst look like a CPU spike.
    pub fn idle_all(&self) -> u64 {
        self.idle + self.iowait
    }

    /// Busy percentage over the interval between `prev` and `self`.
    /// `None` when there is no usable delta — the first sample, or a counter
    /// that went backwards because the sampler was restarted.
    pub fn usage_since(&self, prev: &CpuTimes) -> Option<f64> {
        let total = self.total().checked_sub(prev.total())?;
        let idle = self.idle_all().saturating_sub(prev.idle_all());
        if total == 0 {
            return None;
        }
        Some((100.0 - (idle as f64 * 100.0 / total as f64)).clamp(0.0, 100.0))
    }
}

/// Parses one `cpu`/`cpuN` line. Fields past `steal` (guest, guest_nice) are
/// ignored: guest time is already counted inside `user`, so adding it would
/// double-count and make totals drift.
pub fn parse_cpu_line(line: &str) -> Option<(String, CpuTimes)> {
    let mut fields = line.split_whitespace();
    let label = fields.next()?;
    if !label.starts_with("cpu") {
        return None;
    }
    let nums: Vec<u64> = fields.filter_map(|f| f.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    let at = |i: usize| nums.get(i).copied().unwrap_or(0);
    Some((
        label.to_string(),
        CpuTimes {
            user: at(0),
            nice: at(1),
            system: at(2),
            idle: at(3),
            iowait: at(4),
            irq: at(5),
            softirq: at(6),
            steal: at(7),
        },
    ))
}

/// Splits `/proc/stat` into the aggregate line and the per-core lines, in
/// core order.
pub fn parse_stat(text: &str) -> (Option<CpuTimes>, Vec<CpuTimes>) {
    let mut aggregate = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        let Some((label, times)) = parse_cpu_line(line) else {
            continue;
        };
        if label == "cpu" {
            aggregate = Some(times);
        } else {
            cores.push(times);
        }
    }
    (aggregate, cores)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub total_kb: u64,
    pub available_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
}

impl MemInfo {
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }

    pub fn used_pct(&self) -> f64 {
        if self.total_kb == 0 {
            return 0.0;
        }
        self.used_kb() as f64 * 100.0 / self.total_kb as f64
    }

    pub fn swap_used_kb(&self) -> u64 {
        self.swap_total_kb.saturating_sub(self.swap_free_kb)
    }
}

/// Parses `/proc/meminfo`. Uses MemAvailable, not MemFree: on a Pi running
/// Docker the page cache legitimately eats everything free, and MemFree would
/// have this pane permanently claiming the box is out of memory.
pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut mem = MemInfo::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next().and_then(|v| v.parse().ok()) else {
            continue;
        };
        match key {
            "MemTotal" => mem.total_kb = value,
            "MemAvailable" => mem.available_kb = value,
            "SwapTotal" => mem.swap_total_kb = value,
            "SwapFree" => mem.swap_free_kb = value,
            "Buffers" => mem.buffers_kb = value,
            "Cached" => mem.cached_kb = value,
            _ => {}
        }
    }
    mem
}

/// `(load1, load5, load15, runnable, total_tasks)` from `/proc/loadavg`.
pub fn parse_loadavg(text: &str) -> Option<([f64; 3], u64, u64)> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let load = [
        fields.first()?.parse().ok()?,
        fields.get(1)?.parse().ok()?,
        fields.get(2)?.parse().ok()?,
    ];
    let (runnable, total) = fields
        .get(3)
        .and_then(|f| f.split_once('/'))
        .and_then(|(r, t)| Some((r.parse().ok()?, t.parse().ok()?)))
        .unwrap_or((0, 0));
    Some((load, runnable, total))
}

/// Seconds since boot from `/proc/uptime`.
pub fn parse_uptime(text: &str) -> Option<u64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|s| s as u64)
}

/// The fields of `/proc/<pid>/stat` this pane uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidStat {
    pub pid: i32,
    pub comm: String,
    pub state: char,
    /// utime + stime, in USER_HZ.
    pub cpu_ticks: u64,
    pub rss_pages: u64,
}

/// Parses one `/proc/<pid>/stat`.
///
/// Split on the *last* `)`, never on whitespace: the comm field is the raw
/// executable name in parentheses and may itself contain spaces and
/// parentheses (`(Web Content)`, `((sd-pam))`). Every naive field-index parse
/// of this file is wrong for exactly those processes, which on a desktop is
/// most of them.
pub fn parse_pid_stat(text: &str) -> Option<PidStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = text[..open].trim().parse().ok()?;
    let comm = text[open + 1..close].to_string();

    // After the closing paren the fields are 1-indexed from 3, so field N is
    // at index N-3: state 3, utime 14, stime 15, rss 24.
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let state = rest.first()?.chars().next()?;
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    let rss_pages: u64 = rest.get(21).and_then(|v| v.parse().ok()).unwrap_or(0);

    Some(PidStat {
        pid,
        comm,
        state,
        cpu_ticks: utime + stime,
        rss_pages,
    })
}

/// One row of the process table.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcRow {
    pub pid: i32,
    pub name: String,
    pub state: char,
    /// Percent of *one* core, as `top` reports it — a four-thread build can
    /// legitimately read 380% on a Pi 4.
    pub cpu_pct: f64,
    pub rss_kb: u64,
}

/// Turns two `/proc/<pid>/stat` samples into a sorted process table.
/// Processes that did not exist in `prev` are reported at 0% rather than
/// credited with their whole lifetime's CPU in one interval — that is what
/// makes a freshly forked `apt` briefly appear to be using 4000%.
pub fn process_rows(
    current: &[PidStat],
    prev: &HashMap<i32, u64>,
    elapsed: Duration,
    page_bytes: u64,
) -> Vec<ProcRow> {
    let seconds = elapsed.as_secs_f64();
    let mut rows: Vec<ProcRow> = current
        .iter()
        .map(|stat| {
            let cpu_pct = if seconds <= 0.0 {
                0.0
            } else {
                prev.get(&stat.pid)
                    .map(|before| {
                        let delta = stat.cpu_ticks.saturating_sub(*before) as f64;
                        delta / USER_HZ / seconds * 100.0
                    })
                    .unwrap_or(0.0)
            };
            ProcRow {
                pid: stat.pid,
                name: stat.comm.clone(),
                state: stat.state,
                cpu_pct,
                rss_kb: stat.rss_pages.saturating_mul(page_bytes) / 1024,
            }
        })
        .collect();
    // Busiest first, then largest, then by pid so the order is stable between
    // frames when everything is idle — a table that reshuffles every two
    // seconds is unreadable.
    rows.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.rss_kb.cmp(&a.rss_kb))
            .then(a.pid.cmp(&b.pid))
    });
    rows
}

/// State of the system pane between samples.
#[derive(Debug)]
pub struct SystemPane {
    page_bytes: u64,
    prev_aggregate: Option<CpuTimes>,
    prev_cores: Vec<CpuTimes>,
    prev_proc_ticks: HashMap<i32, u64>,
    last_sample: Option<Instant>,

    pub cpu_pct: Option<f64>,
    pub core_pct: Vec<Option<f64>>,
    pub mem: MemInfo,
    pub load: [f64; 3],
    pub runnable: u64,
    pub task_count: u64,
    pub uptime_secs: u64,
    pub procs: Vec<ProcRow>,
    /// Set when `/proc` is not readable at all, i.e. this is not Linux.
    pub unavailable: Option<String>,
}

impl Default for SystemPane {
    fn default() -> Self {
        SystemPane {
            page_bytes: page_size(),
            prev_aggregate: None,
            prev_cores: Vec::new(),
            prev_proc_ticks: HashMap::new(),
            last_sample: None,
            cpu_pct: None,
            core_pct: Vec::new(),
            mem: MemInfo::default(),
            load: [0.0; 3],
            runnable: 0,
            task_count: 0,
            uptime_secs: 0,
            procs: Vec::new(),
            unavailable: None,
        }
    }
}

impl SystemPane {
    pub fn sample(&mut self, now: Instant) {
        let Some(stat) = read_trimmed("/proc/stat") else {
            self.unavailable = Some("/proc/stat is not readable — not a Linux box?".to_string());
            return;
        };
        self.unavailable = None;

        let (aggregate, cores) = parse_stat(&stat);
        if let (Some(current), Some(prev)) = (aggregate, self.prev_aggregate) {
            self.cpu_pct = current.usage_since(&prev);
        }
        self.core_pct = cores
            .iter()
            .enumerate()
            .map(|(i, current)| {
                self.prev_cores
                    .get(i)
                    .and_then(|prev| current.usage_since(prev))
            })
            .collect();
        self.prev_aggregate = aggregate;
        self.prev_cores = cores;

        if let Some(text) = read_trimmed("/proc/meminfo") {
            self.mem = parse_meminfo(&text);
        }
        if let Some((load, runnable, total)) = read_trimmed("/proc/loadavg")
            .as_deref()
            .and_then(parse_loadavg)
        {
            self.load = load;
            self.runnable = runnable;
            self.task_count = total;
        }
        if let Some(secs) = read_trimmed("/proc/uptime")
            .as_deref()
            .and_then(parse_uptime)
        {
            self.uptime_secs = secs;
        }

        // Measure the interval that actually elapsed rather than assuming the
        // configured one. The Bash dashboard divided by $INTERVAL, but a tick
        // there also forked awk four times, `df`, and `vcgencmd` three times,
        // so the real period was always longer and every rate it printed was
        // overstated by however long the work took.
        let elapsed = self
            .last_sample
            .map(|then| now.saturating_duration_since(then))
            .unwrap_or_default();
        self.last_sample = Some(now);

        let stats = read_process_stats();
        if elapsed > Duration::ZERO {
            self.procs = process_rows(&stats, &self.prev_proc_ticks, elapsed, self.page_bytes);
        }
        self.prev_proc_ticks = stats.iter().map(|s| (s.pid, s.cpu_ticks)).collect();
    }
}

/// Walks `/proc` once per sample. Reading a few hundred small files costs
/// under a millisecond of wall time on a Pi 4 and avoids a `ps` fork, which
/// is what the rest of this dashboard is built to avoid.
fn read_process_stats() -> Vec<PidStat> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            // Numeric directories only; /proc also holds `self`, `net`, etc.
            if !name.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            // A process can exit between the readdir and the read. That is
            // normal, not an error worth reporting.
            let text = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            parse_pid_stat(&text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "\
cpu  100 20 50 800 30 0 5 0 0 0
cpu0 50 10 25 400 15 0 2 0 0 0
cpu1 50 10 25 400 15 0 3 0 0 0
intr 12345 1 2 3
ctxt 999
";

    #[test]
    fn stat_splits_into_aggregate_and_cores() {
        let (aggregate, cores) = parse_stat(STAT);
        let aggregate = aggregate.expect("aggregate cpu line");
        assert_eq!(aggregate.user, 100);
        assert_eq!(aggregate.steal, 0);
        assert_eq!(aggregate.total(), 1005, "100+20+50+800+30+0+5+0");
        assert_eq!(cores.len(), 2);
        assert_eq!(cores[1].softirq, 3);
    }

    #[test]
    fn guest_columns_are_not_double_counted() {
        // Real /proc/stat lines end with guest and guest_nice, which the
        // kernel has already added into user and nice.
        let (aggregate, _) = parse_stat("cpu  1 2 3 4 5 6 7 8 900 901\n");
        assert_eq!(
            aggregate.expect("aggregate").total(),
            1 + 2 + 3 + 4 + 5 + 6 + 7 + 8
        );
    }

    #[test]
    fn cpu_usage_is_the_non_idle_share_of_the_delta() {
        let before = CpuTimes {
            user: 100,
            idle: 900,
            ..Default::default()
        };
        // 200 more ticks total, 50 of them busy.
        let after = CpuTimes {
            user: 150,
            idle: 1050,
            ..Default::default()
        };
        let usage = after.usage_since(&before).expect("a usable delta");
        assert!((usage - 25.0).abs() < 1e-9, "got {usage}");
    }

    #[test]
    fn iowait_counts_as_idle() {
        let before = CpuTimes::default();
        let after = CpuTimes {
            iowait: 100,
            idle: 100,
            ..Default::default()
        };
        assert_eq!(after.usage_since(&before), Some(0.0));
    }

    #[test]
    fn a_stalled_or_rewound_counter_reports_nothing_rather_than_a_wild_number() {
        let before = CpuTimes {
            user: 100,
            idle: 900,
            ..Default::default()
        };
        assert_eq!(before.usage_since(&before), None, "zero delta");
        let rewound = CpuTimes {
            user: 1,
            idle: 1,
            ..Default::default()
        };
        assert_eq!(
            rewound.usage_since(&before),
            None,
            "counters went backwards"
        );
    }

    #[test]
    fn meminfo_prefers_available_over_free() {
        let text = "\
MemTotal:        8000000 kB
MemFree:          100000 kB
MemAvailable:    6000000 kB
Buffers:          200000 kB
Cached:          3000000 kB
SwapTotal:        512000 kB
SwapFree:         512000 kB
";
        let mem = parse_meminfo(text);
        assert_eq!(mem.total_kb, 8_000_000);
        assert_eq!(mem.available_kb, 6_000_000);
        assert_eq!(mem.used_kb(), 2_000_000);
        assert_eq!(mem.swap_used_kb(), 0);
        assert!((mem.used_pct() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn meminfo_without_swap_does_not_divide_by_zero() {
        let mem = parse_meminfo("MemTotal: 0 kB\n");
        assert_eq!(mem.used_pct(), 0.0);
        assert_eq!(mem.swap_used_kb(), 0);
    }

    #[test]
    fn loadavg_carries_the_runnable_over_total_field() {
        let (load, runnable, total) = parse_loadavg("0.52 0.31 0.20 2/431 8899").expect("loadavg");
        assert!((load[0] - 0.52).abs() < 1e-9);
        assert!((load[2] - 0.20).abs() < 1e-9);
        assert_eq!((runnable, total), (2, 431));
    }

    #[test]
    fn uptime_takes_the_first_field() {
        assert_eq!(parse_uptime("90061.42 355000.11"), Some(90_061));
        assert_eq!(parse_uptime(""), None);
    }

    #[test]
    fn pid_stat_survives_a_comm_containing_spaces_and_parentheses() {
        // fields:      1  2               3 4 5 6 7 8 9 0 1 2 3(utime) ...
        let text = "1234 (Web Content (x)) S 1 1 0 0 -1 4194304 100 0 0 0 \
                    700 300 0 0 20 0 5 0 99 123456789 6789 0 0 0 0 0 0";
        let stat = parse_pid_stat(text).expect("parsed");
        assert_eq!(stat.pid, 1234);
        assert_eq!(stat.comm, "Web Content (x)");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.cpu_ticks, 1000);
        assert_eq!(stat.rss_pages, 6789);
    }

    #[test]
    fn pid_stat_rejects_junk_without_panicking() {
        assert!(parse_pid_stat("").is_none());
        assert!(parse_pid_stat("not a stat line").is_none());
        assert!(parse_pid_stat("1234 (short) S 1").is_none());
    }

    fn stat_of(pid: i32, comm: &str, ticks: u64, rss_pages: u64) -> PidStat {
        PidStat {
            pid,
            comm: comm.to_string(),
            state: 'S',
            cpu_ticks: ticks,
            rss_pages,
        }
    }

    #[test]
    fn process_cpu_is_a_delta_over_measured_elapsed_time() {
        let prev: HashMap<i32, u64> = [(1, 100), (2, 500)].into_iter().collect();
        let current = vec![
            stat_of(1, "classg-api", 300, 1000),
            stat_of(2, "idle-thing", 500, 10),
        ];
        let rows = process_rows(&current, &prev, Duration::from_secs(2), 4096);

        // 200 ticks = 2.00 s of CPU over a 2 s window = 100% of one core.
        assert_eq!(rows[0].pid, 1);
        assert!(
            (rows[0].cpu_pct - 100.0).abs() < 1e-9,
            "got {}",
            rows[0].cpu_pct
        );
        assert_eq!(rows[0].rss_kb, 4000);
        assert_eq!(rows[1].cpu_pct, 0.0);
    }

    #[test]
    fn a_brand_new_process_starts_at_zero_not_at_its_whole_lifetime() {
        let prev = HashMap::new();
        let current = vec![stat_of(9, "apt", 99_999, 100)];
        let rows = process_rows(&current, &prev, Duration::from_secs(2), 4096);
        assert_eq!(rows[0].cpu_pct, 0.0);
    }

    #[test]
    fn idle_processes_keep_a_stable_order_between_frames() {
        let prev: HashMap<i32, u64> = [(7, 0), (3, 0), (5, 0)].into_iter().collect();
        let current = vec![
            stat_of(7, "g", 0, 10),
            stat_of(3, "a", 0, 10),
            stat_of(5, "b", 0, 10),
        ];
        let rows = process_rows(&current, &prev, Duration::from_secs(2), 4096);
        assert_eq!(
            rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![3, 5, 7]
        );
    }

    #[test]
    fn a_zero_length_interval_cannot_produce_infinite_cpu() {
        let prev: HashMap<i32, u64> = [(1, 0)].into_iter().collect();
        let current = vec![stat_of(1, "x", 500, 0)];
        let rows = process_rows(&current, &prev, Duration::ZERO, 4096);
        assert_eq!(rows[0].cpu_pct, 0.0);
    }
}
