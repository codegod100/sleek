# Vendored `egui-winit` 0.31.1 patches

Source: crates.io `egui-winit` 0.31.1 (matches eframe/egui 0.31).

## Why

On Wayland, stock `egui-winit`:

1. Uses smithay for clipboard text and **does not fall through** to arboard on failure.
2. On Ctrl/Cmd+V, intercepts the key; if there is no text (image-only clipboard), it emits
   neither `Event::Paste` nor `Event::Key { V }`, so the app cannot implement image paste.

GNOME also lacks `wlr-data-control` / `ext-data-control`, so arboard’s Wayland path fails and
falls back to X11 — which still works for many image pastes when the compositor bridges.

## Changes

1. **`clipboard.rs` `get`**: on smithay error, fall through to arboard (upstream main does this).
   Demote paste miss logs from `error` to `debug` (image-only offers are normal).
2. **`lib.rs` paste shortcut**: if clipboard text is missing/empty, still push `Event::Key` for
   the paste key so apps can handle Ctrl+V image paste.
3. **`clipboard.rs` Android hooks**: `set_android_clipboard_hooks` so long-press paste reads the
   system `ClipboardManager` (stock fallback is in-app text only). Sleek attaches a Cut/Copy/Paste
   menu on `TextEdit`s (`text_edit_clipboard_menu`) that issues `ViewportCommand::RequestPaste`;
   without that menu the hooks never run for hold-to-paste.
4. **`lib.rs` Android back**: map `NamedKey::BrowserBack` (KEYCODE_BACK / gesture back) to
   `Key::Escape`. Upstream egui ≥0.32 adds `Key::BrowserBack` instead; drop this when upgrading.

Remove this vendor once upgrading to an egui that ships image paste events
(https://github.com/emilk/egui/issues/2108) or equivalent, and `Key::BrowserBack`.
