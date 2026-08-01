//! Message search overlay (freeq-android SearchSheet inspired).

use chrono::{Duration, Local};
use eframe::egui::{self, Align, CursorIcon, Layout, RichText, ScrollArea, Sense};
use egui::text::{LayoutJob, TextFormat};
use vidya::{button, dim_label, lead_trail, Theme};

use crate::state::{AppState, MessageSearchHit};

pub enum SearchAction {
    None,
    /// Open `channel` and scroll to `msgid`.
    Open { channel: String, msgid: String },
}

/// Full-panel message search (replaces the chat body while open).
pub fn message_search_panel(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> SearchAction {
    let mut action = SearchAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    // Allocate Close first so the field cannot squeeze it into a vertical strip.
    // IMPORTANT: pin the row height. `lead_trail` centers in the full available
    // height, so an unconstrained call expands to the whole panel and pushes the
    // results list below the clip rect (clicks miss, search looks broken).
    let want_focus = state.focus_message_search;
    state.focus_message_search = false;
    let mut close = false;
    let row_h = th.spacing.control_height + 8.0;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width().max(1.0), row_h),
        Layout::left_to_right(Align::Center),
        |ui| {
            lead_trail(
                ui,
                |ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut state.message_search)
                            .id_salt("message_search_field")
                            .margin(th.text_edit_margin())
                            .desired_width(ui.available_width().max(1.0))
                            .min_size(egui::vec2(0.0, th.spacing.control_height))
                            .hint_text("Search messages…"),
                    );
                    if want_focus {
                        resp.request_focus();
                    }
                },
                |ui| {
                    if button(ui, th, "Close").clicked() {
                        close = true;
                    }
                },
            );
        },
    );
    if close {
        state.close_message_search();
    }
    ui.add_space(sp.sm);

    let query = state.message_search.clone();
    let q_len = query.trim().chars().count();

    if q_len < 2 {
        ui.vertical_centered(|ui| {
            ui.add_space(sp.xl);
            dim_label(ui, th, "Type at least 2 characters to search");
        });
        return action;
    }

    let results = state.search_messages(&query);
    if results.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(sp.xl);
            dim_label(ui, th, "No results found");
        });
        return action;
    }

    let list_h = ui.available_height().max(120.0);
    // drag_to_scroll off: background drag sense otherwise competes with row clicks.
    ScrollArea::vertical()
        .id_salt("message_search_scroll")
        .drag_to_scroll(false)
        .auto_shrink([false, false])
        .max_height(list_h)
        .min_scrolled_height(64.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let n = results.len();
            for (i, hit) in results.iter().enumerate() {
                let label = state.display_name_for(&hit.channel);
                let resp = search_result_row(ui, th, hit, &query, &label);
                // Tests (and debug) read the first hit's clickable rect.
                if i == 0 {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("sleek_search_first_hit_rect"), resp.rect);
                    });
                }
                if resp.clicked() {
                    action = SearchAction::Open {
                        channel: hit.channel.clone(),
                        msgid: hit.message.id.clone(),
                    };
                }
                if i + 1 < n {
                    ui.add_space(sp.xs);
                    let y = ui.cursor().top();
                    let w = ui.available_width();
                    ui.painter().hline(
                        ui.max_rect().left()..=ui.max_rect().left() + w,
                        y,
                        egui::Stroke::new(1.0_f32, p.border_soft.gamma_multiply(0.6)),
                    );
                    ui.add_space(sp.xs);
                }
            }
            ui.add_space(sp.md);
        });

    action
}

