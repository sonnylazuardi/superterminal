//! Real PTY tests: a shell is spawned, its output is parsed, and the Snapshot
//! is checked against what the shell actually printed.
//!
//! Every test skips itself (with a printed reason) when no POSIX shell is
//! available, so the suite still passes in a bare container.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::grid_text;
use st_core::{EngineConfig, ExitStatus, Pty, PtyConfig, Surface, SurfaceConfig, SurfaceStatus};

/// Generous: a cold container can take a while to exec a shell.
const TIMEOUT: Duration = Duration::from_secs(15);

fn shell() -> Option<PathBuf> {
    ["/bin/sh", "/bin/bash", "/usr/bin/sh"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn surface_running(program: &Path, args: &[&str], cols: u16, rows: u16) -> Surface {
    Surface::new(SurfaceConfig {
        engine: EngineConfig {
            cols,
            rows,
            scrollback_lines: 200,
            default_title: "sh".into(),
            ..EngineConfig::default()
        },
        pty: Some(PtyConfig {
            program: Some(program.to_path_buf()),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            login: false,
            cols,
            rows,
            ..PtyConfig::default()
        }),
        ..SurfaceConfig::default()
    })
    .expect("spawning a shell into a PTY")
}

/// Moves the PTY reader onto a thread so the test can poll with a timeout.
fn reader_channel(pty: &Pty) -> mpsc::Receiver<Vec<u8>> {
    let mut reader = pty.reader().expect("a readable master");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Pumps PTY output into the Surface until `done` is satisfied or time runs out.
fn pump_until(
    surface: &mut Surface,
    rx: &mpsc::Receiver<Vec<u8>>,
    mut done: impl FnMut(&mut Surface) -> bool,
) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if done(surface) {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return done(surface);
        }
        match rx.recv_timeout(left.min(Duration::from_millis(250))) {
            Ok(chunk) => surface.feed(&chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return done(surface),
        }
    }
}

fn contains(surface: &mut Surface, needle: &str) -> bool {
    grid_text(&surface.snapshot())
        .iter()
        .any(|line| line.contains(needle))
}

#[test]
fn a_shell_prints_into_the_grid() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(&sh, &["-c", "printf 'hello\\n'"], 40, 6);
    let rx = reader_channel(surface.pty().unwrap());

    assert!(
        pump_until(&mut surface, &rx, |s| contains(s, "hello")),
        "the shell's output never reached the grid"
    );

    let snapshot = surface.snapshot();
    assert_eq!(snapshot.cols, 40);
    assert_eq!(snapshot.rows, 6);
    assert_eq!(grid_text(&snapshot)[0], "hello", "at row 0, column 0");
    assert_eq!(snapshot.grid[0].cells.len(), 5);
    assert_eq!(snapshot.cursor.row, 1, "the cursor moved to the next line");
    assert_eq!(snapshot.cursor.col, 0);

    let exit = wait_for_exit(&mut surface);
    assert_eq!(exit.code, Some(0));
}

#[test]
fn the_environment_reaches_the_child() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(
        &sh,
        &[
            "-c",
            "printf '%s|%s|%s\\n' \"$TERM\" \"$TERM_PROGRAM\" \"$COLORTERM\"",
        ],
        60,
        4,
    );
    let rx = reader_channel(surface.pty().unwrap());
    assert!(pump_until(&mut surface, &rx, |s| contains(s, "|")));
    assert_eq!(
        grid_text(&surface.snapshot())[0],
        "xterm-256color|superterminal|truecolor"
    );
}

#[test]
fn an_interactive_shell_echoes_input_and_reports_its_size() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(&sh, &["-i"], 50, 8);
    let rx = reader_channel(surface.pty().unwrap());
    let mut writer = surface.pty().unwrap().writer().expect("a writable master");

    writer
        .write_all(b"stty size; echo MARK-$?\n")
        .and_then(|()| writer.flush())
        .expect("writing to the PTY");
    assert!(
        pump_until(&mut surface, &rx, |s| contains(s, "MARK-0")),
        "the shell never answered; grid was {:?}",
        grid_text(&surface.snapshot())
    );
    assert!(
        contains(&mut surface, "8 50"),
        "the kernel window size is the grid size; grid was {:?}",
        grid_text(&surface.snapshot())
    );

    // A resize must reach the child through SIGWINCH.
    surface.resize(70, 12).unwrap();
    writer
        .write_all(b"stty size; echo DONE-$?\n")
        .and_then(|()| writer.flush())
        .expect("writing to the PTY");
    assert!(
        pump_until(&mut surface, &rx, |s| contains(s, "DONE-0")),
        "the shell never answered after the resize"
    );
    assert!(
        contains(&mut surface, "12 70"),
        "grid was {:?}",
        grid_text(&surface.snapshot())
    );

    writer.write_all(b"exit\n").ok();
    let _ = wait_for_exit(&mut surface);
}

#[test]
fn a_nonzero_exit_is_reported() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(&sh, &["-c", "exit 7"], 20, 3);
    let exit = wait_for_exit(&mut surface);
    assert_eq!(exit.code, Some(7));
    assert!(!exit.success());

    let event = surface.set_exited(exit);
    assert_eq!(event.status.code, Some(7));
    assert!(!surface.status().is_running());
    assert!(
        surface.snapshot().exited.is_some(),
        "Q22: the grid survives"
    );
}

#[test]
fn sighup_to_the_process_group_kills_the_shell() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(&sh, &["-c", "sleep 30"], 20, 3);
    let pty = surface.pty().unwrap();
    assert!(pty.pid().is_some());
    assert!(pty.hangup(), "killpg(pgid, SIGHUP) should succeed");

    let exit = wait_for_exit(&mut surface);
    assert!(
        !exit.success(),
        "the shell should not have exited cleanly: {exit:?}"
    );
}

#[test]
fn the_foreground_process_group_is_visible() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(&sh, &["-c", "sleep 5"], 20, 3);
    let pgid = surface.pty().unwrap().foreground_pgid();
    if cfg!(target_os = "linux") {
        assert!(pgid.is_some(), "tcgetpgrp should work on Linux");
        // The cwd probe rides on the same pid.
        surface.probe_cwd();
        assert!(surface.cwd().is_absolute());
    }
    surface.pty_mut().unwrap().kill();
    let _ = wait_for_exit(&mut surface);
}

fn wait_for_exit(surface: &mut Surface) -> ExitStatus {
    let pty = surface.pty_mut().expect("this Surface has a PTY");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(status)) = pty.try_wait() {
            return status;
        }
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn status_starts_running_with_a_pid() {
    let Some(sh) = shell() else {
        eprintln!("skipping: no POSIX shell on this machine");
        return;
    };
    let mut surface = surface_running(&sh, &["-c", "exit 0"], 20, 3);
    match surface.status() {
        SurfaceStatus::Running { pid } => assert!(pid.is_some()),
        other => panic!("expected Running, got {other:?}"),
    }
    let _ = wait_for_exit(&mut surface);
    let event = surface.poll_exit().expect("poll_exit records the exit");
    assert_eq!(event.surface_id, surface.id());
    assert!(surface.poll_exit().is_none(), "reported once");
}
