//! Connect screen — Bluesky OAuth (auth broker) + guest, freeq-android inspired.

use eframe::egui::{self, RichText};
use vidya::{
    body, button, checkbox, destructive_button, dim_label, primary_button, text_field_singleline,
    title_2, Theme,
};

use crate::state::{AppState, ConnectMode, ConnectionState};
use crate::ui::widgets::card;

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
                let _ = text_field_singleline(ui, th, &mut state.form_handle);
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
