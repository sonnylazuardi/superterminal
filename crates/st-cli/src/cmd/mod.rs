//! One module per subcommand.
//!
//! Every entry point takes an already-resolved
//! [`Connector`](crate::transport::Connector) and an `impl Write` for its
//! output, so the integration tests can point it at a fake server on a temp
//! socket and assert on the exact bytes.

pub mod config;
pub mod dump_data;
pub mod kill_server;
pub mod ls;
pub mod probe;
pub mod status;

/// Formats a duration in seconds the way `st status` and `st ls` want it:
/// `3d 04h 05m 06s`, dropping leading zero units.
#[must_use]
pub fn format_uptime(secs: u64) -> String {
    let (d, h, m, s) = (
        secs / 86_400,
        (secs % 86_400) / 3_600,
        (secs % 3_600) / 60,
        secs % 60,
    );
    if d > 0 {
        format!("{d}d {h:02}h {m:02}m {s:02}s")
    } else if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Formats a byte count with a binary unit: `1.5 MiB`.
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_drops_leading_zero_units() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(59), "59s");
        assert_eq!(format_uptime(61), "1m 01s");
        assert_eq!(format_uptime(3_661), "1h 01m 01s");
        assert_eq!(format_uptime(90_061), "1d 01h 01m 01s");
    }

    #[test]
    fn bytes_scale_to_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
