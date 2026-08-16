//! The four panes. Each module owns its own state, does its own sampling, and
//! is drawn by the matching module under [`crate::ui`].
//!
//! Sampling reads `/proc`, `/sys` and `vcgencmd` directly rather than shelling
//! out to `iotop`/`nethogs`/`bandwhich`. That is inherited from the Bash
//! dashboard and it is still the right call: those three all need root or
//! CAP_NET_ADMIN to say anything useful, none of them ship on a stock Pi OS,
//! and the numbers they would add (per-process disk and network) are not the
//! numbers that break this box. The ones that do — an under-volting supply, an
//! adapter that dropped off USB, a wlan that fell out of monitor mode — are
//! all readable unprivileged, so this runs as a normal user.
//!
//! Every reader is written to *degrade*, never to fail: a missing `vcgencmd`,
//! an absent `/sys` entry or an unreadable `/proc` file yields `None` and a
//! pane that says so. That is the same rule the sensors themselves follow
//! (ClassG ADR-0003) and it is also what makes the binary runnable on a dev
//! machine that is not a Pi at all.

pub mod classg;
pub mod health;
pub mod radios;
pub mod system;

use std::path::Path;
use std::process::{Command, Stdio};

/// Reads a small `/proc` or `/sys` file, trimmed. `None` for anything that
/// does not exist or cannot be read — callers are expected to cope.
pub fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Reads a `/sys` file that holds a single integer.
pub fn read_u64(path: impl AsRef<Path>) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

/// Runs a command and returns its stdout, or `None` if it is not installed,
/// fails, or writes nothing. stderr is discarded on purpose: `vcgencmd` on a
/// non-Pi prints a complaint that is not interesting once the `None` has
/// already told the caller what it needs to know.
pub fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The kernel's page size, for turning `/proc/<pid>/stat`'s RSS field into
/// bytes.
///
/// Read from the auxiliary vector rather than assumed to be 4096: arm64 is a
/// multi-page-size architecture, and while Raspberry Pi OS builds a 4K kernel
/// today, a hard-coded constant would silently report memory 4x or 16x wrong
/// on a kernel that did not. AT_PAGESZ is key 6; entries are `usize` pairs.
pub fn page_size() -> u64 {
    const AT_PAGESZ: u64 = 6;
    const FALLBACK: u64 = 4096;
    let word = std::mem::size_of::<usize>();

    let Ok(auxv) = std::fs::read("/proc/self/auxv") else {
        return FALLBACK;
    };
    for pair in auxv.chunks_exact(word * 2) {
        let read = |bytes: &[u8]| -> u64 {
            let mut buf = [0u8; 8];
            buf[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
            u64::from_ne_bytes(buf)
        };
        let key = read(&pair[..word]);
        if key == AT_PAGESZ {
            let value = read(&pair[word..]);
            // Sanity-check rather than trust: a nonsense value here would
            // corrupt every memory figure on the screen.
            if value.is_power_of_two() && (1024..=1024 * 1024).contains(&value) {
                return value;
            }
        }
        if key == 0 {
            break;
        }
    }
    FALLBACK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_yield_none_rather_than_panicking() {
        assert!(read_trimmed("/definitely/not/here").is_none());
        assert!(read_u64("/definitely/not/here").is_none());
    }

    #[test]
    fn a_missing_command_yields_none() {
        assert!(command_output("pi-dash-no-such-binary", &["--version"]).is_none());
    }

    #[test]
    fn page_size_is_always_plausible() {
        let size = page_size();
        assert!(size.is_power_of_two());
        assert!((1024..=1024 * 1024).contains(&size));
    }
}
