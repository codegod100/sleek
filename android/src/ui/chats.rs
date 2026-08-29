//! Chats tab — conversation list (freeq-android ChatsTab inspired).

use eframe::egui;
use vidya::{button, primary_button, title_2, Theme};

use crate::state::AppState;
use crate::ui::widgets::{card, conversation_row, empty_state, text_edit_clipboard_menu};

pub enum ChatsAction {
    None,
    Open(String),
    Join(String),
}

pub fn chats_tab(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    pinned_join: bool,
) -> ChatsAction {
    let mut action = ChatsAction::None;
    let sp = &th.spacing;
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

    let search_resp = ui.add(
        egui::TextEdit::singleline(&mut state.channel_search)
            .margin(th.text_edit_margin())
            .desired_width(f32::INFINITY)
            .min_size(egui::vec2(0.0, th.spacing.control_height))
            .hint_text("Search channels"),
    );
    text_edit_clipboard_menu(ui, th, &search_resp);
    ui.add_space(sp.md);

    if pinned_join {
        // In the wide master panel, keep the join and search controls pinned
        // and scroll only the conversation list. The narrow layout already
        // has one outer scroll area, so do not nest another scrollbar there.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("chats_conversation_scroll")
            .show(ui, |ui| action = chats_list(ui, th, state));
    } else {
        action = chats_list(ui, th, state);
    }

    action
}

pub fn chats_list(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> ChatsAction {
    let mut action = ChatsAction::None;
    let sp = &th.spacing;
    let p = &th.palette;
    let query = state.channel_search.trim().to_lowercase();
    let conversations: Vec<(String, String, bool, u32, String, String, i64)> = state
        .sorted_conversations()
        .into_iter()
        .filter_map(|b| {
            let display = state.display_name_for(&b.name);
            if !query.is_empty()
                && !b.name.to_lowercase().contains(&query)
                && !display.to_lowercase().contains(&query)
            {
                return None;
            }
            Some((
                b.name.clone(),
                display,
                state
                    .active_channel
                    .as_ref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&b.name)),
                b.unread,
                b.last_preview(),
                b.topic.clone(),
                b.last_activity,
            ))
        })
        .collect();

    if conversations.is_empty() {
        if query.is_empty() {
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
        } else {
            empty_state(
                ui,
                th,
                "No matching channels",
                "Try a different channel name.",
            );
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

/// Empty detail pane for the wide master–detail shell (no chat selected).
pub fn chat_detail_placeholder(ui: &mut egui::Ui, th: &Theme) {
    empty_state(
        ui,
        th,
        "Select a chat",
        "Pick a conversation from the list, or join a channel above.",
    );
}
