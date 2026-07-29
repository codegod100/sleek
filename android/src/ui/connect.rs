//! Connect screen — Bluesky OAuth (auth broker) + guest, freeq-android inspired.

use eframe::egui::{self, Align, Layout, RichText};
use vidya::{
    body, button, checkbox, dim_label, primary_button, text_field_singleline, title, title_2, Theme,
};

use crate::state::{AppState, ConnectMode, ConnectionState};
use crate::ui::widgets::card;

pub fn connect_screen(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> ConnectAction {
    let mut action = ConnectAction::None;
    let sp = &th.spacing;
    let p = &th.palette;
    let loading = state.connection == ConnectionState::Connecting || state.awaiting_oauth;

    ui.vertical_centered(|ui| {
        ui.add_space(sp.xl + sp.lg);

        // Logo mark
        let (rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 72.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 36.0, p.accent.gamma_multiply(0.35));
        ui.painter()
            .circle_filled(rect.center(), 28.0, p.accent);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "S",
            egui::FontId::proportional(28.0),
            p.accent_fg,
        );

        ui.add_space(sp.md);
        title(ui, th, "Sleek");
        ui.add_space(sp.xs);
        dim_label(ui, th, "freeq · mobile client");
        ui.add_space(sp.xl);
    });

    card(ui, th, |ui| {
        match state.connect_mode {
            ConnectMode::Bluesky => {
                title_2(ui, th, "Sign in with Bluesky");
                ui.add_space(sp.sm);
                body(
                    ui,
                    th,
                    "AT Protocol identity via the freeq auth broker. Your handle is verified with SASL on IRC.",
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

                ui.add_space(sp.md);
                dim_label(
                    ui,
                    th,
                    "A browser window opens. After you approve, return here — or paste a freeq://auth link below.",
                );
                ui.add_space(sp.sm);
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
            }
            ConnectMode::Guest => {
                title_2(ui, th, "Guest connect");
                ui.add_space(sp.sm);
                body(
                    ui,
                    th,
                    "Join freeq as a guest. Your nick is a display alias — no DID until you sign in with Bluesky.",
                );
                ui.add_space(sp.lg);

                body(ui, th, "Nickname");
                ui.add_space(sp.xs);
                let _ = text_field_singleline(ui, th, &mut state.form_nick);
                ui.add_space(sp.md);

                body(ui, th, "Server");
                ui.add_space(sp.xs);
                let _ = text_field_singleline(ui, th, &mut state.form_server);
                ui.add_space(sp.md);

                checkbox(ui, th, &mut state.form_tls, "Use TLS (recommended)");
                ui.add_space(sp.sm);
                checkbox(
                    ui,
                    th,
                    &mut state.form_websocket,
                    "WebSocket transport (good on mobile)",
                );

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

                ui.add_space(sp.lg);
                ui.horizontal(|ui| {
                    ui.set_width(ui.available_width());
                    let label = if loading { "Connecting…" } else { "Connect as guest" };
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

    ui.add_space(sp.md);

    card(ui, th, |ui| {
        title_2(ui, th, "About freeq");
        ui.add_space(sp.sm);
        body(
            ui,
            th,
            "IRC with AT Protocol identity. Messages can be signed; DMs can be end-to-end encrypted. Any IRC client works as a guest.",
        );
        ui.add_space(sp.md);
        ui.horizontal(|ui| {
            if button(ui, th, "irc.freeq.at").clicked() {
                state.form_server = "irc.freeq.at:6697".into();
                state.form_tls = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                dim_label(ui, th, "powered by vidya");
            });
        });
    });

    action
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectAction {
    None,
    ConnectGuest,
    BlueskyLogin,
    ApplyCallback,
    ReconnectSession,
}
