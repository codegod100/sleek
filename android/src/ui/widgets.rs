//! Shared Vidya-styled widgets.

use eframe::egui::{
    self, text::LayoutJob, text::TextFormat, Align, Color32, CursorIcon, FontId, Layout, Rect,
    RichText, ScrollArea, Sense, Stroke, Vec2,
};
use vidya::{dim_label, icon_colored, paint_emoji_in, title_2, Icon, Theme};

use crate::preview::{self, Embed, UrlSpan};
use crate::state::{
    display_emoji, emoji_matches_search, AppState, Buffer, ChatMessage, EmojiPickerGroup,
    ImageState, LinkMeta, LinkState, MediaCache, DEFAULT_REACT_EMOJI, EMOJI_SEARCH_LIMIT,
    QUICK_REACT_EMOJIS,
};

/// Interaction from a chat message bubble.
#[derive(Debug, Clone, Default)]
pub enum MessageBubbleAction {
    #[default]
    None,
    /// Toggle (add/remove) our reaction on this message.
    ToggleReaction { msgid: String, emoji: String },
    /// Open the full emoji reaction picker for this message.
    OpenReactPicker { msgid: String },
    /// Dismiss the emoji reaction picker.
    CloseReactPicker,
    /// Open the full-screen image lightbox for an embed URL.
    OpenImage { url: String },
    /// Begin editing this message in the compose bar.
    Edit { msgid: String, text: String },
    /// Soft-delete this message (`+draft/delete`).
    Delete { msgid: String },
}

/// Card frame filling parent width.
pub fn card(ui: &mut egui::Ui, th: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    let outer = ui.available_width();
    ui.set_max_width(outer);
    th.card_frame().show(ui, |ui| {
        let inner = ui.available_width().max(1.0);
        ui.set_min_width(inner);
        ui.set_max_width(inner);
        add(ui);
    });
}

pub fn section_label(ui: &mut egui::Ui, th: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .size(th.type_scale.caption)
            .color(th.palette.text_secondary)
            .strong(),
    );
}

/// Colored initial circle (freeq-style avatar stand-in).
pub fn avatar_circle(ui: &mut egui::Ui, th: &Theme, name: &str, size: f32) {
    let letter = name
        .trim_start_matches(['#', '&', '@', '+', '%'])
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());

    let hue = hash_hue(name);
    let fill = Color32::from_rgb(
        ((hue * 0.6 + 40.0) as u8).saturating_add(30),
        ((hue * 0.4 + 80.0) as u8).min(200),
        ((200.0 - hue * 0.3) as u8).max(80),
    );

    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size * 0.5, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional((size * 0.42).max(10.0)),
        th.palette.accent_fg,
    );
}

fn hash_hue(s: &str) -> f32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    (h % 160) as f32
}

/// One row in the chats list.
///
/// `display_name` is the human label (nick for DID-keyed DMs); `buf.name` stays
/// the stable buffer key.
pub fn conversation_row(
    ui: &mut egui::Ui,
    th: &Theme,
    buf: &Buffer,
    display_name: &str,
    selected: bool,
) -> egui::Response {
    let p = &th.palette;
    let sp = &th.spacing;

    // Stable id so we can read last-frame hover and paint fill *under* content.
    let row_id = ui.id().with("conv_row").with(&buf.name);
    let hovered = ui
        .ctx()
        .read_response(row_id)
        .is_some_and(|r| r.hovered());

    let fill = if selected {
        p.accent.gamma_multiply(0.22)
    } else if hovered {
        p.button_hover.gamma_multiply(0.45)
    } else {
        Color32::TRANSPARENT
    };

    // List rows share one horizontal band with dividers — full width, modest radius.
    let inner = egui::Frame::new()
        .fill(fill)
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2))
        .show(ui, |ui| {
            // Force full available width so hover/selected match divider ends.
            let w = ui.available_width();
            ui.set_min_width(w);
            ui.set_max_width(w);
            ui.horizontal(|ui| {
                avatar_circle(ui, th, display_name, 40.0);
                ui.add_space(sp.md);
                ui.vertical(|ui| {
                    ui.set_min_width((ui.available_width()).max(40.0));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(display_name)
                                .size(th.type_scale.body)
                                .color(p.text)
                                .strong(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if buf.unread > 0 {
                                badge(ui, th, &format!("{}", buf.unread.min(99)));
                            } else if buf.call.is_some() {
                                ui.label(
                                    RichText::new("📞")
                                        .size(th.type_scale.caption)
                                        .color(p.accent),
                                );
                            }
                        });
                    });
                    ui.add_space(2.0);
                    let preview = buf.last_preview();
                    let truncated = if preview.chars().count() > 48 {
                        let t: String = preview.chars().take(48).collect();
                        format!("{t}…")
                    } else {
                        preview
                    };
                    ui.label(
                        RichText::new(truncated)
                            .size(th.type_scale.caption)
                            .color(p.text_secondary),
                    );
                });
            });
        });

    ui.interact(inner.response.rect, row_id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn badge(ui: &mut egui::Ui, th: &Theme, text: &str) {
    let p = &th.palette;
    egui::Frame::new()
        .fill(p.accent)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(th.type_scale.caption)
                    .color(p.accent_fg)
                    .strong(),
            );
        });
}

