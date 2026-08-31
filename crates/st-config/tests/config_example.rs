//! `docs/config-example.toml` is generated from [`st_config::Config`], so it
//! can never drift from the schema.
//!
//! Run `UPDATE_EXPECT=1 cargo test -p st-config` after changing the schema to
//! regenerate it.

use std::path::PathBuf;

use st_config::Config;

/// Reads the checked-in example, first regenerating it when `UPDATE_EXPECT`
/// is set (so the three tests below cannot race each other on a fresh tree).
fn example_text() -> String {
    let path = example_path();
    if std::env::var_os("UPDATE_EXPECT").is_some() {
        let expected = Config::default().to_toml_string().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if std::fs::read_to_string(&path).ok().as_deref() != Some(expected.as_str()) {
            std::fs::write(&path, &expected).unwrap();
        }
        return expected;
    }
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nrun `UPDATE_EXPECT=1 cargo test -p st-config` to generate it",
            path.display()
        )
    })
}

fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("config-example.toml")
}

#[test]
fn docs_config_example_matches_the_generated_document() {
    let expected = Config::default().to_toml_string().unwrap();
    let path = example_path();
    let actual = example_text();

    assert_eq!(
        actual,
        expected,
        "{} is out of date; run `UPDATE_EXPECT=1 cargo test -p st-config`",
        path.display()
    );
}

#[test]
fn the_example_is_itself_a_valid_default_config() {
    let text = example_text();
    let loaded = Config::parse_str_verbose(&text).unwrap();
    assert_eq!(loaded.config, Config::default());
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn the_example_documents_every_section() {
    let text = example_text();
    for section in [
        "[font]",
        "[window]",
        "[window.padding]",
        "[shell]",
        "[terminal]",
        "[theme]",
        "[keybindings]",
        "[server]",
    ] {
        assert!(
            text.lines().any(|l| l.trim_end() == section),
            "{section} is missing from docs/config-example.toml"
        );
    }
    // Optional keys appear as commented-out examples, not as active settings.
    for optional in ["# family = ", "# program = ", "# login = "] {
        assert!(text.contains(optional), "{optional} missing");
    }
    for active in ["family =", "program =", "login ="] {
        assert!(
            !text.lines().any(|l| l.starts_with(active)),
            "{active} should be commented out"
        );
    }
}
