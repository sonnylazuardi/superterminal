//! The command ids that may appear as keys in `[keybindings]`.
//!
//! The authoritative registry lives in TypeScript
//! (`packages/app/src/commands/registry.ts`, `05-client-app.md` §5); this list
//! exists so `st config check` and the loader can warn about typos without
//! starting the app.

/// Every command id a `[keybindings]` entry may override.
pub const COMMAND_IDS: &[&str] = &[
    "app.quit",
    "app.reconnect",
    "edit.copy",
    "edit.paste",
    "palette.commands",
    "session.new",
    "session.rename",
    "session.switch",
    "surface.clearScrollback",
    "tab.close",
    "tab.goto.1",
    "tab.goto.2",
    "tab.goto.3",
    "tab.goto.4",
    "tab.goto.5",
    "tab.goto.6",
    "tab.goto.7",
    "tab.goto.8",
    "tab.goto.9",
    "tab.new",
    "tab.next",
    "tab.prev",
    "view.toggleVerticalTabs",
];

/// Modifier tokens accepted in a shortcut string.
///
/// `mod` is the portable one: ⌘ on macOS, Ctrl+Shift on Linux
/// (`05-client-app.md` §5).
pub const MODIFIERS: &[&str] = &["mod", "cmd", "super", "ctrl", "alt", "shift"];

/// Whether `id` is a known command id.
pub fn is_known_command(id: &str) -> bool {
    COMMAND_IDS.binary_search(&id).is_ok()
}

/// Checks the shape of a shortcut string such as `"mod+shift+t"`.
///
/// Returns `Err` with a human-readable reason; this is deliberately lenient
/// (it does not know the platform's key names) and is only used for warnings.
pub fn validate_shortcut(shortcut: &str) -> Result<(), String> {
    let trimmed = shortcut.trim();
    if trimmed.is_empty() {
        return Err("shortcut is empty".to_owned());
    }
    let parts: Vec<&str> = trimmed.split('+').collect();
    let (key, mods) = parts.split_last().expect("split always yields one part");
    for m in mods {
        let lower = m.to_ascii_lowercase();
        if !MODIFIERS.contains(&lower.as_str()) {
            return Err(format!(
                "unknown modifier `{m}` (expected one of {})",
                MODIFIERS.join(", ")
            ));
        }
    }
    if key.is_empty() {
        return Err("shortcut ends with `+` but names no key".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_sorted_for_binary_search() {
        let mut sorted = COMMAND_IDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, COMMAND_IDS);
        assert!(is_known_command("tab.new"));
        assert!(!is_known_command("tab.explode"));
    }

    #[test]
    fn validates_shortcut_shapes() {
        assert!(validate_shortcut("mod+shift+t").is_ok());
        assert!(validate_shortcut("ctrl+tab").is_ok());
        assert!(validate_shortcut("f5").is_ok());
        assert!(validate_shortcut("").is_err());
        assert!(validate_shortcut("hyper+t").is_err());
        assert!(validate_shortcut("mod+").is_err());
    }
}
