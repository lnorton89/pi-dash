//! `pi-dash --once`: one plain-text snapshot, no terminal required.
//!
//! This exists because the interesting failures on this box are not the kind a
//! type-checker or a unit test can see — a wedged adapter, an under-volting
//! supply, an API that is not listening. `--once` is how you check the readers
//! against real hardware over SSH, from a cron job, or from a CI step, without
//! a TTY to drive.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::Config;
use crate::format::{human_kb, human_rate, human_rate_compact, uptime};
use crate::panes::classg::{self, Snapshot};
use crate::panes::health::{HealthPane, Tense, Throttle};
use crate::panes::radios::{RadiosPane, WirelessMode};
use crate::panes::system::SystemPane;

/// Gap between the two samples. Every rate on this dashboard is a difference
/// between two readings, so a single sample can only ever report zero — the
/// snapshot has to take two and wait in between, and that wait is the price of
/// the numbers being real.
const SETTLE: Duration = Duration::from_millis(700);

pub fn print_once(config: &Config, out: &mut impl Write) -> Result<()> {
    let mut system = SystemPane::default();
    let mut health = HealthPane::default();
    let mut radios = RadiosPane::default();

    system.sample(Instant::now());
    health.sample(Instant::now());
    radios.sample(
        Instant::now(),
        &config.usb_vendor_ids,
        &config.ignore_interfaces,
    );
    std::thread::sleep(SETTLE);
    let now = Instant::now();
    system.sample(now);
    health.sample(now);
    radios.sample(now, &config.usb_vendor_ids, &config.ignore_interfaces);

    writeln!(out, "system")?;
    match &system.unavailable {
        Some(reason) => writeln!(out, "  {reason}")?,
        None => {
            writeln!(
                out,
                "  cpu      {}  ({} cores)",
                system
                    .cpu_pct
                    .map(|p| format!("{p:.0}%"))
                    .unwrap_or_else(|| "-".to_string()),
                system.core_pct.len()
            )?;
            writeln!(
                out,
                "  mem      {}/{} ({:.0}%)   swap {}",
                human_kb(system.mem.used_kb()),
                human_kb(system.mem.total_kb),
                system.mem.used_pct(),
                if system.mem.swap_total_kb == 0 {
                    "off".to_string()
                } else {
                    format!(
                        "{}/{}",
                        human_kb(system.mem.swap_used_kb()),
                        human_kb(system.mem.swap_total_kb)
                    )
                }
            )?;
            writeln!(
                out,
                "  load     {:.2} {:.2} {:.2}   up {}   {} tasks",
                system.load[0],
                system.load[1],
                system.load[2],
                uptime(system.uptime_secs),
                system.task_count
            )?;
            for proc in system.procs.iter().take(5) {
                writeln!(
                    out,
                    "  {:>7}  {:>6.1}%  {:>7}  {}",
                    proc.pid,
                    proc.cpu_pct,
                    human_kb(proc.rss_kb),
                    proc.name
                )?;
            }
        }
    }

    writeln!(out)?;
    writeln!(out, "health")?;
    writeln!(
        out,
        "  temp     {}",
        health
            .temp_c
            .map(|t| format!("{t:.1}C"))
            .unwrap_or_else(|| "no thermal zone".to_string())
    )?;
    writeln!(
        out,
        "  power    {}   clock {}",
        health
            .volts
            .map(|v| format!("{v:.4}V"))
            .unwrap_or_else(|| "?V".to_string()),
        match (health.arm_mhz, health.max_mhz) {
            (Some(now), Some(max)) => format!("{now}/{max} MHz"),
            (Some(now), None) => format!("{now} MHz"),
            _ => "?".to_string(),
        }
    )?;
    writeln!(out, "  throttle {}", describe_throttle(health.throttle))?;
    writeln!(
        out,
        "  disk     {}",
        health
            .disk
            .map(|d| format!(
                "{}/{} ({:.0}%)",
                human_kb(d.used_kb),
                human_kb(d.total_kb),
                d.pct()
            ))
            .unwrap_or_else(|| "unavailable".to_string())
    )?;
    writeln!(
        out,
        "  io       r {}  w {}",
        human_rate(health.io.read_bps),
        human_rate(health.io.write_bps)
    )?;

    writeln!(out)?;
    writeln!(out, "radios")?;
    for iface in &radios.ifaces {
        writeln!(
            out,
            "  {:<10} {:<8} v{:<8} ^{:<8} {}{}",
            iface.name,
            iface.state,
            human_rate_compact(iface.rx_bps),
            human_rate_compact(iface.tx_bps),
            match iface.mode {
                Some(WirelessMode::Monitor) => "monitor",
                Some(WirelessMode::Managed) => "managed",
                None => "",
            },
            iface.channel.map(|c| format!(" ch{c}")).unwrap_or_default()
        )?;
    }
    if radios.usb.is_empty() {
        writeln!(out, "  usb      none present - adapters gone from the bus")?;
    } else {
        for device in &radios.usb {
            writeln!(out, "  usb      {}  {}", device.id, device.description)?;
        }
    }

    writeln!(out)?;
    writeln!(out, "classg  {}", config.api)?;
    let agent = classg::build_agent();
    print_classg(&classg::fetch(&agent, &config.api, 5, 5), out)?;
    Ok(())
}

