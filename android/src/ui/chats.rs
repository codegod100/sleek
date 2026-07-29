//! Chats tab — conversation list (freeq-android ChatsTab inspired).

use eframe::egui::{self, Align, Layout};
use vidya::{button, dim_label, primary_button, text_field_singleline, title, title_2, Theme};

use crate::state::AppState;
use crate::ui::widgets::{card, conversation_row, empty_state};

pub enum ChatsAction {
    None,
    Open(String),
    Join(String),
}

pub fn chats_tab(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> ChatsAction {
    let mut action = ChatsAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    // Header row
    ui.horizontal(|ui| {
        title(ui, th, "Chats");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            dim_label(ui, th, state.connection.label());
        });
    });
    ui.add_space(sp.md);

    // Search
    ui.horizontal(|ui| {
        ui.set_width(ui.available_width());
        let _ = text_field_singleline(ui, th, &mut state.search);
    });
    if state.search.is_empty() {
        // Hint sits above; field itself has no placeholder API on helper —
        // show dim hint when empty via overlaid caption.
    }
    ui.add_space(sp.xs);
    dim_label(ui, th, "Search chats");
    ui.add_space(sp.md);

    // Quick join
    card(ui, th, |ui| {
        title_2(ui, th, "Join channel");
        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            let field_w = (ui.available_width() - 88.0).max(80.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.join_input)
                    .margin(th.text_edit_margin())
                    .desired_width(field_w)
                    .min_size(egui::vec2(0.0, th.spacing.control_height))
                    .hint_text("#channel"),
            );
            ui.add_space(sp.sm);
            if primary_button(ui, th, "Join").clicked() {
                let ch = AppState::normalize_channel(&state.join_input);
                if !ch.is_empty() {
                    state.join_input.clear();
                    action = ChatsAction::Join(ch);
                }
            }
        });
    });
    ui.add_space(sp.md);

    let conversations: Vec<(String, bool, u32, String, String, i64)> = state
        .sorted_conversations()
        .into_iter()
        .map(|b| {
            (
                b.name.clone(),
                state
                    .active_channel
                    .as_ref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&b.name)),
                b.unread,
                b.last_preview(),
                b.topic.clone(),
                b.last_activity,
            )
        })
        .collect();

    if conversations.is_empty() {
        empty_state(
            ui,
            th,
            "No chats yet",
            "Join a channel from Discover or use the field above.",
        );
        ui.add_space(sp.md);
        if button(ui, th, "Open Discover").clicked() {
            state.tab = crate::state::Tab::Discover;
        }
        return action;
    }

    for (name, selected, _unread, _preview, _topic, _) in &conversations {
        // Re-borrow buffer for rendering
        if let Some(buf) = state.channels.get(name) {
            let resp = conversation_row(ui, th, buf, *selected);
            if resp.clicked() {
                action = ChatsAction::Open(name.clone());
            }
        } else {
            let _ = selected;
        }
        ui.add_space(sp.xs);

        // Soft divider
        let w = ui.available_width();
        let y = ui.cursor().top();
        ui.painter().hline(
            ui.min_rect().left() + sp.md..=ui.min_rect().left() + w - sp.md,
            y,
            egui::Stroke::new(1.0_f32, p.border_soft),
        );
    }

    action
}
