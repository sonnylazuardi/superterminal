//! Loading, merging, validating and re-serialising `config.toml`.

use std::collections::BTreeMap;

use st_config::{
    BackspaceSends, Config, ConfigError, OptionAsAlt, Platform, Rgb, ShellConfig, TerminalConfig,
    WindowBackground,
};

fn tempdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "st-config-test-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn empty_input_equals_defaults() {
    assert_eq!(Config::parse_str("").unwrap(), Config::default());
    assert_eq!(
        Config::parse_str(
            "[font]\n[window]\n[shell]\n[terminal]\n[theme]\n[keybindings]\n[server]\n"
        )
        .unwrap(),
        Config::default()
    );
}

#[test]
fn defaults_are_the_documented_values() {
    let c = Config::default();
    assert_eq!(c.font.family, None);
    assert_eq!(c.font.size, 13.0);
    assert_eq!(c.font.line_height, 1.2);
    assert_eq!(c.font.cell_height(), 13.0 * 1.2);
    assert_eq!(c.font.resolved_family(Platform::MacOs), "Menlo");
    assert_eq!(c.font.resolved_family(Platform::Linux), "DejaVu Sans Mono");

    assert_eq!(c.window.background, WindowBackground::Auto);
    assert!(!c.window.vertical_tabs);
    assert_eq!(c.window.padding.top, 8.0);

    assert_eq!(c.shell.program, None);
    assert!(c.shell.args.is_empty());
    assert_eq!(c.shell.login, None);
    assert!(c.shell.login_enabled(Platform::MacOs));
    assert!(!c.shell.login_enabled(Platform::Linux));

    assert_eq!(c.terminal.scrollback_lines, 10_000);
    assert!(!c.terminal.bold_is_bright);
    assert!(c.terminal.alt_screen_scroll);
    assert_eq!(c.terminal.option_as_alt, OptionAsAlt::None);
    assert_eq!(c.terminal.backspace_sends, BackspaceSends::Del);
    assert_eq!(c.terminal.backspace_sends.byte(), 0x7f);
    assert_eq!(BackspaceSends::Bs.byte(), 0x08);

    assert_eq!(c.theme.background, Rgb::new(0x1e, 0x1e, 0x1e));
    assert_eq!(c.theme.foreground, Rgb::new(0xd4, 0xd4, 0xd4));
    assert_eq!(c.theme.ansi().len(), 16);
    assert_eq!(c.theme.ansi()[1], c.theme.red);
    assert_eq!(st_config::ThemeConfig::ansi_key(9), Some("bright_red"));
    assert_eq!(st_config::ThemeConfig::ansi_key(16), None);

    assert!(c.keybindings.is_empty());
    assert_eq!(c.server.idle_exit_minutes, 15.0);
    assert!(!c.server.osc52);
}

#[test]
fn defaults_round_trip_through_both_serialisers() {
    let c = Config::default();

    let commented = c.to_toml_string().unwrap();
    assert_eq!(Config::parse_str(&commented).unwrap(), c);
    assert!(Config::parse_str_verbose(&commented)
        .unwrap()
        .warnings
        .is_empty());

    let compact = c.to_toml_string_compact().unwrap();
    assert_eq!(Config::parse_str(&compact).unwrap(), c);
}

#[test]
fn non_default_config_round_trips() {
    let mut c = Config::default();
    c.font.family = Some("JetBrains Mono".into());
    c.font.size = 15.5;
    c.window.background = WindowBackground::Transparent;
    c.window.vertical_tabs = true;
    c.window.padding.left = 0.0;
    c.shell.program = Some("/usr/bin/fish".into());
    c.shell.args = vec!["--no-config".into()];
    c.shell.login = Some(true);
    c.terminal.scrollback_lines = 42;
    c.terminal.word_chars = "a\"b\\c".into();
    c.terminal.option_as_alt = OptionAsAlt::Both;
    c.terminal.backspace_sends = BackspaceSends::Bs;
    c.theme.background = Rgb::new(0, 0, 0);
    c.server.idle_exit_minutes = 0.05;
    c.server.osc52 = true;
    c.keybindings.insert("tab.new".into(), "mod+shift+n".into());

    for text in [
        c.to_toml_string().unwrap(),
        c.to_toml_string_compact().unwrap(),
    ] {
        assert_eq!(
            Config::parse_str(&text).unwrap(),
            c,
            "round trip of:\n{text}"
        );
    }
}