/// Message bubble / row in chat detail (with optional image / OG link embed).
///
/// `react_picker_open` highlights the react control while the modal picker
/// ([`react_picker_overlay`]) is open for this message.
pub fn message_bubble(
    ui: &mut egui::Ui,
    th: &Theme,
    msg: &ChatMessage,
    own_nick: &str,
    media: &mut MediaCache,
    react_picker_open: bool,
    highlighted: bool,
) -> MessageBubbleAction {
    let p = &th.palette;
    let sp = &th.spacing;
    let is_own = !msg.is_system && msg.from.eq_ignore_ascii_case(own_nick);
    let mut action = MessageBubbleAction::None;

    if msg.is_system {
        ui.horizontal(|ui| {
            ui.add_space(sp.sm);
            dim_label(ui, th, &format!("· {}", msg.text));
        });
        return action;
    }

    if msg.is_deleted {
        dim_label(ui, th, "Message deleted");
        return action;
    }

    let body_text = if msg.is_action {
        format!("* {} {}", msg.from, msg.text)
    } else {
        msg.text.clone()
    };

    let bg = if highlighted {
        p.accent.gamma_multiply(0.38)
    } else if is_own {
        p.accent.gamma_multiply(0.28)
    } else {
        p.card_bg
    };
    let stroke = if highlighted {
        Stroke::new(2.0_f32, p.accent)
    } else {
        Stroke::new(1.0_f32, p.border_soft)
    };

    let embed = msg.resolved_embed();
    let can_mutate =
        !msg.id.is_empty() && !msg.id.starts_with("local-") && !msg.id.starts_with("sys-");
    let can_react = can_mutate;
    let can_edit = can_mutate && is_own;
    let can_delete = can_mutate && is_own;

    // Body frame only — reaction chips live *below* so bubble gestures can't
    // steal their clicks (later full-rect interacts would otherwise win).
    let frame_resp = egui::Frame::new()
        .fill(bg)
        .stroke(stroke)
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                if !is_own {
                    ui.label(
                        RichText::new(&msg.from)
                            .size(th.type_scale.caption)
                            .color(p.accent)
                            .strong(),
                    );
                } else {
                    ui.label(
                        RichText::new("You")
                            .size(th.type_scale.caption)
                            .color(p.accent)
                            .strong(),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if can_react {
                        let react_size = (th.type_scale.caption * 1.2).max(16.0);
                        let react_color = if react_picker_open {
                            p.accent
                        } else {
                            p.text_secondary
                        };
                        let (r, react_resp) =
                            ui.allocate_exact_size(Vec2::splat(react_size + 4.0), Sense::click());
                        paint_emoji_in(ui, r.shrink(2.0), "😂", react_color);
                        let react_btn = react_resp
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text("Add reaction");
                        if react_btn.clicked() {
                            action = if react_picker_open {
                                MessageBubbleAction::CloseReactPicker
                            } else {
                                MessageBubbleAction::OpenReactPicker {
                                    msgid: msg.id.clone(),
                                }
                            };
                        }
                        ui.add_space(sp.xs);
                    }
                    let mut meta = msg.time_label();
                    if msg.is_edited {
                        meta = format!("edited · {meta}");
                    }
                    if msg.is_signed {
                        meta = format!("✓ {meta}");
                    }
                    dim_label(ui, th, &meta);
                });
            });
            ui.add_space(sp.xs);
            // Sense on the body — URL tap opens browser; double-click heart;
            // right-click / long-press opens the context menu.
            let url_spans = preview::extract_url_spans(&body_text);
            let mut job = linkify_layout_job(&body_text, &url_spans, th);
            job.wrap.max_width = ui.available_width();
            let galley = ui.fonts(|f| f.layout_job(job));
            let body_resp = ui.add(
                egui::Label::new(egui::WidgetText::Galley(galley.clone()))
                    .sense(Sense::click())
                    .selectable(false),
            );
            let galley_origin = body_resp.rect.min;
            let hovered_url = body_resp.hover_pos().and_then(|pos| {
                url_at_galley_pos(&galley, galley_origin, &body_text, &url_spans, pos)
            });
            let tip = if let Some(url) = hovered_url.as_deref() {
                url.to_string()
            } else if can_edit || can_delete {
                "Right-click / long-press for edit, delete, react".to_string()
            } else if can_react {
                let heart = display_emoji(DEFAULT_REACT_EMOJI);
                format!("Double-click {heart} · right-click / long-press for more")
            } else {
                "Right-click / long-press to copy".into()
            };
            let mut body_resp = body_resp.on_hover_text(tip);
            if hovered_url.is_some() {
                body_resp = body_resp.on_hover_cursor(CursorIcon::PointingHand);
            }
            let mut opened_link = false;
            if body_resp.clicked() {
                if let Some(pos) = body_resp.interact_pointer_pos() {
                    if let Some(url) =
                        url_at_galley_pos(&galley, galley_origin, &body_text, &url_spans, pos)
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        opened_link = true;
                    }
                }
            }
            if can_react
                && !opened_link
                && matches!(action, MessageBubbleAction::None)
                && body_resp.double_clicked()
            {
                action = MessageBubbleAction::ToggleReaction {
                    msgid: msg.id.clone(),
                    emoji: DEFAULT_REACT_EMOJI.to_string(),
                };
            }
            body_resp.context_menu(|ui| {
                if can_react {
                    if ui.button("React…").clicked() {
                        action = if react_picker_open {
                            MessageBubbleAction::CloseReactPicker
                        } else {
                            MessageBubbleAction::OpenReactPicker {
                                msgid: msg.id.clone(),
                            }
                        };
                        ui.close_menu();
                    }
                }
                if !msg.text.is_empty() && ui.button("Copy text").clicked() {
                    ui.ctx().copy_text(msg.text.clone());
                    ui.close_menu();
                }
                if can_edit || can_delete {
                    ui.separator();
                }
                if can_edit && ui.button("Edit").clicked() {
                    action = MessageBubbleAction::Edit {
                        msgid: msg.id.clone(),
                        text: msg.text.clone(),
                    };
                    ui.close_menu();
                }
                if can_delete && ui.button("Delete").clicked() {
                    // Server `+draft/delete` names the edit-chain root.
                    let delete_id = msg
                        .edit_of
                        .clone()
                        .unwrap_or_else(|| msg.id.clone());
                    action = MessageBubbleAction::Delete {
                        msgid: delete_id,
                    };
                    ui.close_menu();
                }
            });

            if let Some(embed) = embed {
                ui.add_space(sp.sm);
                match embed {
                    Embed::Image { url } => {
                        if inline_image_preview(ui, th, media, &url) {
                            action = MessageBubbleAction::OpenImage { url };
                        }
                    }
                    Embed::Link { url } => {
                        let seed = msg.link_meta.clone();
                        // Salt with message id so two bubbles with the same URL
                        // never share an interact / hover widget id.
                        og_link_preview(ui, th, media, &url, seed.as_ref(), &msg.id);
                    }
                }
            }
        });
    let _ = frame_resp;

    // Reaction tallies (outside body hit-target) — Vidya Lucide icons when mapped.
    if !msg.reactions.is_empty() {
        ui.add_space(sp.xs);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut entries: Vec<_> = msg.reactions.iter().collect();
            entries.sort_by(|(a, na), (b, nb)| nb.len().cmp(&na.len()).then_with(|| a.cmp(b)));
            let icon_size = (th.type_scale.caption * 1.15).max(14.0);
            for (emoji, nicks) in entries {
                let count = nicks.len();
                let mine = nicks.iter().any(|n| n.eq_ignore_ascii_case(own_nick));
                let fill = if mine {
                    p.accent.gamma_multiply(0.35)
                } else {
                    p.headerbar_bg
                };
                let stroke = if mine {
                    Stroke::new(1.0_f32, p.accent.gamma_multiply(0.7))
                } else {
                    Stroke::new(1.0_f32, p.border_soft)
                };
                let shown_emoji = display_emoji(emoji);
                let tip = {
                    let mut names: Vec<_> = nicks.iter().cloned().collect();
                    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                    let shown: Vec<_> = names.into_iter().take(12).collect();
                    let extra = count.saturating_sub(shown.len());
                    let mut s = format!("reacted with {shown_emoji}: {}", shown.join(", "));
                    if extra > 0 {
                        s.push_str(&format!(" +{extra} more"));
                    }
                    s
                };
                let chip = egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let (ir, _) =
                                ui.allocate_exact_size(Vec2::splat(icon_size), Sense::hover());
                            paint_emoji_in(ui, ir, emoji, p.text);
                            if count > 1 {
                                ui.label(
                                    RichText::new(count.to_string())
                                        .size(th.type_scale.caption)
                                        .color(p.text),
                                );
                            }
                        });
                    });
                let resp = ui
                    .interact(
                        chip.response.rect,
                        ui.id().with("react_chip").with(&msg.id).with(emoji.as_str()),
                        Sense::click(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(tip);
                if can_react && resp.clicked() {
                    action = MessageBubbleAction::ToggleReaction {
                        msgid: msg.id.clone(),
                        emoji: emoji.clone(),
                    };
                }
            }
            if can_react {
                let add_size = (th.type_scale.caption * 1.15).max(14.0);
                let add = egui::Frame::new()
                    .fill(p.headerbar_bg)
                    .stroke(Stroke::new(1.0_f32, p.border_soft))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        icon_colored(ui, p.text_secondary, Icon::Plus, add_size);
                    });
                let resp = ui
                    .interact(
                        add.response.rect,
                        ui.id().with("react_add").with(&msg.id),
                        Sense::click(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("Add reaction");
                if resp.clicked() {
                    action = MessageBubbleAction::OpenReactPicker {
                        msgid: msg.id.clone(),
                    };
                }
            }
        });
    }

    action
}

