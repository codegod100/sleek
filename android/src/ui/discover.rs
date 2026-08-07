//! Discover tab — popular channels + custom join.

use eframe::egui::{self, Align, CursorIcon, Layout, RichText};
use vidya::{body, dim_label, primary_button, title, title_2, Theme};

use crate::state::{AppState, POPULAR_CHANNELS};
use crate::ui::widgets::{avatar_circle, card};

pub enum DiscoverAction {
    None,
    Join(String),
    /// Already a member — open the channel buffer.
    Open(String),
}

pub fn discover_tab(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> DiscoverAction {
    let mut action = DiscoverAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    title(ui, th, "Discover");
    ui.add_space(sp.xs);
    dim_label(ui, th, "Find channels on this freeq network");
    ui.add_space(sp.lg);

    card(ui, th, |ui| {
        title_2(ui, th, "Join a channel");
        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            let field_w = (ui.available_width() - 88.0).max(80.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.discover_input)
                    .margin(th.text_edit_margin())
                    .desired_width(field_w)
                    .min_size(egui::vec2(0.0, th.spacing.control_height))
                    .hint_text("#channel"),
            );
            // singleline TextEdit surrenders focus on Enter — join when that happens.
            let enter = resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(sp.sm);
            let join_clicked = primary_button(ui, th, "Join").clicked();
            if join_clicked || enter {
                let ch = AppState::normalize_channel(&state.discover_input);
                if !ch.is_empty() {
                    state.discover_input.clear();
                    action = DiscoverAction::Join(ch);
                }
            }
        });
    });

    ui.add_space(sp.lg);
    title_2(ui, th, "Popular");
    ui.add_space(sp.md);

    for (name, blurb) in POPULAR_CHANNELS {
        // Only treat successful membership as joined (not pending / denied).
        let joined = state.channels.values().any(|b| {
            b.name.eq_ignore_ascii_case(name) && b.is_joined()
        });

        let resp = egui::Frame::new()
            .fill(p.card_bg)
            .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
            .corner_radius(sp.radius_md)
            .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.md as i8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    avatar_circle(ui, th, name, 36.0);
                    ui.add_space(sp.md);
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 72.0).max(40.0));
                        ui.label(
                            RichText::new(*name)
                                .size(th.type_scale.body)
                                .color(p.text)
                                .strong(),
                        );
                        ui.add_space(2.0);
                        dim_label(ui, th, blurb);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if joined {
                            dim_label(ui, th, "Joined");
                        } else {
                            body(ui, th, "Join");
                        }
                    });
                });
            })
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);

        if resp.clicked() {
            action = if joined {
                DiscoverAction::Open((*name).to_string())
            } else {
                DiscoverAction::Join((*name).to_string())
            };
        }
        if resp.hovered() {
            ui.painter().rect_filled(
                resp.rect,
                sp.radius_md,
                p.button_hover.gamma_multiply(0.4),
            );
        }
        ui.add_space(sp.sm);
    }

    action
}
