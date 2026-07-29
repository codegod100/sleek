//! Connect / guest login screen (freeq-android ConnectScreen inspired).

use eframe::egui::{self, Align, Layout, RichText};
use vidya::{
    body, button, checkbox, dim_label, primary_button, text_field_singleline, title, title_2, Theme,
};

use crate::state::{AppState, ConnectionState};
use crate::ui::widgets::card;

pub fn connect_screen(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> ConnectAction {
    let mut action = ConnectAction::None;
    let sp = &th.spacing;
    let p = &th.palette;
    let loading = state.connection == ConnectionState::Connecting;

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
        title_2(ui, th, "Guest connect");
        ui.add_space(sp.sm);
        body(
            ui,
            th,
            "Join freeq as a guest. Your nick is a display alias — sign in with Bluesky later for a portable DID.",
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
            let label = if loading { "Connecting…" } else { "Connect" };
            let resp = primary_button(ui, th, label);
            if resp.clicked() && !loading {
                action = ConnectAction::Connect;
            }
        });
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
    Connect,
}