/// Modal emoji reaction picker (quick row + search + category grid).
///
/// Call once per frame while `state.react_picker_msg` is set for `channel`.
/// Returns `(msgid, emoji)` when the user picks a reaction. Esc / backdrop /
/// Close dismiss without reacting.
pub fn react_picker_overlay(
    ctx: &egui::Context,
    th: &Theme,
    state: &mut AppState,
    channel: &str,
) -> Option<(String, String)> {
    let Some(msgid) = state.react_picker_msg.clone() else {
        return None;
    };

    let msg = state
        .channels
        .get(channel)
        .and_then(|buf| buf.messages.iter().find(|m| m.id == msgid).cloned());
    let Some(msg) = msg else {
        // Message left the buffer (history trim / channel switch) — drop picker.
        state.close_react_picker();
        return None;
    };
    let own_nick = state.nick.clone();

    let p = &th.palette;
    let sp = &th.spacing;
    let frame = egui::Frame::new()
        .fill(p.card_bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .shadow(egui::epaint::Shadow {
            offset: [0, 4],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(80),
        });

    let mut picked: Option<String> = None;
    let mut close = false;

    let modal = egui::Modal::new(egui::Id::new("react_emoji_picker"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(frame)
        .show(ctx, |ui| {
            let panel_w = (ctx.screen_rect().width() - 32.0).clamp(260.0, 360.0);
            ui.set_width(panel_w);

            // Header + search + close
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Add reaction")
                        .size(th.type_scale.body)
                        .color(p.text)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close_resp = ui
                        .add(
                            egui::Button::new(
                                RichText::new("✕")
                                    .size(th.type_scale.body)
                                    .color(p.text_secondary),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .frame(false),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!("Close ({})", vidya::escape_label()));
                    if close_resp.clicked() {
                        close = true;
                    }
                    let search_w = (ui.available_width() - 8.0).clamp(100.0, 200.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut state.react_picker_search)
                            .id_salt(("react_emoji_search", msgid.as_str()))
                            .desired_width(search_w)
                            .hint_text("Search emoji…")
                            .font(egui::TextStyle::Body),
                    );
                });
            });

            ui.add_space(sp.sm);

            let pick_size = (th.type_scale.body * 1.25).max(18.0);

            // Quick-react strip (freeq-android parity).
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for emoji in QUICK_REACT_EMOJIS {
                    if emoji_pick_button(ui, th, emoji, &msg, &own_nick, pick_size) {
                        picked = Some((*emoji).to_string());
                    }
                }
            });

            ui.add_space(sp.xs);

            let searching = !state.react_picker_search.trim().is_empty();

            // Category tabs (hidden while searching — results span all groups).
            if !searching {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let tab_icon = (th.type_scale.caption * 1.15).max(14.0);
                    for g in EmojiPickerGroup::ALL {
                        let selected = state.react_picker_group == *g;
                        let fill = if selected {
                            p.accent.gamma_multiply(0.35)
                        } else {
                            Color32::TRANSPARENT
                        };
                        let stroke = if selected {
                            Stroke::new(1.0_f32, p.accent.gamma_multiply(0.65))
                        } else {
                            Stroke::NONE
                        };
                        let btn_size = Vec2::new(32.0, 28.0);
                        let (rect, resp) = ui.allocate_exact_size(btn_size, Sense::click());
                        if ui.is_rect_visible(rect) {
                            ui.painter().rect(
                                rect,
                                sp.radius_sm,
                                fill,
                                stroke,
                                egui::StrokeKind::Inside,
                            );
                            let ir = Rect::from_center_size(rect.center(), Vec2::splat(tab_icon));
                            paint_emoji_in(ui, ir, g.tab_emoji(), p.text);
                        }
                        let resp = resp
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text(g.label());
                        if resp.clicked() {
                            state.react_picker_group = *g;
                        }
                    }
                });
                ui.add_space(sp.xs);
            } else {
                dim_label(
                    ui,
                    th,
                    &format!("Results for “{}”", state.react_picker_search.trim()),
                );
                ui.add_space(2.0);
            }

            // Scrollable emoji grid.
            let grid_h = 240.0_f32;
            let cell = Vec2::new(36.0, 34.0);
            let group = state.react_picker_group;
            let search_q = state.react_picker_search.clone();
            ScrollArea::vertical()
                .id_salt(("react_emoji_grid", msgid.as_str(), searching))
                .max_height(grid_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                        if searching {
                            let mut count = 0usize;
                            for emoji in emojis::iter() {
                                if !emoji_matches_search(emoji, &search_q) {
                                    continue;
                                }
                                if emoji_pick_cell(
                                    ui,
                                    th,
                                    emoji.as_str(),
                                    &msg,
                                    &own_nick,
                                    cell,
                                    pick_size,
                                ) {
                                    picked = Some(emoji.as_str().to_string());
                                }
                                count += 1;
                                if count >= EMOJI_SEARCH_LIMIT {
                                    break;
                                }
                            }
                            if count == 0 {
                                dim_label(ui, th, "No matching emoji");
                            } else if count >= EMOJI_SEARCH_LIMIT {
                                ui.label(
                                    RichText::new(format!(
                                        "Showing first {EMOJI_SEARCH_LIMIT} — refine search"
                                    ))
                                    .size(th.type_scale.caption)
                                    .color(p.text_secondary),
                                );
                            }
                        } else {
                            for emoji in group.emojis() {
                                if emoji_pick_cell(
                                    ui,
                                    th,
                                    emoji.as_str(),
                                    &msg,
                                    &own_nick,
                                    cell,
                                    pick_size,
                                ) {
                                    picked = Some(emoji.as_str().to_string());
                                }
                            }
                        }
                    });
                });
        });

    if close || modal.should_close() {
        state.close_react_picker();
        return None;
    }

    if let Some(emoji) = picked {
        state.close_react_picker();
        return Some((msgid, emoji));
    }

    None
}

