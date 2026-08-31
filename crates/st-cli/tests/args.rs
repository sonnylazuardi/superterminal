//! Argument parsing, help text, exit codes and `st config`.

mod common;

use common::{code, run_st, run_st_env, stderr, stdout};

#[test]
fn help_lists_every_subcommand_and_the_exit_codes() {
    let out = run_st(None, &["--help"]);
    assert_eq!(code(&out), 0);
    let help = stdout(&out);
    for name in [
        "status",
        "ls",
        "probe",
        "kill-server",
        "dump-data",
        "config",
    ] {
        assert!(help.contains(name), "`{name}` missing from --help:\n{help}");
    }
    for line in [
        "0  success",
        "1  failure",
        "2  usage error",
        "3  no server",
        "4  protocol error",
        "5  not found",
        "6  refused",
    ] {
        assert!(help.contains(line), "`{line}` missing from --help:\n{help}");
    }
    assert!(help.contains("--socket"), "{help}");
}

#[test]
fn no_subcommand_is_a_usage_error() {
    let out = run_st(None, &[]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("Usage:"), "{}", stderr(&out));
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let out = run_st(None, &["frobnicate"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn a_negative_surface_id_is_a_usage_error() {
    let out = run_st(None, &["probe", "-1"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn version_prints_the_crate_version() {
    let out = run_st(None, &["--version"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).starts_with("st "), "{}", stdout(&out));
}

#[test]
fn config_path_follows_the_config_environment_override() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("custom.toml");
    let out = run_st_env(
        None,
        &[("SUPERTERMINAL_CONFIG", target.to_str().unwrap())],
        &["config", "path"],
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim_end(), target.to_str().unwrap());
}

#[test]
fn config_init_writes_a_commented_file_and_refuses_to_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("config.toml");
    let env = [("SUPERTERMINAL_CONFIG", target.to_str().unwrap())];

    let out = run_st_env(None, &env, &["config", "init"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), format!("wrote {}\n", target.display()));

    let written = std::fs::read_to_string(&target).unwrap();
    assert!(written.contains('#'), "should be commented:\n{written}");
    // The generated document must parse back to the defaults it documents.
    assert_eq!(
        st_config::Config::parse_str(&written).unwrap(),
        st_config::Config::default()
    );

    let out = run_st_env(None, &env, &["config", "init"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("already exists"), "{}", stderr(&out));

    let out = run_st_env(None, &env, &["config", "init", "--force"]);
    assert_eq!(code(&out), 0);
}

#[test]
fn config_init_can_print_to_stdout() {
    let out = run_st(None, &["config", "init", "--path", "-"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains('#'));
    assert_eq!(
        st_config::Config::parse_str(&text).unwrap(),
        st_config::Config::default()
    );
}

#[test]
fn config_show_dumps_the_effective_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("config.toml");
    std::fs::write(&target, "[font]\nsize = 15.5\n").unwrap();
    let env = [("SUPERTERMINAL_CONFIG", target.to_str().unwrap())];

    let out = run_st_env(None, &env, &["config", "show"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let toml = stdout(&out);
    assert!(toml.contains("size = 15.5"), "{toml}");
    // Defaults are filled in, not just the keys the file set.
    let parsed = st_config::Config::parse_str(&toml).unwrap();
    assert_eq!(parsed.font.size, 15.5);

    let out = run_st_env(None, &env, &["config", "show", "--json"]);
    assert_eq!(code(&out), 0);
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["font"]["size"], 15.5);
}

#[test]
fn config_show_warns_when_the_file_is_missing_but_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("absent.toml");
    let out = run_st_env(
        None,
        &[("SUPERTERMINAL_CONFIG", target.to_str().unwrap())],
        &["config", "show"],
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("showing built-in defaults"),
        "{}",
        stderr(&out)
    );
    assert!(!stdout(&out).is_empty());
}

#[test]
fn config_show_reports_unknown_keys_as_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("config.toml");
    std::fs::write(&target, "[font]\nsize = 14.0\nnonsense = 1\n").unwrap();
    let out = run_st_env(
        None,
        &[("SUPERTERMINAL_CONFIG", target.to_str().unwrap())],
        &["config", "show"],
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("unknown key `font.nonsense`"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn config_show_on_broken_toml_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("config.toml");
    std::fs::write(&target, "this is not toml =\n").unwrap();
    let out = run_st_env(
        None,
        &[("SUPERTERMINAL_CONFIG", target.to_str().unwrap())],
        &["config", "show"],
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).starts_with("st: "), "{}", stderr(&out));
}

#[test]
fn every_subcommand_has_its_own_help() {
    for name in [
        "status",
        "ls",
        "probe",
        "kill-server",
        "dump-data",
        "config",
    ] {
        let out = run_st(None, &[name, "--help"]);
        assert_eq!(code(&out), 0, "`st {name} --help` failed");
        assert!(
            stdout(&out).contains("Usage:"),
            "`st {name} --help` is empty"
        );
    }
}
