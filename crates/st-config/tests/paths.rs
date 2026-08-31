//! Path resolution. Every case uses a synthetic environment: the real `$HOME`
//! and the real process environment are never read or written.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use st_config::{Paths, Platform};

/// Builds `Paths` over a fixed map, so tests can run in parallel without
/// touching the process environment.
fn paths(platform: Platform, vars: &[(&str, &str)]) -> Paths {
    let map: BTreeMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), OsString::from(*v)))
        .collect();
    Paths::from_lookup(platform, 1000, move |k| map.get(k).cloned())
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn linux_uses_xdg_when_set() {
    let ps = paths(
        Platform::Linux,
        &[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/x/config"),
            ("XDG_STATE_HOME", "/x/state"),
            ("XDG_CACHE_HOME", "/x/cache"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ],
    );
    assert_eq!(ps.config_dir().unwrap(), p("/x/config/superterminal"));
    assert_eq!(
        ps.config_path().unwrap(),
        p("/x/config/superterminal/config.toml")
    );
    assert_eq!(ps.state_dir().unwrap(), p("/x/state/superterminal"));
    assert_eq!(ps.cache_dir().unwrap(), p("/x/cache/superterminal"));
    assert_eq!(ps.log_dir().unwrap(), p("/x/state/superterminal/logs"));
    assert_eq!(
        ps.workspace_file().unwrap(),
        p("/x/state/superterminal/workspace.json")
    );
    assert_eq!(ps.runtime_dir(), p("/run/user/1000/superterminal"));
    assert_eq!(
        ps.socket_path(),
        p("/run/user/1000/superterminal/server.sock")
    );
    assert_eq!(ps.lock_path(), p("/run/user/1000/superterminal/lock"));
}

#[test]
fn linux_falls_back_to_home_when_xdg_is_unset() {
    let ps = paths(Platform::Linux, &[("HOME", "/home/u")]);
    assert_eq!(
        ps.config_path().unwrap(),
        p("/home/u/.config/superterminal/config.toml")
    );
    assert_eq!(
        ps.state_dir().unwrap(),
        p("/home/u/.local/state/superterminal")
    );
    assert_eq!(ps.cache_dir().unwrap(), p("/home/u/.cache/superterminal"));
    assert_eq!(ps.runtime_dir(), p("/tmp/superterminal-1000"));
}

#[test]
fn empty_env_vars_count_as_unset() {
    let ps = paths(
        Platform::Linux,
        &[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", ""),
            ("XDG_RUNTIME_DIR", ""),
        ],
    );
    assert_eq!(
        ps.config_path().unwrap(),
        p("/home/u/.config/superterminal/config.toml")
    );
    assert_eq!(ps.runtime_dir(), p("/tmp/superterminal-1000"));
}

#[test]
fn runtime_dir_prefers_tmpdir_over_slash_tmp() {
    let ps = paths(
        Platform::Linux,
        &[("HOME", "/home/u"), ("TMPDIR", "/var/folders/ab")],
    );
    assert_eq!(ps.runtime_dir(), p("/var/folders/ab/superterminal-1000"));
    assert_eq!(
        ps.socket_path(),
        p("/var/folders/ab/superterminal-1000/server.sock")
    );
}

#[test]
fn macos_uses_library_layout() {
    let ps = paths(Platform::MacOs, &[("HOME", "/Users/u")]);
    assert_eq!(
        ps.config_path().unwrap(),
        p("/Users/u/Library/Application Support/superterminal/config.toml")
    );
    assert_eq!(
        ps.state_dir().unwrap(),
        p("/Users/u/Library/Application Support/superterminal")
    );
    assert_eq!(
        ps.cache_dir().unwrap(),
        p("/Users/u/Library/Caches/superterminal")
    );
    assert_eq!(ps.runtime_dir(), p("/tmp/superterminal-1000"));
}

#[test]
fn macos_still_honours_an_explicit_xdg_config_home() {
    let ps = paths(
        Platform::MacOs,
        &[
            ("HOME", "/Users/u"),
            ("XDG_CONFIG_HOME", "/Users/u/.config"),
        ],
    );
    assert_eq!(
        ps.config_path().unwrap(),
        p("/Users/u/.config/superterminal/config.toml")
    );
}

#[test]
fn superterminal_overrides_win() {
    let ps = paths(
        Platform::Linux,
        &[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/x/config"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("SUPERTERMINAL_CONFIG", "/etc/st.toml"),
            ("SUPERTERMINAL_RUNTIME_DIR", "/run/custom"),
            ("SUPERTERMINAL_SOCKET", "/tmp/st-dev.sock"),
            ("SUPERTERMINAL_STATE_DIR", "/var/st-state"),
            ("SUPERTERMINAL_CACHE_DIR", "/var/st-cache"),
        ],
    );
    assert_eq!(ps.config_path().unwrap(), p("/etc/st.toml"));
    // The override names a file, so the directory is unaffected.
    assert_eq!(ps.config_dir().unwrap(), p("/x/config/superterminal"));
    assert_eq!(ps.runtime_dir(), p("/run/custom"));
    assert_eq!(ps.socket_path(), p("/tmp/st-dev.sock"));
    assert_eq!(ps.lock_path(), p("/run/custom/lock"));
    assert_eq!(ps.state_dir().unwrap(), p("/var/st-state"));
    assert_eq!(ps.log_dir().unwrap(), p("/var/st-state/logs"));
    assert_eq!(ps.cache_dir().unwrap(), p("/var/st-cache"));
}

#[test]
fn no_home_and_no_xdg_is_a_clear_error() {
    let ps = paths(Platform::Linux, &[]);
    let err = ps.config_path().unwrap_err();
    assert!(err.to_string().contains("SUPERTERMINAL_CONFIG"), "{err}");
    assert!(ps.state_dir().unwrap_err().to_string().contains("$HOME"));
    assert!(ps.cache_dir().unwrap_err().to_string().contains("$HOME"));
    // The runtime dir always resolves: it needs no environment at all.
    assert_eq!(ps.runtime_dir(), p("/tmp/superterminal-1000"));
}

#[test]
fn ensure_creates_directories_with_mode_0700() {
    let base = std::env::temp_dir().join(format!("st-config-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);

    let ps = paths(
        Platform::Linux,
        &[
            ("HOME", base.join("home").to_str().unwrap()),
            ("XDG_RUNTIME_DIR", base.join("run").to_str().unwrap()),
        ],
    );

    let made = [
        ps.ensure_config_dir().unwrap(),
        ps.ensure_runtime_dir().unwrap(),
        ps.ensure_state_dir().unwrap(),
        ps.ensure_cache_dir().unwrap(),
        ps.ensure_log_dir().unwrap(),
    ];
    for dir in &made {
        assert!(dir.is_dir(), "{} was not created", dir.display());
        assert_mode_700(dir);
    }

    // The socket's parent is created, the socket itself is not.
    let sock = ps.ensure_socket_path().unwrap();
    assert!(sock.parent().unwrap().is_dir());
    assert!(!sock.exists());

    // Idempotent, and it repairs a too-permissive existing directory.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&made[0], std::fs::Permissions::from_mode(0o755)).unwrap();
        let again = ps.ensure_config_dir().unwrap();
        assert_eq!(again, made[0]);
        assert_mode_700(&again);
    }

    std::fs::remove_dir_all(&base).unwrap();
}

fn assert_mode_700(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{} has mode {mode:o}", dir.display());
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[test]
fn process_environment_paths_are_consistent() {
    // Whatever the machine looks like, the free functions must agree with the
    // Paths built from the same environment.
    let ps = Paths::from_env();
    assert_eq!(ps.platform(), Platform::current());
    assert_eq!(st_config::runtime_dir(), ps.runtime_dir());
    assert_eq!(st_config::socket_path(), ps.socket_path());
    assert_eq!(st_config::lock_path(), ps.lock_path());
    assert_eq!(
        st_config::socket_path().parent().unwrap(),
        st_config::runtime_dir()
    );
    assert_eq!(
        st_config::Config::default_path().ok(),
        ps.config_path().ok()
    );
    assert_eq!(st_config::state_dir().ok(), ps.state_dir().ok());
    assert_eq!(st_config::cache_dir().ok(), ps.cache_dir().ok());
    assert_eq!(st_config::log_dir().ok(), ps.log_dir().ok());
    assert_eq!(st_config::workspace_file().ok(), ps.workspace_file().ok());
}

#[test]
fn current_uid_is_plausible() {
    // Not a fixed value, but it must be stable and match a file we own.
    let a = st_config::current_uid();
    assert_eq!(a, st_config::current_uid());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let probe = std::env::temp_dir().join(format!("st-config-uid-{}", std::process::id()));
        std::fs::write(&probe, b"x").unwrap();
        assert_eq!(std::fs::metadata(&probe).unwrap().uid(), a);
        std::fs::remove_file(&probe).unwrap();
    }
}