/// Compact quick-react button; returns true when clicked.
fn emoji_pick_button(
    ui: &mut egui::Ui,
    th: &Theme,
    emoji: &str,
    msg: &ChatMessage,
    own_nick: &str,
    pick_size: f32,
) -> bool {
    emoji_pick_cell(
        ui,
        th,
        emoji,
        msg,
        own_nick,
        Vec2::new(36.0, 32.0),
        pick_size,
    )
}

/// One tappable cell in the emoji grid; returns true when clicked.
fn emoji_pick_cell(
    ui: &mut egui::Ui,
    th: &Theme,
    emoji: &str,
    msg: &ChatMessage,
    own_nick: &str,
    btn_size: Vec2,
    pick_size: f32,
) -> bool {
    let p = &th.palette;
    let sp = &th.spacing;
    let mine = msg.has_reaction_from(emoji, own_nick);
    let fill = if mine {
        p.accent.gamma_multiply(0.4)
    } else {
        Color32::TRANSPARENT
    };
    let shown = display_emoji(emoji);
    let name = emojis::get(emoji).map(|e| e.name()).unwrap_or(emoji);
    let (rect, btn) = ui.allocate_exact_size(btn_size, Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            sp.radius_sm,
            fill,
            if mine {
                Stroke::new(1.0_f32, p.accent.gamma_multiply(0.6))
            } else {
                Stroke::NONE
            },
            egui::StrokeKind::Inside,
        );
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(pick_size));
        paint_emoji_in(ui, icon_rect, emoji, p.text);
    }
    let btn = btn
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if mine {
            format!("Remove {shown} ({name})")
        } else {
            format!("React {shown} ({name})")
        });
    btn.clicked()
}

