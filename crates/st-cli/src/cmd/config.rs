//! `st config` — locate, generate and dump `config.toml` (grilling Q34, Q46).
//!
//! Everything here is `st-config`'s work; this module only chooses the path
//! and the output format. `st config init` writes
//! [`st_config::Config::to_toml_string`], the commented document `st-config`
//! generates — the same one `docs/config-example.toml` is built from — so the
//! CLI and the shared fixture can never drift.

use std::io::Write;
use std::path::{Path, PathBuf};

use st_config::Config;

use crate::exit::{CliError, ExitCode, Result};

/// `st config path`.
pub fn path(out: &mut dyn Write) -> Result<()> {
    let path = resolve_path()?;
    writeln!(out, "{}", path.display()).map_err(write_error)
}

/// `st config init`.
///
/// Writes to `dest`, or to the resolved config path when `dest` is `None`.
/// `-` writes to `out` instead of the filesystem.
pub fn init(dest: Option<&Path>, force: bool, out: &mut dyn Write) -> Result<()> {
    let document = Config::default().to_toml_string().map_err(|e| {
        CliError::failure(format!("cannot generate the example configuration: {e}"))
    })?;

    let target = match dest {
        Some(p) if p.as_os_str() == "-" => {
            return out.write_all(document.as_bytes()).map_err(write_error);
        }
        Some(p) => p.to_path_buf(),
        None => resolve_path()?,
    };

    if target.exists() && !force {
        return Err(CliError::new(
            ExitCode::Failure,
            format!("{} already exists", target.display()),
        )
        .with_hint("pass --force to overwrite it, or --path - to print it instead"));
    }

    Config::default()
        .save_to(&target)
        .map_err(|e| CliError::failure(format!("cannot write {}: {e}", target.display())))?;
    writeln!(out, "wrote {}", target.display()).map_err(write_error)
}

/// `st config show`.
pub fn show(json: bool, out: &mut dyn Write) -> Result<()> {
    let path = resolve_path()?;
    let loaded = Config::load_from_verbose(&path)
        .map_err(|e| CliError::failure(format!("cannot load {}: {e}", path.display())))?;
    for warning in &loaded.warnings {
        eprintln!("st: warning: {warning}");
    }
    if !loaded.found {
        eprintln!(
            "st: no config at {}; showing built-in defaults",
            path.display()
        );
    }

    let text = if json {
        let mut doc = serde_json::to_string_pretty(&loaded.config)
            .map_err(|e| CliError::failure(format!("cannot encode the config as JSON: {e}")))?;
        doc.push('\n');
        doc
    } else {
        loaded
            .config
            .to_toml_string_compact()
            .map_err(|e| CliError::failure(format!("cannot encode the config as TOML: {e}")))?
    };
    out.write_all(text.as_bytes()).map_err(write_error)
}

/// The config path `st-config` resolves: `$SUPERTERMINAL_CONFIG`, then
/// `$XDG_CONFIG_HOME/superterminal/config.toml`, then the platform default.
fn resolve_path() -> Result<PathBuf> {
    Config::default_path().map_err(|e| {
        CliError::failure(format!("cannot determine the configuration path: {e}"))
            .with_hint("set $SUPERTERMINAL_CONFIG to a file")
    })
}

fn write_error(err: std::io::Error) -> CliError {
    CliError::failure(format!("cannot write output: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_to_stdout_produces_a_parseable_commented_document() {
        let mut out = Vec::new();
        init(Some(Path::new("-")), false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains('#'), "the example should be commented");
        assert_eq!(Config::parse_str(&text).unwrap(), Config::default());
    }

    #[test]
    fn init_writes_a_file_and_refuses_to_clobber_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.toml");

        let mut out = Vec::new();
        init(Some(&target), false, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().starts_with("wrote "));
        assert!(target.exists());

        let err = init(Some(&target), false, &mut Vec::new()).unwrap_err();
        assert_eq!(err.exit, ExitCode::Failure);
        assert!(err.message.contains("already exists"));

        // --force goes through.
        init(Some(&target), true, &mut Vec::new()).unwrap();
    }

    #[test]
    fn init_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested/deeper/config.toml");
        init(Some(&target), false, &mut Vec::new()).unwrap();
        assert!(target.exists());
    }
}