#[test]
fn partial_config_merges_with_defaults() {
    let text = r##"
        [font]
        size = 16.0

        [theme]
        background = "#000"

        [keybindings]
        "tab.new" = "mod+n"
    "##;
    let c = Config::parse_str(text).unwrap();

    assert_eq!(c.font.size, 16.0);
    assert_eq!(c.font.line_height, Config::default().font.line_height);
    assert_eq!(c.font.family, None);
    assert_eq!(c.theme.background, Rgb::new(0, 0, 0));
    assert_eq!(c.theme.foreground, Config::default().theme.foreground);
    assert_eq!(c.terminal, TerminalConfig::default());
    assert_eq!(c.window, Config::default().window);
    assert_eq!(
        c.keybindings,
        BTreeMap::from([("tab.new".to_owned(), "mod+n".to_owned())])
    );
}

#[test]
fn invalid_color_reports_line_and_column() {
    let text = "[font]\nsize = 12.0\n\n[theme]\nbackground = \"rebeccapurple\"\n";
    let err = Config::parse_str(text).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("invalid colour"), "{msg}");
    assert!(msg.contains("#1e1e1e"), "expected an example in: {msg}");
    assert_eq!(err.line(), Some(5));
    assert_eq!(err.column(), Some(14));
    assert!(matches!(err, ConfigError::Parse { .. }));
}

#[test]
fn invalid_enum_lists_the_alternatives() {
    let err = Config::parse_str("[window]\nbackground = \"frosted\"\n").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown variant"), "{msg}");
    assert!(msg.contains("blurred"), "{msg}");
    assert_eq!(err.line(), Some(2));

    let err = Config::parse_str("[terminal]\nbackspace_sends = \"ctrl-h\"\n").unwrap_err();
    assert!(err.to_string().contains("del"), "{err}");
}

#[test]
fn wrong_type_is_an_error_not_a_default() {
    let err = Config::parse_str("[terminal]\nscrollback_lines = \"lots\"\n").unwrap_err();
    assert_eq!(err.line(), Some(2));
    let err = Config::parse_str("[font]\nsize = true\n").unwrap_err();
    assert_eq!(err.line(), Some(2));
}

#[test]
fn malformed_toml_reports_position() {
    let err = Config::parse_str("[font\nsize = 12\n").unwrap_err();
    assert!(err.to_string().contains("TOML"), "{err}");
    assert_eq!(err.line(), Some(1));
}

#[test]
fn unknown_keys_warn_and_are_ignored() {
    let loaded =
        Config::parse_str_verbose("[font]\nsize = 12.0\nweight = \"bold\"\n\n[nonsense]\nx = 1\n")
            .unwrap();
    assert_eq!(loaded.config.font.size, 12.0);
    assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
    assert!(loaded.warnings.iter().any(|w| w.contains("font.weight")));
    assert!(loaded.warnings.iter().any(|w| w.contains("nonsense.x")));
}

#[test]
fn out_of_range_values_are_clamped_with_a_warning() {
    let loaded = Config::parse_str_verbose(
        "[terminal]\nscrollback_lines = 5000000\n\n[font]\nsize = 0.0\n\n[server]\nidle_exit_minutes = -1.0\n",
    )
    .unwrap();
    assert_eq!(
        loaded.config.terminal.scrollback_lines,
        TerminalConfig::MAX_SCROLLBACK_LINES
    );
    assert_eq!(loaded.config.font.size, 13.0);
    assert_eq!(loaded.config.server.idle_exit_minutes, 15.0);
    assert_eq!(loaded.warnings.len(), 3, "{:?}", loaded.warnings);
}

#[test]
fn keybinding_typos_warn() {
    let loaded = Config::parse_str_verbose(
        "[keybindings]\n\"tab.knew\" = \"mod+t\"\n\"tab.new\" = \"hyper+t\"\n",
    )
    .unwrap();
    assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
    assert!(loaded.warnings.iter().any(|w| w.contains("tab.knew")));
    assert!(loaded.warnings.iter().any(|w| w.contains("hyper")));
    // The entry is still kept: the app decides what to do with it.
    assert_eq!(loaded.config.keybindings.len(), 2);
}

#[test]
fn missing_file_yields_defaults() {
    let dir = tempdir("missing");
    let path = dir.join("nope").join("config.toml");
    let loaded = Config::load_from_verbose(&path).unwrap();
    assert!(!loaded.found);
    assert_eq!(loaded.path, path);
    assert_eq!(loaded.config, Config::default());
    assert!(loaded.warnings.is_empty());
    assert_eq!(Config::load_from(&path).unwrap(), Config::default());
}

#[test]
fn save_then_load_round_trips_and_reports_the_path() {
    let dir = tempdir("save");
    let path = dir.join("sub").join("config.toml");

    let mut c = Config::default();
    c.terminal.bold_is_bright = true;
    c.save_to(&path).unwrap();

    let loaded = Config::load_from_verbose(&path).unwrap();
    assert!(loaded.found);
    assert_eq!(loaded.path, path);
    assert_eq!(loaded.config, c);
    assert!(loaded.warnings.is_empty());
}