fn search_result_row(
    ui: &mut egui::Ui,
    th: &Theme,
    hit: &MessageSearchHit,
    query: &str,
    channel_label: &str,
) -> egui::Response {
    let p = &th.palette;
    let sp = &th.spacing;

    // Same clickable-row pattern as conversation / member list: draw the
    // frame, then `ui.interact` over the rect so labels cannot eat the click.
    let row_id = ui
        .id()
        .with("search_hit")
        .with(hit.message.id.as_str())
        .with(hit.channel.as_str());
    let hovered = ui
        .ctx()
        .read_response(row_id)
        .is_some_and(|r| r.hovered());
    let fill = if hovered {
        p.button_hover.gamma_multiply(0.45)
    } else {
        egui::Color32::TRANSPARENT
    };

    let inner = egui::Frame::new()
        .fill(fill)
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(sp.sm as i8, sp.sm as i8))
        .show(ui, |ui| {
            let w = ui.available_width();
            ui.set_min_width(w);
            ui.set_max_width(w);

            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(channel_label)
                            .size(th.type_scale.caption)
                            .color(p.accent)
                            .strong(),
                    )
                    .selectable(false),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format_search_time(hit.message.timestamp))
                                .size(th.type_scale.caption)
                                .color(p.text_secondary),
                        )
                        .selectable(false),
                    );
                });
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&hit.message.from)
                            .size(th.type_scale.body)
                            .color(p.accent)
                            .strong(),
                    )
                    .selectable(false),
                );
                ui.add_space(sp.xs);
                let job = highlight_query(&hit.message.text, query, th);
                ui.add(egui::Label::new(job).wrap().selectable(false));
            });
        });

    ui.interact(inner.response.rect, row_id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn highlight_query(text: &str, query: &str, th: &Theme) -> LayoutJob {
    let p = &th.palette;
    let size = th.type_scale.body;
    let base = TextFormat {
        font_id: egui::FontId::proportional(size),
        color: p.text,
        ..Default::default()
    };
    let mark = TextFormat {
        font_id: egui::FontId::proportional(size),
        color: p.accent,
        underline: egui::Stroke::new(1.0, p.accent.gamma_multiply(0.7)),
        ..Default::default()
    };

    let mut job = LayoutJob::default();
    let q = query.trim();
    if q.is_empty() {
        job.append(text, 0.0, base);
        return job;
    }

    // Match on lowercase copies but slice `text` via char indices so a
    // case-fold length change (e.g. İ → i̇) can't panic or mis-slice.
    let lower_chars: Vec<char> = text.to_lowercase().chars().collect();
    let q_chars: Vec<char> = q.to_lowercase().chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    if lower_chars.len() != text_chars.len() || q_chars.is_empty() {
        job.append(text, 0.0, base);
        return job;
    }

    let mut start = 0usize;
    while start < text_chars.len() {
        let Some(rel) = lower_chars[start..]
            .windows(q_chars.len())
            .position(|w| w == q_chars.as_slice())
        else {
            let rest: String = text_chars[start..].iter().collect();
            job.append(&rest, 0.0, base);
            break;
        };
        let idx = start + rel;
        if idx > start {
            let before: String = text_chars[start..idx].iter().collect();
            job.append(&before, 0.0, base.clone());
        }
        let end = idx + q_chars.len();
        let hit: String = text_chars[idx..end].iter().collect();
        job.append(&hit, 0.0, mark.clone());
        start = end;
    }
    job
}

fn format_search_time(ts: chrono::DateTime<Local>) -> String {
    let now = Local::now();
    let today = now.date_naive();
    let day = ts.date_naive();
    if day == today {
        ts.format("%-I:%M %p").to_string()
    } else if day + Duration::days(1) == today {
        "Yesterday".into()
    } else {
        ts.format("%-m/%-d/%y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use eframe::egui::{
        self, CentralPanel, Event, FontDefinitions, PointerButton, Pos2, RawInput, Rect, Vec2,
    };
    use std::collections::HashMap;
    use vidya::Theme;

    use crate::state::{AppState, ChatMessage, Route};

    fn sample_msg(id: &str, from: &str, text: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            from: from.into(),
            text: text.into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        }
    }

    fn base_input(size: Vec2) -> RawInput {
        let rect = Rect::from_min_size(Pos2::ZERO, size);
        let mut input = RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        if let Some(vp) = input.viewports.get_mut(&input.viewport_id) {
            vp.inner_rect = Some(rect);
        }
        input
    }

    /// Click press+release at `pos` over successive frames while rendering `ui_fn`.
    fn click_at(
        ctx: &egui::Context,
        size: Vec2,
        pos: Pos2,
        mut ui_fn: impl FnMut(&mut egui::Ui) -> SearchAction,
    ) -> SearchAction {
        let mut last = SearchAction::None;

        // Hover / move onto the target.
        let mut input = base_input(size);
        input.events.push(Event::PointerMoved(pos));
        let _ = ctx.run(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                last = ui_fn(ui);
            });
        });

        // Press.
        let mut input = base_input(size);
        input.events.push(Event::PointerMoved(pos));
        input.events.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        let _ = ctx.run(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                last = ui_fn(ui);
            });
        });

        // Release — this is when `clicked()` becomes true.
        let mut input = base_input(size);
        input.events.push(Event::PointerMoved(pos));
        input.events.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        let _ = ctx.run(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                last = ui_fn(ui);
            });
        });

        last
    }

    #[test]
    fn search_panel_click_navigates_to_hit() {
        let ctx = egui::Context::default();
        // Keep default fonts so text layout produces real row heights.
        let _ = FontDefinitions::default();

        let mut state = AppState::new();
        state.ensure_buffer("#room");
        state
            .channels
            .get_mut("#room")
            .unwrap()
            .append(sample_msg("hit-1", "alice", "unique-needle-xyz"));
        state
            .channels
            .get_mut("#room")
            .unwrap()
            .append(sample_msg("other", "bob", "something else"));

        state.open_message_search();
        state.message_search = "unique-needle".into();

        let th = Theme::dark();
        let size = Vec2::new(400.0, 700.0);

        // Warm up a few frames so the result row exists and has a stable rect.
        let mut hit_rect = Rect::NOTHING;
        for _ in 0..3 {
            let input = base_input(size);
            let _ = ctx.run(input, |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    let _ = message_search_panel(ui, &th, &mut state);
                    hit_rect = ctx.data(|d| {
                        d.get_temp::<Rect>(egui::Id::new("sleek_search_first_hit_rect"))
                            .unwrap_or(Rect::NOTHING)
                    });
                });
            });
        }

        assert!(
            hit_rect.height() > 1.0,
            "expected a laid-out search hit row, got {hit_rect:?}"
        );
        let click_pos = hit_rect.center();

        let action = click_at(&ctx, size, click_pos, |ui| {
            message_search_panel(ui, &th, &mut state)
        });

        match action {
            SearchAction::Open { channel, msgid } => {
                assert_eq!(channel, "#room");
                assert_eq!(msgid, "hit-1");
                state.navigate_to_message(&channel, &msgid);
            }
            SearchAction::None => panic!(
                "search result click produced no Open action (click_pos={click_pos:?}, hit_rect={hit_rect:?})"
            ),
        }

        assert!(matches!(state.route, Route::Chat(ref c) if c == "#room"));
        assert!(!state.show_message_search);
        assert_eq!(state.scroll_to_msgid.as_deref(), Some("hit-1"));
        assert_eq!(state.highlight_msgid.as_deref(), Some("hit-1"));
    }

    #[test]
    fn search_messages_then_navigate_sets_scroll_target() {
        let mut state = AppState::new();
        state.ensure_buffer("#a");
        state.ensure_buffer("#b");
        state
            .channels
            .get_mut("#a")
            .unwrap()
            .append(sample_msg("1", "alice", "hello world"));
        state
            .channels
            .get_mut("#b")
            .unwrap()
            .append(sample_msg("2", "carol", "HELLO again"));

        let hits = state.search_messages("hello");
        assert_eq!(hits.len(), 2);

        state.open_chat("#a");
        state.open_message_search();
        let hit = &hits[0];
        state.navigate_to_message(&hit.channel, &hit.message.id);
        assert!(!state.show_message_search);
        assert_eq!(state.scroll_to_msgid.as_deref(), Some(hit.message.id.as_str()));
        assert_eq!(state.highlight_msgid.as_deref(), Some(hit.message.id.as_str()));
    }
}
