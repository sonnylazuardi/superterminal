//! Working-directory tracking for a Surface (`docs/plan/03-server.md` §9).
//!
//! Two independent sources, in priority order:
//!
//! 1. **OSC 7** — `ESC ] 7 ; file://<host>/<path> ST`, emitted by a shell hook.
//!    `alacritty_terminal`'s `Handler` has no OSC 7 callback at all, so the
//!    sequence can never be intercepted through `Term`; §9 therefore specifies
//!    a *second* `vte::Parser` with a [`Perform`] impl that implements nothing
//!    but `osc_dispatch`. That is [`Osc7Sniffer`], and the Surface feeds it the
//!    same bytes it feeds the engine.
//! 2. **A probe of the foreground process** — the PTY's foreground process
//!    group leader, then `readlink /proc/<pid>/cwd` on Linux.
//!
//! Anything the probe cannot answer falls back to the Surface's spawn cwd,
//! which is the caller's job (see [`CwdTracker::current`]).

use std::path::{Path, PathBuf};

use vte::{Parser, Perform};

/// Sniffs OSC 7 out of a PTY byte stream.
///
/// It is a full second VT parse of the same bytes, which is the price of
/// `alacritty_terminal` not surfacing OSC 7; the parser is a byte-wise state
/// machine, so the cost is a few ns per byte.
#[derive(Default)]
pub struct Osc7Sniffer {
    parser: Parser,
    sink: Osc7Sink,
}

impl std::fmt::Debug for Osc7Sniffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Osc7Sniffer")
            .field("latest", &self.sink.latest)
            .finish()
    }
}

impl Osc7Sniffer {
    /// A sniffer that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds PTY output through the sniffer.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.sink, bytes);
    }

    /// The most recent directory announced with OSC 7, if any.
    #[must_use]
    pub fn latest(&self) -> Option<&Path> {
        self.sink.latest.as_deref()
    }

    /// Takes the most recent directory, clearing the "changed" state.
    pub fn take_changed(&mut self) -> Option<PathBuf> {
        if std::mem::replace(&mut self.sink.changed, false) {
            self.sink.latest.clone()
        } else {
            None
        }
    }
}

#[derive(Debug, Default)]
struct Osc7Sink {
    latest: Option<PathBuf>,
    changed: bool,
}

impl Perform for Osc7Sink {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() < 2 || params[0] != b"7" {
            return;
        }
        if let Some(path) = parse_osc7(params[1]) {
            if self.latest.as_deref() != Some(path.as_path()) {
                self.latest = Some(path);
                self.changed = true;
            }
        }
    }
}

/// Parses the payload of an OSC 7 sequence: `file://<host>/<percent-encoded>`.
///
/// A bare absolute path (some shells emit one) is accepted too. Returns `None`
/// for anything that is not an absolute path.
#[must_use]
pub fn parse_osc7(payload: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(payload).ok()?;
    let path = if let Some(rest) = text.strip_prefix("file://") {
        // Everything up to the first '/' is the (ignored) host.
        let slash = rest.find('/')?;
        &rest[slash..]
    } else {
        text
    };
    let decoded = percent_decode(path)?;
    let decoded = decoded.trim_end_matches('\0');
    if !decoded.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(decoded))
}