fn describe_throttle(throttle: Option<Throttle>) -> String {
    let Some(throttle) = throttle else {
        return "unknown - no vcgencmd here".to_string();
    };
    if throttle.clean() {
        return "ok, clean since boot".to_string();
    }
    // Named rather than concatenated: "under-voltage, throttled" on its own
    // does not say whether the supply is sagging now or sagged an hour ago,
    // and that is the entire difference between the two failure modes.
    let join = |labels: Vec<&str>| {
        if labels.is_empty() {
            "-".to_string()
        } else {
            labels.join(", ")
        }
    };
    format!(
        "now [{}]  since boot [{}]  (0x{:x})",
        join(throttle.now.labels(Tense::Now)),
        join(throttle.since_boot.labels(Tense::SinceBoot)),
        throttle.raw
    )
}

fn print_classg(snapshot: &Snapshot, out: &mut impl Write) -> Result<()> {
    let Some(health) = &snapshot.health else {
        writeln!(
            out,
            "  not reachable: {}",
            snapshot.error.as_deref().unwrap_or("unknown error")
        )?;
        writeln!(out, "  start it with: make dev   (or set CLASSG_API)")?;
        return Ok(());
    };
    writeln!(
        out,
        "  status   {}  up {}s  {}",
        health.status, health.uptime_s, health.version
    )?;
    for sensor in &health.sensors {
        writeln!(
            out,
            "  sensor   {:<12} {}  5m:{}  {}",
            sensor.sensor_id,
            if sensor.healthy { "ok" } else { "DOWN" },
            sensor.detections_5m,
            sensor.reason.as_deref().unwrap_or("")
        )?;
    }
    if let Some(page) = &snapshot.tracks {
        writeln!(out, "  tracks   {} total", page.total)?;
    }
    if let Some(page) = &snapshot.detections {
        writeln!(out, "  detects  {} total", page.total)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_renders_without_a_terminal_and_without_an_api() {
        let config = Config {
            api: "http://127.0.0.1:1".to_string(),
            ..Config::default()
        };
        let mut buffer: Vec<u8> = Vec::new();
        print_once(&config, &mut buffer).expect("snapshot must not fail");
        let text = String::from_utf8(buffer).expect("utf-8");
        assert!(text.contains("system"));
        assert!(text.contains("health"));
        assert!(text.contains("radios"));
        assert!(text.contains("classg"));
        // A dead API degrades into a hint, it does not abort the snapshot.
        assert!(text.contains("not reachable"), "{text}");
        assert!(text.contains("make dev"));
    }

    #[test]
    fn throttle_description_distinguishes_unknown_from_clean() {
        assert_eq!(describe_throttle(None), "unknown - no vcgencmd here");
        assert_eq!(
            describe_throttle(Some(Throttle::decode(0))),
            "ok, clean since boot"
        );
        assert_eq!(
            describe_throttle(Some(Throttle::decode(0x50001))),
            "now [UNDER-VOLTAGE NOW]  since boot [under-voltage, throttled]  (0x50001)"
        );
        // A sticky-only register must not read as if it were happening now.
        assert_eq!(
            describe_throttle(Some(Throttle::decode(0x50000))),
            "now [-]  since boot [under-voltage, throttled]  (0x50000)"
        );
    }
}