#[test]
fn parse_errors_name_the_file() {
    let dir = tempdir("badfile");
    let path = dir.join("config.toml");
    std::fs::write(&path, "[theme]\ncursor = \"nope\"\n").unwrap();
    let err = Config::load_from(&path).unwrap_err();
    assert!(
        err.to_string().contains(&path.display().to_string()),
        "{err}"
    );
}

#[test]
fn shell_resolution_order() {
    let default = ShellConfig::default();

    let mut cfg = ShellConfig {
        login: Some(false),
        ..default.clone()
    };
    assert_eq!(
        cfg.resolve(Some("/bin/zsh")).program,
        std::path::PathBuf::from("/bin/zsh")
    );
    assert_eq!(
        cfg.resolve(None).program,
        std::path::PathBuf::from("/bin/sh")
    );
    assert_eq!(
        cfg.resolve(Some("")).program,
        std::path::PathBuf::from("/bin/sh")
    );

    cfg.program = Some("/usr/bin/fish".into());
    assert_eq!(
        cfg.resolve(Some("/bin/zsh")).program,
        std::path::PathBuf::from("/usr/bin/fish")
    );
}

#[test]
fn login_flag_only_for_shells_that_take_it() {
    let cfg = ShellConfig {
        program: None,
        args: vec![],
        login: Some(true),
    };
    let on = |shell| cfg.resolve_on(Platform::Linux, shell).args;
    assert_eq!(on(Some("/bin/bash")), vec!["-l".to_owned()]);
    assert_eq!(on(Some("/usr/bin/zsh")), vec!["-l".to_owned()]);
    assert_eq!(on(Some("/usr/local/bin/fish")), vec!["-l".to_owned()]);
    assert!(on(Some("/bin/sh")).is_empty());
    assert!(on(Some("/bin/dash")).is_empty());
    assert!(on(None).is_empty());

    let explicit = ShellConfig {
        program: Some("/bin/bash".into()),
        args: vec!["-l".into()],
        login: Some(true),
    };
    assert_eq!(
        explicit.resolve_on(Platform::Linux, None).args,
        vec!["-l".to_owned()],
        "no duplicate -l"
    );

    // The platform default applies when `login` is unset.
    let unset = ShellConfig {
        program: Some("/bin/bash".into()),
        args: vec![],
        login: None,
    };
    assert_eq!(
        unset.resolve_on(Platform::MacOs, None).args,
        vec!["-l".to_owned()]
    );
    assert!(unset.resolve_on(Platform::Linux, None).args.is_empty());

    let off = ShellConfig {
        login: Some(false),
        ..unset
    };
    assert!(off.resolve_on(Platform::MacOs, None).args.is_empty());
}

#[test]
fn window_background_resolution() {
    assert_eq!(
        WindowBackground::Auto.resolve(Platform::MacOs),
        WindowBackground::Blurred
    );
    assert_eq!(
        WindowBackground::Auto.resolve(Platform::Linux),
        WindowBackground::Opaque
    );
    assert_eq!(
        WindowBackground::Blurred.resolve(Platform::Linux),
        WindowBackground::Transparent
    );
    assert_eq!(
        WindowBackground::Opaque.resolve(Platform::MacOs),
        WindowBackground::Opaque
    );
    assert!(WindowBackground::Blurred.is_translucent());
    assert!(!WindowBackground::Opaque.is_translucent());
}

#[test]
fn word_chars_drive_selection() {
    let t = TerminalConfig::default();
    assert!(t.is_word_char('a'));
    assert!(t.is_word_char('9'));
    assert!(t.is_word_char('_'));
    assert!(t.is_word_char('/'));
    assert!(!t.is_word_char(' '));
    assert!(!t.is_word_char('"'));
}

#[test]
fn integers_are_accepted_for_float_fields() {
    let c = Config::parse_str(
        "[font]\nsize = 14\nline_height = 1\n\n[window.padding]\ntop = 0\n\n[server]\nidle_exit_minutes = 30\n",
    )
    .unwrap();
    assert_eq!(c.font.size, 14.0);
    assert_eq!(c.font.line_height, 1.0);
    assert_eq!(c.window.padding.top, 0.0);
    assert_eq!(c.server.idle_exit_minutes, 30.0);
}

#[test]
fn floats_serialise_without_f32_widening_noise() {
    let text = Config::default().to_toml_string().unwrap();
    assert!(text.contains("line_height = 1.2"), "{text}");
    assert!(!text.contains("1.2000000"), "{text}");
    assert!(text.contains("size = 13.0"), "{text}");
}