/// Percent-decodes a URL path. Returns `None` on invalid UTF-8 or a truncated
/// escape.
fn percent_decode(input: &str) -> Option<String> {
    if !input.contains('%') {
        return Some(input.to_owned());
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            let byte = (hex(hi)? << 4) | hex(lo)?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Reads a process's working directory from the OS.
///
/// Linux: `readlink /proc/<pid>/cwd`. On other platforms this returns `None`.
///
/// TODO(macOS): `03-server.md` §9 specifies
/// `proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, …)` from `libproc`. It needs an
/// `unsafe` FFI declaration and is unreachable from the Linux CI, so it is
/// deliberately left out of v1 of this crate rather than shipped untested;
/// OSC 7 and the spawn cwd cover macOS in the meantime.
#[must_use]
pub fn probe_process_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// The current working directory of a Surface, from all available sources.
///
/// The Surface owns one of these, feeds it PTY bytes, and asks it for the cwd
/// whenever `workspace.json` or a new Tab needs one.
#[derive(Debug)]
pub struct CwdTracker {
    sniffer: Osc7Sniffer,
    probed: Option<PathBuf>,
    spawn_cwd: PathBuf,
}

impl CwdTracker {
    /// A tracker anchored at the directory the Surface was spawned in.
    #[must_use]
    pub fn new(spawn_cwd: PathBuf) -> Self {
        Self {
            sniffer: Osc7Sniffer::new(),
            probed: None,
            spawn_cwd,
        }
    }

    /// Feeds PTY output, looking for OSC 7.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.sniffer.feed(bytes);
    }

    /// Re-reads the foreground process's directory from the OS.
    ///
    /// `03-server.md` §9 runs this every 2 s while a Client is attached. It is
    /// only consulted when no OSC 7 has ever been seen.
    pub fn probe(&mut self, foreground_pid: Option<u32>) {
        if let Some(pid) = foreground_pid {
            if let Some(cwd) = probe_process_cwd(pid) {
                self.probed = Some(cwd);
            }
        }
    }

    /// The best directory known: OSC 7, else the probe, else the spawn cwd.
    #[must_use]
    pub fn current(&self) -> &Path {
        self.sniffer
            .latest()
            .or(self.probed.as_deref())
            .unwrap_or(&self.spawn_cwd)
    }

    /// Returns the directory if it changed since the last call.
    pub fn take_changed(&mut self) -> Option<PathBuf> {
        self.sniffer.take_changed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_file_url() {
        assert_eq!(
            parse_osc7(b"file://host/home/sonny/projects"),
            Some(PathBuf::from("/home/sonny/projects"))
        );
        assert_eq!(
            parse_osc7(b"file:///tmp"),
            Some(PathBuf::from("/tmp")),
            "an empty host is normal"
        );
    }

    #[test]
    fn percent_decodes_the_path() {
        assert_eq!(
            parse_osc7(b"file://h/home/a%20b/c%2Bd"),
            Some(PathBuf::from("/home/a b/c+d"))
        );
        assert_eq!(
            parse_osc7("file://h/home/caf%C3%A9".as_bytes()),
            Some(PathBuf::from("/home/café"))
        );
    }

    #[test]
    fn rejects_junk() {
        assert_eq!(parse_osc7(b"relative/path"), None);
        assert_eq!(parse_osc7(b"file://host"), None);
        assert_eq!(parse_osc7(b"file://h/a%"), None);
        assert_eq!(parse_osc7(b"file://h/a%zz"), None);
    }

    #[test]
    fn sniffs_osc7_out_of_a_stream() {
        let mut sniffer = Osc7Sniffer::new();
        sniffer.feed(b"hello \x1b]0;title\x07 world");
        assert_eq!(sniffer.latest(), None, "OSC 0 is not OSC 7");

        sniffer.feed(b"\x1b]7;file://box/home/sonny\x1b\\");
        assert_eq!(sniffer.latest(), Some(Path::new("/home/sonny")));
        assert_eq!(
            sniffer.take_changed(),
            Some(PathBuf::from("/home/sonny")),
            "the first sighting is a change"
        );
        assert_eq!(sniffer.take_changed(), None, "and only once");

        // Split across two feeds, the way a PTY read would.
        sniffer.feed(b"\x1b]7;file://box/tm");
        assert_eq!(sniffer.latest(), Some(Path::new("/home/sonny")));
        sniffer.feed(b"p\x07");
        assert_eq!(sniffer.latest(), Some(Path::new("/tmp")));
        assert_eq!(sniffer.take_changed(), Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn tracker_prefers_osc7_then_probe_then_spawn() {
        let mut tracker = CwdTracker::new(PathBuf::from("/spawn"));
        assert_eq!(tracker.current(), Path::new("/spawn"));

        tracker.probed = Some(PathBuf::from("/probed"));
        assert_eq!(tracker.current(), Path::new("/probed"));

        tracker.feed(b"\x1b]7;file:///osc7\x07");
        assert_eq!(tracker.current(), Path::new("/osc7"));
        assert_eq!(tracker.take_changed(), Some(PathBuf::from("/osc7")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probes_our_own_cwd() {
        let pid = std::process::id();
        let probed = probe_process_cwd(pid).expect("/proc/self/cwd is readable");
        assert_eq!(probed, std::env::current_dir().unwrap());
        assert_eq!(probe_process_cwd(u32::MAX), None);
    }
}
