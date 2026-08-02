//! Connect screen — Bluesky OAuth (auth broker) + guest, freeq-android inspired.

use eframe::egui::{self, Key, RichText, Vec2};
use vidya::{
    body, button, checkbox, destructive_button, dim_label, primary_button, text_field_singleline,
    title_2, Theme,
};

use crate::state::{AppState, ConnectMode, ConnectionState};
use crate::ui::widgets::{avatar_circle, card};

pub fn connect_screen(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> ConnectAction {
    let mut action = ConnectAction::None;
    let sp = &th.spacing;
    let p = &th.palette;
    let loading = state.connection == ConnectionState::Connecting || state.awaiting_oauth;
    let cached = state.has_cached_identity();

    card(ui, th, |ui| {
        match state.connect_mode {
            ConnectMode::Bluesky => {
                title_2(ui, th, "Sign in with Bluesky");
                ui.add_space(sp.sm);
                dim_label(
                    ui,
                    th,
                    "AT Protocol identity via freeq. Handle verified with SASL on IRC.",
                );
                ui.add_space(sp.lg);

                body(ui, th, "Bluesky handle");
                ui.add_space(sp.xs);
                handle_field(ui, th, state, loading);
                ui.add_space(sp.md);

                if let Some(err) = &state.error {
                    ui.label(
                        RichText::new(err)
                            .size(th.type_scale.caption)
                            .color(p.destructive),
                    );
                    ui.add_space(sp.sm);
                }

                if !state.status_line.is_empty() && loading {
                    dim_label(ui, th, &state.status_line);
                    ui.add_space(sp.sm);
                }

                ui.horizontal(|ui| {
                    ui.set_width(ui.available_width());
                    let label = if loading {
                        "Signing in…"
                    } else {
                        "Sign in with Bluesky"
                    };
                    let resp = primary_button(ui, th, label);
                    if resp.clicked() && !loading {
                        action = ConnectAction::BlueskyLogin;
                    }
                });

                ui.add_space(sp.sm);
                #[cfg(target_os = "android")]
                dim_label(
                    ui,
                    th,
                    "Browser opens to approve. When done, freeq:// returns you here automatically. Paste a freeq://auth link below only if needed.",
                );
                #[cfg(not(target_os = "android"))]
                dim_label(
                    ui,
                    th,
                    "Browser opens to approve (Chromium on VNC — Alt+Tab if covered). Or paste a freeq://auth link below.",
                );

                // Advanced: callback paste + session restore, only when needed.
                ui.add_space(sp.md);
                body(ui, th, "Paste callback (optional)");
                ui.add_space(sp.xs);
                let _ = text_field_singleline(ui, th, &mut state.form_callback);
                ui.add_space(sp.sm);
                if button(ui, th, "Apply pasted link").clicked() && !loading {
                    action = ConnectAction::ApplyCallback;
                }

                if state.has_saved_session() {
                    ui.add_space(sp.md);
                    if button(ui, th, "Reconnect saved session").clicked() && !loading {
                        action = ConnectAction::ReconnectSession;
                    }
                }

                if cached {
                    ui.add_space(sp.sm);
                    cached_account_row(ui, th, state, loading, &mut action);
                }
            }
            ConnectMode::Guest => {
                title_2(ui, th, "Guest connect");
                ui.add_space(sp.sm);
                dim_label(
                    ui,
                    th,
                    "Display nick only — no DID until you sign in with Bluesky.",
                );
                ui.add_space(sp.lg);

                body(ui, th, "Nickname");
                ui.add_space(sp.xs);
                let _ = text_field_singleline(ui, th, &mut state.form_nick);
                if state.has_saved_guest() {
                    ui.add_space(sp.xs);
                    dim_label(ui, th, "Saved guest — reconnects on next launch.");
                }
                ui.add_space(sp.md);

                body(ui, th, "Server");
                ui.add_space(sp.xs);
                let _ = text_field_singleline(ui, th, &mut state.form_server);
                ui.add_space(sp.md);

                checkbox(ui, th, &mut state.form_tls, "Use TLS");
                ui.add_space(sp.sm);
                checkbox(ui, th, &mut state.form_websocket, "WebSocket transport");

                if let Some(err) = &state.error {
                    ui.add_space(sp.md);
                    ui.label(
                        RichText::new(err)
                            .size(th.type_scale.caption)
                            .color(p.destructive),
                    );
                }

                if !state.status_line.is_empty() && loading {
                    ui.add_space(sp.sm);
                    dim_label(ui, th, &state.status_line);
                }

                if cached {
                    ui.add_space(sp.md);
                    cached_account_row(ui, th, state, loading, &mut action);
                }

                ui.add_space(sp.lg);
                ui.horizontal(|ui| {
                    ui.set_width(ui.available_width());
                    let label = if loading {
                        "Connecting…"
                    } else {
                        "Connect as guest"
                    };
                    let resp = primary_button(ui, th, label);
                    if resp.clicked() && !loading {
                        action = ConnectAction::ConnectGuest;
                    }
                });
            }
        }
    });

    ui.add_space(sp.md);

    // Toggle Bluesky ↔ guest
    ui.horizontal(|ui| {
        match state.connect_mode {
            ConnectMode::Bluesky => {
                if button(ui, th, "Continue as guest").clicked() {
                    state.connect_mode = ConnectMode::Guest;
                    state.error = None;
                    state.handle_typeahead.dismiss();
                }
            }
            ConnectMode::Guest => {
                if button(ui, th, "Sign in with Bluesky instead").clicked() {
                    state.connect_mode = ConnectMode::Bluesky;
                    state.error = None;
                }
            }
        }
    });

    action
}

/// Handle text field + AppView typeahead suggestions (debounced in `App`).
fn handle_field(ui: &mut egui::Ui, th: &Theme, state: &mut AppState, login_loading: bool) {
    let sp = &th.spacing;
    let p = &th.palette;
    let w = ui.available_width().max(1.0);

    let resp = ui.add(
        egui::TextEdit::singleline(&mut state.form_handle)
            .margin(th.text_edit_margin())
            .desired_width(w)
            .min_size(Vec2::new(0.0, sp.control_height))
            .hint_text("you.bsky.social")
            .interactive(!login_loading),
    );

    state.handle_typeahead.sync_from_input(&state.form_handle);

    let typeahead_open = state.handle_typeahead.open
        && (!state.handle_typeahead.suggestions.is_empty() || state.handle_typeahead.loading);

    if resp.has_focus() && typeahead_open {
        let mut accept = false;
        let mut dismiss = false;
        let mut down = false;
        let mut up = false;
        ui.input(|i| {
            if i.key_pressed(Key::ArrowDown) {
                down = true;
            }
            if i.key_pressed(Key::ArrowUp) {
                up = true;
            }
            if i.key_pressed(Key::Tab) || i.key_pressed(Key::Enter) {
                // Enter also submits login when no highlight — only accept if open+selected.
                if state.handle_typeahead.selected.is_some()
                    || !state.handle_typeahead.suggestions.is_empty()
                {
                    accept = true;
                }
            }
            if i.key_pressed(Key::Escape) {
                dismiss = true;
            }
        });
        if down {
            state.handle_typeahead.move_selection(1);
            ui.ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
        }
        if up {
            state.handle_typeahead.move_selection(-1);
            ui.ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
        }
        if dismiss {
            state.handle_typeahead.dismiss();
            ui.ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        }
        if accept && state.handle_typeahead.accept_selected(&mut state.form_handle) {
            ui.ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab));
            ui.ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        }
    }

    if !typeahead_open {
        return;
    }

    ui.add_space(sp.xs);
    let frame = egui::Frame::new()
        .fill(p.popover_bg)
        .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.sm as i8, sp.xs as i8));
    frame.show(ui, |ui| {
        ui.set_min_width(w);
        if state.handle_typeahead.loading && state.handle_typeahead.suggestions.is_empty() {
            dim_label(ui, th, "Searching handles…");
            return;
        }
        let suggestions = state.handle_typeahead.suggestions.clone();
        let selected = state.handle_typeahead.selected;
        let mut picked: Option<String> = None;
        let mut hovered: Option<usize> = None;
        for (i, actor) in suggestions.iter().enumerate() {
            let is_sel = selected == Some(i);
            let row_h = sp.control_height.max(40.0);
            let (rect, row_resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), egui::Sense::click());
            if is_sel || row_resp.hovered() {
                ui.painter().rect_filled(
                    rect,
                    sp.radius_sm,
                    if is_sel {
                        p.accent.gamma_multiply(0.18)
                    } else {
                        p.button_hover
                    },
                );
            }
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(Vec2::new(sp.sm, 4.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = sp.sm;
                    avatar_circle(ui, th, &actor.handle, 28.0);
                    ui.vertical(|ui| {
                        if let Some(name) = &actor.display_name {
                            ui.label(
                                RichText::new(name)
                                    .size(th.type_scale.body)
                                    .color(p.text)
                                    .strong(),
                            );
                        }
                        ui.label(
                            RichText::new(format!("@{}", actor.handle))
                                .size(th.type_scale.caption)
                                .color(p.text_secondary),
                        );
                    });
                },
            );
            if row_resp.clicked() {
                picked = Some(actor.handle.clone());
            }
            if row_resp.hovered() {
                hovered = Some(i);
            }
        }
        if let Some(i) = hovered {
            state.handle_typeahead.selected = Some(i);
        }
        if let Some(handle) = picked {
            state.handle_typeahead.pick(&handle, &mut state.form_handle);
        }
    });
}

/// Banner + destructive clear when a previous Bluesky login is still cached.
fn cached_account_row(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &AppState,
    loading: bool,
    action: &mut ConnectAction,
) {
    let sp = &th.spacing;
    let who = state
        .handle
        .as_deref()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            let n = state.nick.as_str();
            if n.is_empty() {
                None
            } else {
                Some(n)
            }
        })
        .unwrap_or("saved account");
    dim_label(ui, th, &format!("Saved account: {who}"));
    ui.add_space(sp.xs);
    if destructive_button(ui, th, "Clear saved account").clicked() && !loading {
        *action = ConnectAction::ClearAccount;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectAction {
    None,
    ConnectGuest,
    BlueskyLogin,
    ApplyCallback,
    ReconnectSession,
    /// Wipe disk + in-memory broker session so the next guest connect is clean.
    ClearAccount,
}