/// Max width for inline image / link cards inside a bubble.
const EMBED_MAX_W: f32 = 280.0;
const EMBED_MAX_H: f32 = 220.0;

/// LayoutJob with http(s) spans styled as accent underlines.
fn linkify_layout_job(text: &str, spans: &[UrlSpan], th: &Theme) -> LayoutJob {
    let p = &th.palette;
    let size = th.type_scale.body;
    let base = TextFormat {
        font_id: FontId::proportional(size),
        color: p.text,
        ..Default::default()
    };
    let link = TextFormat {
        font_id: FontId::proportional(size),
        color: p.accent,
        underline: Stroke::new(1.0_f32, p.accent),
        ..Default::default()
    };

    let mut job = LayoutJob::default();
    if spans.is_empty() {
        job.append(text, 0.0, base);
        return job;
    }

    let mut cursor = 0usize;
    for span in spans {
        if span.start > cursor && span.start <= text.len() {
            job.append(&text[cursor..span.start], 0.0, base.clone());
        }
        let end = span.end.min(text.len());
        if span.start < end {
            job.append(&text[span.start..end], 0.0, link.clone());
        }
        cursor = end;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, base);
    } else if job.is_empty() {
        job.append(text, 0.0, base);
    }
    job
}

/// URL under a pointer position within a laid-out message body galley, if any.
fn url_at_galley_pos(
    galley: &egui::Galley,
    galley_origin: egui::Pos2,
    text: &str,
    spans: &[UrlSpan],
    pos: egui::Pos2,
) -> Option<String> {
    if spans.is_empty() {
        return None;
    }
    let relative = pos - galley_origin;
    let cursor = galley.cursor_from_pos(relative);
    let char_idx = cursor.ccursor.index;
    let byte_idx = text
        .char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    spans
        .iter()
        .find(|s| byte_idx >= s.start && byte_idx < s.end)
        .or_else(|| {
            // Cursor often lands just after the last glyph of a link.
            let prev = byte_idx.saturating_sub(1);
            spans
                .iter()
                .find(|s| byte_idx == s.end && prev >= s.start && prev < s.end)
        })
        .map(|s| s.url.clone())
}

