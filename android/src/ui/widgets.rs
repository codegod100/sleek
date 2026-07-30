//! Shared Vidya-styled widgets.

use eframe::egui::{
    self, Align, Color32, CursorIcon, Layout, Rect, RichText, ScrollArea, Sense, Stroke, Vec2,
};
use vidya::{dim_label, icon_colored, paint_emoji_in, title_2, Icon, Theme};

use crate::preview::{self, Embed};
use crate::state::{
    display_emoji, emoji_matches_search, Buffer, ChatMessage, EmojiPickerGroup, ImageState,
    LinkMeta, LinkState, MediaCache, DEFAULT_REACT_EMOJI, EMOJI_SEARCH_LIMIT, QUICK_REACT_EMOJIS,
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
/// When the reaction picker is open, pass mutable search + category so the
/// full emoji grid can filter without reopening the bubble.
pub fn message_bubble(
    ui: &mut egui::Ui,
    th: &Theme,
    msg: &ChatMessage,
    own_nick: &str,
    media: &mut MediaCache,
    react_picker_open: bool,
    react_search: &mut String,
    react_group: &mut EmojiPickerGroup,
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

    let bg = if is_own {
        p.accent.gamma_multiply(0.28)
    } else {
        p.card_bg
    };

    let embed = msg.resolved_embed();
    let can_react =
        !msg.id.is_empty() && !msg.id.starts_with("local-") && !msg.id.starts_with("sys-");

    // Body frame only — reaction chips live *below* so bubble gestures can't
    // steal their clicks (later full-rect interacts would otherwise win).
    let frame_resp = egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
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
                            .on_hover_text(if react_picker_open {
                                "Hide reactions"
                            } else {
                                "Add reaction"
                            });
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
            // Sense on the body only — double-click heart, right-click / long-press picker
            // (freeq-android parity). Keeps chips / embeds / ☺ free of click steal.
            let body_resp = ui.add(
                egui::Label::new(
                    RichText::new(&body_text)
                        .size(th.type_scale.body)
                        .color(p.text),
                )
                .wrap()
                .sense(if can_react {
                    Sense::click()
                } else {
                    Sense::hover()
                }),
            );
            if can_react {
                let heart = display_emoji(DEFAULT_REACT_EMOJI);
                let body_resp = body_resp.on_hover_text(format!(
                    "Double-click {heart} · right-click / long-press to react"
                ));
                if matches!(action, MessageBubbleAction::None) {
                    if body_resp.double_clicked() {
                        action = MessageBubbleAction::ToggleReaction {
                            msgid: msg.id.clone(),
                            emoji: DEFAULT_REACT_EMOJI.to_string(),
                        };
                    } else if body_resp.secondary_clicked() || body_resp.long_touched() {
                        action = if react_picker_open {
                            MessageBubbleAction::CloseReactPicker
                        } else {
                            MessageBubbleAction::OpenReactPicker {
                                msgid: msg.id.clone(),
                            }
                        };
                    }
                }
            }

            if let Some(embed) = embed {
                ui.add_space(sp.sm);
                match embed {
                    Embed::Image { url } => {
                        inline_image_preview(ui, th, media, &url);
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

    // Full emoji reaction picker (quick row + search + category grid).
    if react_picker_open && can_react {
        if let Some(picked) = emoji_react_picker(
            ui,
            th,
            msg,
            own_nick,
            react_search,
            react_group,
        ) {
            action = MessageBubbleAction::ToggleReaction {
                msgid: msg.id.clone(),
                emoji: picked,
            };
        }
    }

    action
}

/// Full emoji reaction picker: quick reacts, search, category tabs, scrollable grid.
/// Returns the chosen emoji string when the user picks one.
fn emoji_react_picker(
    ui: &mut egui::Ui,
    th: &Theme,
    msg: &ChatMessage,
    own_nick: &str,
    search: &mut String,
    group: &mut EmojiPickerGroup,
) -> Option<String> {
    let p = &th.palette;
    let sp = &th.spacing;
    let pick_size = (th.type_scale.body * 1.25).max(18.0);
    let mut picked: Option<String> = None;

    ui.add_space(sp.xs);
    egui::Frame::new()
        .fill(p.headerbar_bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());

            // Header + search
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Add reaction")
                        .size(th.type_scale.caption)
                        .color(p.text_secondary)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let search_w = (ui.available_width() - 8.0).clamp(120.0, 220.0);
                    ui.add(
                        egui::TextEdit::singleline(search)
                            .id_salt(("react_emoji_search", msg.id.as_str()))
                            .desired_width(search_w)
                            .hint_text("Search emoji…")
                            .font(egui::TextStyle::Body),
                    );
                });
            });

            ui.add_space(sp.xs);

            // Quick-react strip (freeq-android parity).
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for emoji in QUICK_REACT_EMOJIS {
                    if emoji_pick_button(ui, th, emoji, msg, own_nick, pick_size) {
                        picked = Some((*emoji).to_string());
                    }
                }
            });

            ui.add_space(sp.xs);

            let searching = !search.trim().is_empty();

            // Category tabs (hidden while searching — results span all groups).
            if !searching {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let tab_icon = (th.type_scale.caption * 1.15).max(14.0);
                    for g in EmojiPickerGroup::ALL {
                        let selected = *group == *g;
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
                            *group = *g;
                        }
                    }
                });
                ui.add_space(sp.xs);
            } else {
                dim_label(ui, th, &format!("Results for “{}”", search.trim()));
                ui.add_space(2.0);
            }

            // Scrollable emoji grid.
            let grid_h = 220.0_f32;
            let cell = Vec2::new(36.0, 34.0);
            ScrollArea::vertical()
                .id_salt(("react_emoji_grid", msg.id.as_str(), searching))
                .max_height(grid_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
                        if searching {
                            let q = search.clone();
                            let mut count = 0usize;
                            for emoji in emojis::iter() {
                                if !emoji_matches_search(emoji, &q) {
                                    continue;
                                }
                                if emoji_pick_cell(
                                    ui,
                                    th,
                                    emoji.as_str(),
                                    msg,
                                    own_nick,
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
                                    msg,
                                    own_nick,
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

    picked
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

fn inline_image_preview(ui: &mut egui::Ui, th: &Theme, media: &mut MediaCache, url: &str) {
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
            // Failed: show the URL as a muted fallback (still tappable).
            link_fallback_row(ui, th, url, "Couldn't load image");
            return;
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
                .on_hover_text(url);
            if resp.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
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
