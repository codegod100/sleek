//! Settings tab — account, theme, chat prefs, disconnect.

use eframe::egui::{self, Align, Layout, RichText};
use vidya::{
    body, button, checkbox, destructive_button, dim_label, primary_button, status_dot, title,
    title_2, Mode, Theme,
};

use crate::state::{AppState, ConnectionState};
use crate::ui::widgets::{avatar_circle, card, section_label};

pub enum SettingsAction {
    None,
    Disconnect,
    Logout,
    ToggleTheme,
}

pub fn settings_tab(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    mode: Mode,
) -> SettingsAction {
    let mut action = SettingsAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    title(ui, th, "Settings");
    ui.add_space(sp.lg);

    // Account
    card(ui, th, |ui| {
        section_label(ui, th, "ACCOUNT");
        ui.add_space(sp.md);
        ui.horizontal(|ui| {
            avatar_circle(ui, th, &state.nick, 52.0);
            ui.add_space(sp.md);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(&state.nick)
                        .size(th.type_scale.title_2)
                        .color(p.text)
                        .strong(),
                );
                ui.add_space(sp.xs);
                if let Some(handle) = &state.handle {
                    dim_label(ui, th, &format!("@{handle}"));
                }
                if let Some(did) = &state.did {
                    let short = if did.len() > 28 {
                        format!("{}…", &did[..28])
                    } else {
                        did.clone()
                    };
                    dim_label(ui, th, &short);
                } else {
                    dim_label(ui, th, "Guest · not authenticated");
                }
            });
        });
    });

    ui.add_space(sp.md);

    // Connection
    card(ui, th, |ui| {
        section_label(ui, th, "CONNECTION");
        ui.add_space(sp.md);
        row(ui, th, "Server", &state.server);
        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            body(ui, th, "Status");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let live = state.connection.is_live();
                status_dot(ui, th, live);
                ui.add_space(4.0);
                body(ui, th, state.connection.label());
            });
        });
        ui.add_space(sp.sm);
        row(
            ui,
            th,
            "Transport",
            if state.use_websocket {
                "WebSocket"
            } else if state.use_tls {
                "TLS"
            } else {
                "TCP"
            },
        );
        if !state.status_line.is_empty() {
            ui.add_space(sp.sm);
            dim_label(ui, th, &state.status_line);
        }
    });

    ui.add_space(sp.md);

    // Chat
    card(ui, th, |ui| {
        section_label(ui, th, "CHAT");
        ui.add_space(sp.md);
        let prev = state.show_join_part;
        checkbox(
            ui,
            th,
            &mut state.show_join_part,
            "Show join/part/quit messages",
        );
        if state.show_join_part != prev {
            state.persist_prefs();
        }
        ui.add_space(sp.sm);
        dim_label(
            ui,
            th,
            "When off, presence lines stay out of the buffer. Member lists still update.",
        );
    });

    ui.add_space(sp.md);

    // Appearance
    card(ui, th, |ui| {
        section_label(ui, th, "APPEARANCE");
        ui.add_space(sp.md);
        ui.horizontal(|ui| {
            body(ui, th, "Shell");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = match mode {
                    Mode::Dark => " Dark ",
                    Mode::Light => " Light ",
                };
                if button(ui, th, label).clicked() {
                    action = SettingsAction::ToggleTheme;
                }
            });
        });
        ui.add_space(sp.sm);
        dim_label(ui, th, "Vidya theme · GNOME/HIG-inspired");
    });

    ui.add_space(sp.md);

    // About
    card(ui, th, |ui| {
        section_label(ui, th, "ABOUT");
        ui.add_space(sp.md);
        title_2(ui, th, "Sleek");
        ui.add_space(sp.xs);
        body(
            ui,
            th,
            "A calm freeq client for phones — egui + Vidya, freeq-sdk under the hood.",
        );
        ui.add_space(sp.sm);
        dim_label(ui, th, "v0.1 · MIT · nandi.uk");
    });

    ui.add_space(sp.lg);

    if state.connection != ConnectionState::Disconnected {
        if destructive_button(ui, th, "Disconnect").clicked() {
            action = SettingsAction::Disconnect;
        }
        ui.add_space(sp.sm);
    }

    // Available connected or not — clears disk session so guest connect is real.
    if state.has_cached_identity() {
        if destructive_button(ui, th, "Clear saved account").clicked() {
            action = SettingsAction::Logout;
        }
        ui.add_space(sp.xs);
        dim_label(
            ui,
            th,
            "Removes broker session, DID, and handle. Next connect is a pure guest.",
        );
    } else if state.has_saved_guest() {
        if destructive_button(ui, th, "Forget saved guest").clicked() {
            action = SettingsAction::Logout;
        }
        ui.add_space(sp.xs);
        dim_label(
            ui,
            th,
            "Stops auto-connecting as this guest nick on next launch.",
        );
    } else if state.connection == ConnectionState::Disconnected {
        // Shell already shows connect when offline with no session.
        let _ = primary_button(ui, th, "Back to connect");
    }

    action
}

fn row(ui: &mut egui::Ui, th: &Theme, key: &str, value: &str) {
    ui.horizontal(|ui| {
        body(ui, th, key);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            dim_label(ui, th, value);
        });
    });
}
