//! Keyboard shortcut helpers for egui apps.
//!
//! Thin wrappers around egui’s input API so call sites stay readable and
//! platform labels stay consistent. Uses [`egui::Modifiers::COMMAND`] (Cmd on
//! macOS, Ctrl elsewhere) — no global registry or remapping.

use egui::{Key, Modifiers, Ui};

/// True when running on macOS (Cmd-based shortcuts / ⌘ labels).
#[inline]
pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// Platform command modifier glyph for labels: `"⌘"` on macOS, `"Ctrl"` elsewhere.
#[inline]
pub fn command_glyph() -> &'static str {
    if is_macos() {
        "⌘"
    } else {
        "Ctrl"
    }
}

/// Human-readable label for a command+key chord (`"⌘F"` / `"Ctrl+F"`).
pub fn command_shortcut_label(key: Key) -> String {
    let name = key.name();
    if is_macos() {
        format!("{}{}", command_glyph(), name)
    } else {
        format!("{}+{}", command_glyph(), name)
    }
}

/// Human-readable label for command+shift+key (`"⇧⌘F"` / `"Ctrl+Shift+F"`).
pub fn command_shift_shortcut_label(key: Key) -> String {
    let name = key.name();
    if is_macos() {
        format!("⇧{}{}", command_glyph(), name)
    } else {
        format!("{}+Shift+{}", command_glyph(), name)
    }
}

/// Label for the Escape key.
#[inline]
pub fn escape_label() -> &'static str {
    "Esc"
}

/// Consume a command+key chord this frame (Cmd/Ctrl+key).
///
/// Returns `true` if the chord was pressed and consumed so it won’t also
/// type into a focused text field.
pub fn consume_command(ui: &Ui, key: Key) -> bool {
    ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, key))
}

/// Consume a command+shift+key chord this frame.
pub fn consume_command_shift(ui: &Ui, key: Key) -> bool {
    ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, key))
}

/// Consume bare Escape this frame.
pub fn consume_escape(ui: &Ui) -> bool {
    ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
}

/// Non-consuming peek: true if command+key is pressed this frame.
pub fn command_pressed(ui: &Ui, key: Key) -> bool {
    ui.input(|i| i.key_pressed(key) && i.modifiers.matches_exact(Modifiers::COMMAND))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_glyph_matches_target() {
        if cfg!(target_os = "macos") {
            assert_eq!(command_glyph(), "⌘");
            assert!(is_macos());
        } else {
            assert_eq!(command_glyph(), "Ctrl");
            assert!(!is_macos());
        }
    }

    #[test]
    fn command_shortcut_label_formats_f() {
        let label = command_shortcut_label(Key::F);
        if cfg!(target_os = "macos") {
            assert_eq!(label, "⌘F");
        } else {
            assert_eq!(label, "Ctrl+F");
        }
    }

    #[test]
    fn command_shift_shortcut_label_formats_f() {
        let label = command_shift_shortcut_label(Key::F);
        if cfg!(target_os = "macos") {
            assert_eq!(label, "⇧⌘F");
        } else {
            assert_eq!(label, "Ctrl+Shift+F");
        }
    }

    #[test]
    fn escape_label_is_esc() {
        assert_eq!(escape_label(), "Esc");
    }
}