/// Inline chat image. Returns `true` when the user taps to open the lightbox.
fn inline_image_preview(ui: &mut egui::Ui, th: &Theme, media: &mut MediaCache, url: &str) -> bool {
    let p = &th.palette;
    let sp = &th.spacing;
    media.touch_image(url);

    // Snapshot state so we can mutably load textures without holding map borrows.
    let ready = match media.images.get_mut(url) {
        Some(ImageState::Ready(pixels)) => {
            let tex = pixels.texture(ui.ctx(), url).clone();
            Some((tex, pixels.width, pixels.height))
        }
        Some(ImageState::Loading) => None,
        Some(ImageState::Failed) | None => {
            // Failed: show the URL as a muted fallback (still tappable → web).
            link_fallback_row(ui, th, url, "Couldn't load image");
            return false;
        }
    };

    match ready {
        None => {
            // Loading placeholder.
            egui::Frame::new()
                .fill(p.headerbar_bg)
                .corner_radius(sp.radius_sm)
                .inner_margin(egui::Margin::symmetric(12, 16))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(sp.sm);
                        dim_label(ui, th, "Loading image…");
                    });
                });
            false
        }
        Some((tex, width, height)) => {
            let max_w = ui.available_width().min(EMBED_MAX_W).max(80.0);
            let scale = (max_w / width.max(1) as f32)
                .min(EMBED_MAX_H / height.max(1) as f32)
                .min(1.0);
            let size = Vec2::new(
                (width as f32 * scale).max(1.0),
                (height as f32 * scale).max(1.0),
            );
            let resp = ui
                .add(
                    egui::Image::new((tex.id(), size))
                        .corner_radius(sp.radius_sm)
                        .sense(Sense::click()),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Tap to enlarge");
            resp.clicked()
        }
    }
}

