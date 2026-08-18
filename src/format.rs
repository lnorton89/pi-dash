//! Value formatting shared by every pane: byte rates, sizes and ages.
//!
//! Meters and graphs are not here — they emit styled spans rather than
//! strings, because a gradient is a property of the cells and not of the
//! text. See [`crate::ui::gauge`].
//!
//! Everything here is deliberately narrow — fixed-width, no thousands
//! separators, one decimal at most. The panes live in a ~46-column body and a
//! number that grows a character when it crosses a power of 1024 reflows the
//! column next to it, which on a dashboard you watch out of the corner of your
//! eye reads as the display glitching.

/// `1.2 MB/s`. Used where there is room for the full unit.
pub fn human_rate(bytes_per_sec: f64) -> String {
    let b = bytes_per_sec.max(0.0);
    if b < 1024.0 {
        format!("{:.0} B/s", b)
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", b / 1024.0)
    } else {
        format!("{:.1} MB/s", b / (1024.0 * 1024.0))
    }
}

/// `1.2M`. Used for the per-interface rx/tx columns, where two of these plus
/// the interface name and mode already fill the pane.
pub fn human_rate_compact(bytes_per_sec: f64) -> String {
    let b = bytes_per_sec.max(0.0);
    if b < 1024.0 {
        format!("{:.0}B", b)
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1}K", b / 1024.0)
    } else {
        format!("{:.1}M", b / (1024.0 * 1024.0))
    }
}

/// kB in (the unit /proc/meminfo and `df -k` both use), `1.2G` out.
pub fn human_kb(kb: u64) -> String {
    // "0K" reads as a rounded-down small number; plain "0" reads as nothing,
    // which is what it is. Swap on a fresh boot is the case that matters.
    if kb == 0 {
        return "0".to_string();
    }
    let k = kb as f64;
    if k < 1024.0 {
        format!("{:.0}K", k)
    } else if k < 1024.0 * 1024.0 {
        format!("{:.0}M", k / 1024.0)
    } else {
        format!("{:.1}G", k / (1024.0 * 1024.0))
    }
}

/// `12s` / `4m` / `2h` / `3d` — the age of something, at one significant unit.
pub fn short_age(secs: i64) -> String {
    if secs < 0 {
        // Clock skew between this box and whatever stamped the record. Say so
        // rather than printing a negative age that looks like a parse bug.
        return "+".to_string();
    }
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// `2d3h41m` — how long the box has been up, always all three units so the
/// field never changes width mid-watch.
pub fn uptime(secs: u64) -> String {
    format!(
        "{}d{}h{}m",
        secs / 86400,
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
    )
}

/// `1h 5m` / `12m` — a service uptime, where days are rare and seconds noise.
pub fn coarse_uptime(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}m", secs / 60)
    }
}

/// Truncates to `max` *characters* (not bytes) so a non-ASCII device string
/// out of `lsusb` can never split a UTF-8 sequence or overrun the pane.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_switch_unit_at_the_1024_boundaries() {
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(1023.0), "1023 B/s");
        assert_eq!(human_rate(1024.0), "1.0 KB/s");
        assert_eq!(human_rate(1_572_864.0), "1.5 MB/s");
        assert_eq!(human_rate_compact(1024.0), "1.0K");
        assert_eq!(human_rate_compact(1_572_864.0), "1.5M");
    }

    #[test]
    fn negative_rates_read_as_zero_not_as_a_huge_number() {
        // Counters reset when an adapter is re-plugged; the delta goes
        // negative for exactly one sample.
        assert_eq!(human_rate(-5.0), "0 B/s");
        assert_eq!(human_rate_compact(-5.0), "0B");
    }

    #[test]
    fn kilobytes_scale_to_gigabytes() {
        assert_eq!(human_kb(0), "0");
        assert_eq!(human_kb(512), "512K");
        assert_eq!(human_kb(2048), "2M");
        assert_eq!(human_kb(3_145_728), "3.0G");
    }

    #[test]
    fn ages_pick_one_unit() {
        assert_eq!(short_age(0), "0s");
        assert_eq!(short_age(59), "59s");
        assert_eq!(short_age(60), "1m");
        assert_eq!(short_age(3600), "1h");
        assert_eq!(short_age(86400), "1d");
        assert_eq!(short_age(-3), "+");
    }

    #[test]
    fn uptime_keeps_a_stable_width() {
        assert_eq!(uptime(0), "0d0h0m");
        assert_eq!(uptime(90_061), "1d1h1m");
        assert_eq!(coarse_uptime(59), "0m");
        assert_eq!(coarse_uptime(3_900), "1h 5m");
    }

    #[test]
    fn clip_counts_characters_not_bytes() {
        assert_eq!(clip("abcdef", 3), "abc");
        assert_eq!(clip("abc", 10), "abc");
        // A multi-byte name must not be cut mid-sequence.
        assert_eq!(clip("Réaltek café", 6), "Réalte");
    }
}
