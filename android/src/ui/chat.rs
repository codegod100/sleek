//! Chat detail — messages + compose (freeq-android ChatDetailScreen inspired).

use eframe::egui::{self, Align, Layout, RichText, ScrollArea};
use vidya::{button, dim_label, primary_button, title_2, Theme};

use crate::state::AppState;
use crate::ui::widgets::message_bubble;

pub enum ChatAction {
    None,
    Back,
    Send { target: String, text: String },
    Part(String),
}

pub fn chat_screen(ui: &mut egui::Ui, th: &Theme, state: &mut AppState, channel: &str) -> ChatAction {
    let mut action = ChatAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    // Snapshot buffer data we need (avoid holding borrow across mut compose)
    let (topic, member_count, messages, is_channel) = {
        let buf = state.channels.get(channel);
        match buf {
            Some(b) => (
                b.topic.clone(),
                b.members.len(),
                b.messages.iter().cloned().collect::<Vec<_>>(),
                b.is_channel(),
            ),
            None => (String::new(), 0, Vec::new(), channel.starts_with('#')),
        }
    };
    let own_nick = state.nick.clone();

    // Header
    ui.horizontal(|ui| {
        if button(ui, th, "←").clicked() {
            action = ChatAction::Back;
        }
        ui.add_space(sp.sm);
        ui.vertical(|ui| {
            title_2(ui, th, channel);
            if is_channel {
                let sub = if topic.is_empty() {
                    format!("{member_count} members")
                } else {
                    let t = if topic.chars().count() > 42 {
                        format!("{}…", topic.chars().take(42).collect::<String>())
                    } else {
                        topic.clone()
                    };
                    t
                };
                dim_label(ui, th, &sub);
            } else {
                dim_label(ui, th, "Direct message");
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if is_channel && button(ui, th, "Leave").clicked() {
                action = ChatAction::Part(channel.to_string());
            }
        });
    });
    ui.add_space(sp.sm);
    ui.separator();
    ui.add_space(sp.sm);

    // Messages
    let msg_h = (ui.available_height() - th.spacing.control_height - 56.0).max(120.0);
    ScrollArea::vertical()
        .id_salt("chat_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .max_height(msg_h)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if messages.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    dim_label(ui, th, "No messages yet — say hello.");
                });
            } else {
                for msg in &messages {
                    message_bubble(ui, th, msg, &own_nick);
                    ui.add_space(sp.sm);
                }
            }
            ui.add_space(sp.md);
        });

    ui.add_space(sp.sm);

    // Compose bar
    egui::Frame::new()
        .fill(p.headerbar_bg)
        .stroke(egui::Stroke::new(1.0, p.border_soft))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.sm as i8, sp.sm as i8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                let send_w = 72.0_f32;
                let field_w = (ui.available_width() - send_w - sp.sm).max(60.0);
                let te = egui::TextEdit::multiline(&mut state.compose)
                    .margin(th.text_edit_margin())
                    .desired_width(field_w)
                    .desired_rows(1)
                    .hint_text("Message…");
                let resp = ui.add(te);
                let enter = resp.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

                ui.add_space(sp.sm);
                let send_clicked = primary_button(ui, th, "Send").clicked();

                if (send_clicked || enter) && !state.compose.trim().is_empty() {
                    let text = state.compose.trim().to_string();
                    // strip trailing newline from enter
                    let text = text.trim_end_matches('\n').to_string();
                    if !text.is_empty() {
                        state.compose.clear();
                        action = ChatAction::Send {
                            target: channel.to_string(),
                            text,
                        };
                    }
                }
            });
        });

    // Suppress unused warning when leave not used
    let _ = RichText::new("");

    action
}