fn og_link_preview(
    ui: &mut egui::Ui,
    th: &Theme,
    media: &mut MediaCache,
    url: &str,
    seeded: Option<&LinkMeta>,
    // Unique per bubble (message id). Must not be URL alone — same link
    // in two messages would collide on interact/hover state.
    id_salt: &str,
) {
    let p = &th.palette;
    let sp = &th.spacing;

    // Prefer seeded IRCv3 meta; otherwise fetch.
    if let Some(meta) = seeded {
        media.seed_link(url, meta.clone());
    } else {
        media.touch_link(url);
    }

    let (title, description, thumb_url, site_name, loading) = match media.links.get(url) {
        Some(LinkState::Ready(m)) => (
            m.title.clone(),
            m.description.clone(),
            m.thumb_url.clone(),
            m.site_name.clone(),
            false,
        ),
        Some(LinkState::Loading) => (None, None, None, None, true),
        Some(LinkState::Failed) | None => (None, None, None, None, false),
    };

    let host = preview::display_host(url);
    let path = preview::display_path(url);
    let card_w = ui.available_width().min(300.0).max(120.0);
    // Per-message id — not the URL (duplicate links must not share widget state).
    let card_id = ui.id().with("og").with(id_salt);

    let frame_resp = egui::Frame::new()
        .fill(p.headerbar_bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(sp.sm as i8 + 2, sp.sm as i8 + 2))
        .show(ui, |ui| {
            ui.set_width(card_w);
            ui.horizontal(|ui| {
                // Thumbnail (if we have one and it loaded).
                if let Some(ref thumb) = thumb_url {
                    media.touch_image(thumb);
                    if let Some(ImageState::Ready(pixels)) = media.images.get_mut(thumb.as_str()) {
                        let tex = pixels.texture(ui.ctx(), thumb).clone();
                        const TH: f32 = 56.0;
                        let scale = (TH / pixels.width.max(1) as f32)
                            .min(TH / pixels.height.max(1) as f32)
                            .min(1.0);
                        let size = Vec2::new(
                            (pixels.width as f32 * scale).max(1.0),
                            (pixels.height as f32 * scale).max(1.0),
                        );
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(TH), Sense::hover());
                        ui.painter()
                            .rect_filled(rect, sp.radius_sm, p.card_bg);
                        let img_rect = egui::Rect::from_center_size(rect.center(), size);
                        ui.put(
                            img_rect,
                            egui::Image::new((tex.id(), size)).corner_radius(sp.radius_sm),
                        );
                        ui.add_space(sp.sm);
                    } else if matches!(
                        media.images.get(thumb.as_str()),
                        Some(ImageState::Loading)
                    ) {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                        ui.painter()
                            .rect_filled(rect, sp.radius_sm, p.card_bg);
                        ui.put(rect, egui::Spinner::new());
                        ui.add_space(sp.sm);
                    }
                } else {
                    // Link glyph stand-in.
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        sp.radius_sm,
                        p.accent.gamma_multiply(0.18),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "🔗",
                        egui::FontId::proportional(14.0),
                        p.accent,
                    );
                    ui.add_space(sp.sm);
                }

                ui.vertical(|ui| {
                    ui.set_max_width((card_w - 72.0).max(80.0));
                    if loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(4.0);
                            dim_label(ui, th, "Fetching preview…");
                        });
                    }
                    // Site name above title (freeq-app parity).
                    if let Some(site) = site_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        ui.label(
                            RichText::new(site.to_uppercase())
                                .size((th.type_scale.caption - 1.0).max(9.0))
                                .color(p.text_secondary),
                        );
                    }
                    let headline = title
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or(host.as_str());
                    ui.label(
                        RichText::new(headline)
                            .size(th.type_scale.body)
                            .color(p.text)
                            .strong(),
                    );
                    if let Some(desc) = description
                        .as_deref()
                        .filter(|d| !d.trim().is_empty())
                    {
                        let short = if desc.chars().count() > 160 {
                            format!("{}…", desc.chars().take(160).collect::<String>())
                        } else {
                            desc.to_string()
                        };
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(short)
                                .size(th.type_scale.caption)
                                .color(p.text_secondary),
                        );
                    }
                    ui.add_space(2.0);
                    let foot = if path.is_empty() {
                        host.clone()
                    } else if title.is_some() {
                        format!("{host}{path}")
                    } else {
                        path
                    };
                    ui.label(
                        RichText::new(foot)
                            .size(th.type_scale.caption)
                            .color(p.accent),
                    );
                });
            });
        });

    let resp = ui.interact(frame_resp.response.rect, card_id, Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

fn link_fallback_row(ui: &mut egui::Ui, th: &Theme, url: &str, note: &str) {
    let p = &th.palette;
    ui.horizontal(|ui| {
        dim_label(ui, th, note);
        ui.add_space(th.spacing.xs);
        let resp = ui
            .add(
                egui::Label::new(
                    RichText::new(preview::display_host(url))
                        .size(th.type_scale.caption)
                        .color(p.accent),
                )
                .sense(Sense::click()),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(url);
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    });
}

/// Full-screen image lightbox: large preview + link to open the original on the web.
///
/// Call once per frame while `state.image_lightbox` is set. Handles Esc, backdrop
/// tap, and Close. "View original" opens the URL in the system browser.
pub fn image_lightbox_overlay(ctx: &egui::Context, th: &Theme, state: &mut AppState) {
    let Some(url) = state.image_lightbox.clone() else {
        return;
    };

    let p = &th.palette;
    let sp = &th.spacing;
    let screen = ctx.screen_rect();

    let mut close = false;
    let mut open_web = false;

    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        close = true;
    }

    // Keep the decoded image warm while the lightbox is open.
    state.media.touch_image(&url);

    let ready = match state.media.images.get_mut(url.as_str()) {
        Some(ImageState::Ready(pixels)) => {
            let tex = pixels.texture(ctx, &url).clone();
            Some((tex, pixels.width, pixels.height))
        }
        _ => None,
    };

    egui::Area::new(egui::Id::new("image_lightbox"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (full, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(full, 0.0, Color32::from_black_alpha(220));

            // Top chrome: Close (left) + View original (right).
            let chrome_h = (sp.control_height + sp.md * 2.0).max(48.0);
            let chrome = egui::Rect::from_min_size(
                full.min + Vec2::new(sp.md, sp.md),
                Vec2::new((full.width() - sp.md * 2.0).max(1.0), chrome_h),
            );

            // Image area: remaining viewport below chrome, with padding.
            let img_pad = sp.md;
            let img_area = egui::Rect::from_min_max(
                egui::pos2(full.left() + img_pad, chrome.bottom() + sp.sm),
                egui::pos2(full.right() - img_pad, full.bottom() - img_pad),
            );

            let mut hit_chrome = false;
            let mut img_rect = egui::Rect::NOTHING;

            ui.scope_builder(egui::UiBuilder::new().max_rect(chrome), |ui| {
                ui.horizontal(|ui| {
                    let close_resp = ui
                        .add(
                            egui::Button::new(
                                RichText::new("Close")
                                    .size(th.type_scale.body)
                                    .color(Color32::WHITE)
                                    .strong(),
                            )
                            .fill(p.card_bg.gamma_multiply(0.85))
                            .stroke(Stroke::new(1.0_f32, p.border_soft))
                            .corner_radius(sp.radius_md)
                            .min_size(Vec2::new(0.0, sp.control_height)),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!("Close ({})", vidya::escape_label()));
                    if close_resp.clicked() {
                        close = true;
                    }
                    if close_resp.hovered() || close_resp.contains_pointer() {
                        hit_chrome = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let view_resp = ui
                            .add(
                                egui::Button::new(
                                    RichText::new("View original")
                                        .size(th.type_scale.body)
                                        .color(p.accent)
                                        .strong(),
                                )
                                .fill(p.card_bg.gamma_multiply(0.85))
                                .stroke(Stroke::new(1.0_f32, p.accent.gamma_multiply(0.55)))
                                .corner_radius(sp.radius_md)
                                .min_size(Vec2::new(0.0, sp.control_height)),
                            )
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text(&url);
                        if view_resp.clicked() {
                            open_web = true;
                        }
                        if view_resp.hovered() || view_resp.contains_pointer() {
                            hit_chrome = true;
                        }
                    });
                });
            });

            match ready {
                Some((tex, width, height)) => {
                    // Letterbox into img_area — never stretch. (`ui.put` uses a
                    // justified layout that expands the widget to the full rect.)
                    let max_w = img_area.width().max(1.0);
                    let max_h = img_area.height().max(1.0);
                    let nw = width.max(1) as f32;
                    let nh = height.max(1) as f32;
                    let scale = (max_w / nw).min(max_h / nh).min(3.0);
                    let size = Vec2::new((nw * scale).max(1.0), (nh * scale).max(1.0));
                    img_rect = egui::Rect::from_center_size(img_area.center(), size);
                    let uv = egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1.0, 1.0),
                    );
                    ui.painter().add(
                        egui::epaint::RectShape::filled(img_rect, sp.radius_sm, Color32::WHITE)
                            .with_texture(tex.id(), uv),
                    );
                }
                None => {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(img_area), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(img_area.height() * 0.35);
                            ui.spinner();
                            ui.add_space(sp.sm);
                            ui.label(
                                RichText::new("Loading image…")
                                    .size(th.type_scale.body)
                                    .color(Color32::WHITE),
                            );
                        });
                    });
                }
            }

            // Backdrop tap (outside chrome / image) dismisses.
            if backdrop.clicked() && !hit_chrome {
                let on_image = backdrop
                    .interact_pointer_pos()
                    .is_some_and(|pos| img_rect.contains(pos));
                if !on_image {
                    close = true;
                }
            }
        });

    if open_web {
        ctx.open_url(egui::OpenUrl::new_tab(url));
    }
    if close {
        state.close_image_lightbox();
    }
}

pub fn empty_state(ui: &mut egui::Ui, th: &Theme, title: &str, blurb: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        title_2(ui, th, title);
        ui.add_space(th.spacing.sm);
        // Allow multi-line server reasons (wrap within the column).
        ui.set_max_width((ui.available_width() - 24.0).max(120.0));
        dim_label(ui, th, blurb);
    });
}
