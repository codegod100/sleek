//! Chats tab — conversation list (freeq-android ChatsTab inspired).

use eframe::egui::{self, Align, Layout};
use vidya::{button, dim_label, primary_button, title, title_2, Theme};

use crate::state::AppState;
use crate::ui::widgets::{card, conversation_row, empty_state, text_edit_clipboard_menu};

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
    let resp = ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .margin(th.text_edit_margin())
            .desired_width(ui.available_width())
            .min_size(egui::vec2(0.0, th.spacing.control_height))
            .hint_text("Search chats"),
    );
    text_edit_clipboard_menu(ui, th, &resp);
    ui.add_space(sp.md);

    // Quick join
    card(ui, th, |ui| {
        title_2(ui, th, "Join channel");
        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            let field_w = (ui.available_width() - 88.0).max(80.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.join_input)
                    .margin(th.text_edit_margin())
                    .desired_width(field_w)
                    .min_size(egui::vec2(0.0, th.spacing.control_height))
                    .hint_text("#channel"),
            );
            text_edit_clipboard_menu(ui, th, &resp);
            // singleline TextEdit surrenders focus on Enter — join when that happens.
            let enter = resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(sp.sm);
            let join_clicked = primary_button(ui, th, "Join").clicked();
            if join_clicked || enter {
                let ch = AppState::normalize_channel(&state.join_input);
                if !ch.is_empty() {
                    state.join_input.clear();
                    action = ChatsAction::Join(ch);
                }
            }
        });
    });
    ui.add_space(sp.md);

    let conversations: Vec<(String, String, bool, u32, String, String, i64)> = state
        .sorted_conversations()
        .into_iter()
        .map(|b| {
            (
                b.name.clone(),
                state.display_name_for(&b.name),
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

    let n = conversations.len();
    for (i, (name, display, selected, _unread, _preview, _topic, _)) in
        conversations.iter().enumerate()
    {
        // Re-borrow buffer for rendering
        let row_rect = if let Some(buf) = state.channels.get(name) {
            let resp = conversation_row(ui, th, buf, display, *selected);
            if resp.clicked() {
                action = ChatsAction::Open(name.clone());
            }
            Some(resp.rect)
        } else {
            let _ = selected;
            None
        };

        // Soft divider between rows — same horizontal span as the row hover/fill.
        if i + 1 < n {
            if let Some(rect) = row_rect {
                ui.add_space(sp.xs);
                let y = ui.cursor().top();
                ui.painter().hline(
                    rect.left()..=rect.right(),
                    y,
                    egui::Stroke::new(1.0_f32, p.border_soft),
                );
                ui.add_space(sp.xs);
            } else {
                ui.add_space(sp.xs);
            }
        }
    }

    action
}
