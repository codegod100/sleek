//! Shared Vidya-styled widgets.

use eframe::egui::{
    self, text::LayoutJob, text::TextFormat, Align, Align2, Color32, CursorIcon, FontId, Id, Key,
    Layout, Order, PointerButton, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2,
};
use vidya::{
    dim_label, icon_colored, paint_emoji_in, paint_icon_in, system_chrome, title_2, Icon, Theme,
};

use crate::preview::{self, Embed, MessageLinkSpan};
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
    /// Begin replying to this message in the compose bar.
    Reply { msgid: String },
    /// Soft-delete this message (`+draft/delete`).
    Delete { msgid: String },
    /// Open / join an IRC channel mentioned in the message body.
    OpenChannel { channel: String },
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

/// Compact icon button for message action toolbars / menus.
fn message_action_icon_btn(
    ui: &mut egui::Ui,
    th: &Theme,
    glyph: MessageActionGlyph,
    tip: &str,
    accent: bool,
) -> egui::Response {
    let p = &th.palette;
    let size = (th.type_scale.body * 1.35).max(22.0);
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    response = response
        .on_hover_text(tip)
        .on_hover_cursor(CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() && ui.is_enabled();
        let active = response.is_pointer_button_down_on();
        let fill = if active {
            p.button_active
        } else if hovered || accent {
            p.button_hover
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect(
                rect,
                th.spacing.radius_sm,
                fill,
                Stroke::NONE,
                egui::StrokeKind::Inside,
            );
        }
        let icon_rect = rect.shrink(size * 0.18);
        let color = if accent { p.accent } else { p.text };
        match glyph {
            MessageActionGlyph::Icon(icon) => paint_icon_in(ui, icon_rect, icon, color),
            MessageActionGlyph::Emoji(emoji) => paint_emoji_in(ui, icon_rect, emoji, color),
        }
    }

    response
}

#[derive(Clone, Copy)]
enum MessageActionGlyph {
    Icon(Icon),
    Emoji(&'static str),
}

const SWIPE_REPLY_THRESHOLD: f32 = 60.0;

#[derive(Clone, Default)]
struct SwipeReplyState {
    offset: f32,
    press_origin: Option<Pos2>,
    armed: bool,
    triggered: bool,
}

/// Touch swipe-right on a message bubble (APK). Returns the visual offset and
/// whether the gesture just crossed the reply threshold.
fn touch_swipe_reply(
    ui: &mut egui::Ui,
    msg_id: &str,
    bubble_rect: Rect,
    can_reply: bool,
) -> (f32, bool) {
    if !can_reply {
        return (0.0, false);
    }
    let touch_ui =
        cfg!(target_os = "android") || ui.input(|i| i.has_touch_screen());
    if !touch_ui {
        return (0.0, false);
    }

    let id = Id::new(("swipe_reply", msg_id));
    let max_off = SWIPE_REPLY_THRESHOLD * 1.2;
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_released = ui.input(|i| i.pointer.primary_released());
    let pos = ui.ctx().pointer_interact_pos();
    let over = pos.is_some_and(|p| bubble_rect.contains(p));

    let mut st = ui
        .ctx()
        .data_mut(|d| d.get_temp::<SwipeReplyState>(id).unwrap_or_default());

    if primary_pressed && over {
        st.press_origin = pos;
        st.armed = false;
        st.triggered = false;
    }

    if primary_down {
        if let (Some(origin), Some(cur)) = (st.press_origin, pos) {
            let dx = cur.x - origin.x;
            let dy = (cur.y - origin.y).abs();
            if !st.armed && dx > 8.0 && dx > dy {
                st.armed = true;
            }
            if st.armed {
                st.offset = dx.clamp(0.0, max_off);
            }
        }
    }

    let mut just_triggered = false;
    if primary_released {
        if st.armed && st.offset >= SWIPE_REPLY_THRESHOLD {
            just_triggered = true;
        }
        st.offset = 0.0;
        st.press_origin = None;
        st.armed = false;
        st.triggered = false;
    } else if st.armed && !st.triggered && st.offset >= SWIPE_REPLY_THRESHOLD {
        st.triggered = true;
        just_triggered = true;
    }

    ui.ctx().data_mut(|d| d.insert_temp(id, st.clone()));
    (st.offset, just_triggered)
}

fn reply_context_preview(ui: &mut egui::Ui, th: &Theme, parent: &ChatMessage) {
    let p = &th.palette;
    let sp = &th.spacing;
    ui.horizontal(|ui| {
        ui.add_space(sp.md);
        let bar_h = (th.type_scale.caption * 1.2).max(14.0);
        let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(2.0, bar_h), Sense::hover());
        ui.painter().rect_filled(bar_rect, 1.0, p.accent);
        ui.add_space(4.0);
        // `ChatMessage::preview()` already prefixes the sender nick.
        ui.add(
            egui::Label::new(
                RichText::new(parent.preview())
                    .size(th.type_scale.caption)
                    .color(p.text_secondary),
            )
            .wrap(),
        );
    });
    ui.add_space(sp.xs);
}

/// Width / height for the shared hover + context icon bar.
#[derive(Clone, Copy)]
struct MessageActionBarMetrics {
    pad: f32,
    btn: f32,
    row_w: f32,
    bar_w: f32,
}

fn message_action_bar_metrics(
    th: &Theme,
    can_react: bool,
    can_reply: bool,
    can_edit: bool,
    can_delete: bool,
) -> MessageActionBarMetrics {
    let pad = 4.0;
    let btn = (th.type_scale.body * 1.35).max(22.0);
    let n = [can_react, can_reply, can_edit, can_delete]
        .into_iter()
        .filter(|v| *v)
        .count() as f32;
    let row_w = n * btn + (n - 1.0).max(0.0) * 2.0;
    MessageActionBarMetrics {
        pad,
        btn,
        row_w,
        bar_w: row_w + pad * 2.0,
    }
}

fn message_action_bar_frame(th: &Theme, pad: f32) -> egui::Frame {
    let p = &th.palette;
    egui::Frame::new()
        .fill(p.card_bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(th.spacing.radius_md)
        .inner_margin(egui::Margin::same(pad as i8))
}

/// Pointer must stay on the bubble this long before the inline actions fade in.
const MESSAGE_HOVER_DWELL_SECS: f64 = 0.30;

/// Fade duration for the per-message inline action chip.
const MESSAGE_HOVER_FADE_SECS: f32 = 0.18;

/// Keep actions visible briefly after leave so a small pointer slip doesn't flash.
const MESSAGE_HOVER_GRACE_SECS: f64 = 0.20;

fn message_hover_enter_id(hover_id: Id) -> Id {
    hover_id.with("enter")
}

fn message_hover_seen_id(hover_id: Id) -> Id {
    hover_id.with("seen")
}

fn message_hover_rect_id(hover_id: Id) -> Id {
    hover_id.with("rect")
}

fn message_hover_fade_id(hover_id: Id) -> Id {
    hover_id.with("fade")
}

/// Shared React / Edit / Delete icon actions (inline hover chip + context menu).
fn message_action_icons(
    ui: &mut egui::Ui,
    th: &Theme,
    metrics: MessageActionBarMetrics,
    can_react: bool,
    can_reply: bool,
    can_edit: bool,
    can_delete: bool,
    react_picker_open: bool,
    msg: &ChatMessage,
) -> Option<MessageBubbleAction> {
    let mut action = None;
    ui.allocate_ui_with_layout(
        Vec2::new(metrics.row_w, metrics.btn),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if can_reply
                && message_action_icon_btn(
                    ui,
                    th,
                    MessageActionGlyph::Emoji("↩"),
                    "Reply",
                    false,
                )
                .clicked()
            {
                action = Some(MessageBubbleAction::Reply {
                    msgid: msg.id.clone(),
                });
            }
            if can_react
                && message_action_icon_btn(
                    ui,
                    th,
                    MessageActionGlyph::Icon(Icon::Laugh),
                    "Add reaction",
                    react_picker_open,
                )
                .clicked()
            {
                action = Some(if react_picker_open {
                    MessageBubbleAction::CloseReactPicker
                } else {
                    MessageBubbleAction::OpenReactPicker {
                        msgid: msg.id.clone(),
                    }
                });
            }
            if can_edit
                && message_action_icon_btn(
                    ui,
                    th,
                    MessageActionGlyph::Emoji("✏️"),
                    "Edit message",
                    false,
                )
                .clicked()
            {
                action = Some(MessageBubbleAction::Edit {
                    msgid: msg.id.clone(),
                    text: msg.text.clone(),
                });
            }
            if can_delete
                && message_action_icon_btn(
                    ui,
                    th,
                    MessageActionGlyph::Emoji("🗑️"),
                    "Delete message",
                    false,
                )
                .clicked()
            {
                // Server `+draft/delete` names the edit-chain root.
                let delete_id = msg.edit_of.clone().unwrap_or_else(|| msg.id.clone());
                action = Some(MessageBubbleAction::Delete { msgid: delete_id });
            }
        },
    );
    action
}

/// Fade opacity for the inline action chip (0 = reserved but invisible, 1 = shown).
///
/// Hit-tests last frame's bubble rect so the chip can live inside the message
/// frame without a floating overlay. Space stays reserved; only opacity changes.
fn message_inline_hover_opacity(ui: &egui::Ui, hover_id: Id, force_show: bool) -> f32 {
    // APK / touch: no hover chip; long-press opens the icon context menu.
    if cfg!(target_os = "android") || ui.input(|i| i.has_touch_screen()) {
        return if force_show { 1.0 } else { 0.0 };
    }

    let time = ui.input(|i| i.time);
    let prev_rect: Option<Rect> = ui
        .ctx()
        .data(|d| d.get_temp(message_hover_rect_id(hover_id)));
    let in_hit_zone = prev_rect.is_some_and(|r| {
        let clipped = r.intersect(ui.clip_rect());
        clipped.is_positive()
            && ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|p| clipped.expand(4.0).contains(p))
    });

    let enter_id = message_hover_enter_id(hover_id);
    let last_seen_id = message_hover_seen_id(hover_id);

    if in_hit_zone {
        ui.ctx().data_mut(|d| {
            if d.get_temp::<f64>(enter_id).is_none() {
                d.insert_temp(enter_id, time);
            }
        });
    }

    let (entered_at, last_seen) = ui.ctx().data(|d| {
        (
            d.get_temp::<f64>(enter_id).unwrap_or(0.0),
            d.get_temp::<f64>(last_seen_id).unwrap_or(0.0),
        )
    });

    let dwell_ok = entered_at > 0.0 && time - entered_at >= MESSAGE_HOVER_DWELL_SECS;
    if in_hit_zone && !dwell_ok && entered_at > 0.0 {
        let remaining_ms =
            ((MESSAGE_HOVER_DWELL_SECS - (time - entered_at)).max(0.0) * 1000.0).ceil() as u64;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(remaining_ms.max(16)));
    }
    if in_hit_zone && dwell_ok {
        ui.ctx()
            .data_mut(|d| d.insert_temp(last_seen_id, time));
    }

    let within_grace = last_seen > 0.0 && time - last_seen < MESSAGE_HOVER_GRACE_SECS;
    let want_show = force_show || (in_hit_zone && dwell_ok) || within_grace;

    if !in_hit_zone && !within_grace && !force_show {
        // Drop enter only after grace ends. Clearing it while grace is active
        // would force a full re-dwell on a brief slip + re-enter, so the chip
        // could start fading out while the pointer is already back on the bubble.
        ui.ctx().data_mut(|d| {
            d.remove::<f64>(enter_id);
            d.remove::<f64>(last_seen_id);
        });
    }

    ui.ctx().animate_bool_with_time(
        message_hover_fade_id(hover_id),
        want_show,
        MESSAGE_HOVER_FADE_SECS,
    )
}

/// Reserve header space for React / Edit / Delete; fade icons in on hover.
fn message_inline_action_chip(
    ui: &mut egui::Ui,
    th: &Theme,
    metrics: MessageActionBarMetrics,
    opacity: f32,
    can_react: bool,
    can_reply: bool,
    can_edit: bool,
    can_delete: bool,
    react_picker_open: bool,
    msg: &ChatMessage,
) -> Option<MessageBubbleAction> {
    let size = Vec2::new(metrics.row_w, metrics.btn);
    if opacity <= 0.001 {
        // Keep layout stable — bubble shape does not jump when actions appear.
        ui.allocate_exact_size(size, Sense::hover());
        return None;
    }

    let interactive = opacity > 0.85;
    let mut action = None;
    ui.scope(|ui| {
        ui.multiply_opacity(opacity);
        ui.add_enabled_ui(interactive, |ui| {
            action = message_action_icons(
                ui,
                th,
                metrics,
                can_react,
                can_reply,
                can_edit,
                can_delete,
                react_picker_open,
                msg,
            );
        });
    });
    action
}

/// Where to pin the menu. `above_finger` uses a bottom-center pivot so the
/// whole popup sits above the contact point (not under the fingertip).
#[derive(Clone, Copy)]
struct MenuAnchor {
    pos: Pos2,
    above_finger: bool,
}

/// Track a press that started over a widget so long-press works without
/// stealing click sense.
#[derive(Clone, Copy)]
struct PressHold {
    t0: f64,
    pos: Pos2,
    long_fired: bool,
}

/// Edge-trigger long-press over `rect` (APK / touch). Returns
/// `(long_press_this_frame, suppress_short_click)` — the latter stays true from
/// the long-press frame until the primary button is released.
fn touch_long_press(ui: &mut egui::Ui, press_id: Id, rect: Rect) -> (bool, bool) {
    let clipped = rect.intersect(ui.clip_rect());
    let over = clipped.is_positive() && ui.rect_contains_pointer(clipped);
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let time = ui.input(|i| i.time);
    let max_dur = ui.ctx().options(|o| o.input_options.max_click_duration);
    let max_slide = ui
        .ctx()
        .options(|o| o.input_options.max_click_dist as f32)
        .max(8.0);
    let pos = ui.ctx().pointer_interact_pos();

    if primary_pressed && over {
        if let Some(pos) = pos {
            ui.ctx()
                .data_mut(|d| d.insert_temp(press_id, PressHold {
                    t0: time,
                    pos,
                    long_fired: false,
                }));
        }
    }

    let mut long_this_frame = false;
    let mut suppress_click = false;
    if primary_down {
        if let Some(mut start) = ui.ctx().data(|d| d.get_temp::<PressHold>(press_id)) {
            suppress_click = start.long_fired;
            let moved = pos
                .map(|p| (p - start.pos).length() > max_slide)
                .unwrap_or(false);
            if moved {
                ui.ctx().data_mut(|d| d.remove::<PressHold>(press_id));
                suppress_click = false;
            } else if !start.long_fired && time - start.t0 >= max_dur {
                long_this_frame = true;
                start.long_fired = true;
                suppress_click = true;
                ui.ctx().data_mut(|d| d.insert_temp(press_id, start));
            }
        }
    } else {
        ui.ctx().data_mut(|d| d.remove::<PressHold>(press_id));
    }
    (long_this_frame, suppress_click)
}

/// Long-press (APK) popup listing nicknames who used one reaction emoji.
fn reaction_reactors_popup(
    ui: &mut egui::Ui,
    th: &Theme,
    popup_id: Id,
    chip_rect: Rect,
    emoji: &str,
    nicks: &[String],
    opening: bool,
) {
    let touch_ui =
        cfg!(target_os = "android") || ui.input(|i| i.any_touches() || i.has_touch_screen());

    if opening {
        let anchor = if let Some(finger) = ui.ctx().pointer_interact_pos() {
            if touch_ui {
                const FINGER_CLEARANCE: f32 = 72.0;
                MenuAnchor {
                    pos: Pos2::new(finger.x, finger.y - FINGER_CLEARANCE),
                    above_finger: true,
                }
            } else {
                MenuAnchor {
                    pos: finger,
                    above_finger: false,
                }
            }
        } else {
            MenuAnchor {
                pos: Pos2::new(chip_rect.center().x, chip_rect.top() - 8.0),
                above_finger: true,
            }
        };
        ui.memory_mut(|m| m.open_popup(popup_id));
        ui.ctx().data_mut(|d| d.insert_temp(popup_id, anchor));
    }

    if !ui.memory(|m| m.is_popup_open(popup_id)) {
        return;
    }

    let anchor = ui
        .ctx()
        .data(|d| d.get_temp::<MenuAnchor>(popup_id))
        .unwrap_or(MenuAnchor {
            pos: chip_rect.left_top(),
            above_finger: touch_ui,
        });

    let p = &th.palette;
    let sp = &th.spacing;
    let shown_emoji = display_emoji(emoji);
    let pivot = if anchor.above_finger {
        Align2::CENTER_BOTTOM
    } else {
        Align2::LEFT_TOP
    };

    let popup = egui::Area::new(popup_id.with("area"))
        .kind(egui::UiKind::Popup)
        .order(Order::Foreground)
        .fixed_pos(anchor.pos)
        .pivot(pivot)
        .default_width(200.0)
        .sense(Sense::click())
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(p.card_bg)
                .stroke(Stroke::new(1.0_f32, p.border_soft))
                .corner_radius(sp.radius_md)
                .inner_margin(egui::Margin::same(8))
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let icon_size = (th.type_scale.caption * 1.15).max(14.0);
                        let (ir, _) =
                            ui.allocate_exact_size(Vec2::splat(icon_size), Sense::hover());
                        paint_emoji_in(ui, ir, emoji, p.text);
                        ui.label(
                            RichText::new(format!("{shown_emoji} · {}", nicks.len()))
                                .size(th.type_scale.caption)
                                .color(p.text_secondary)
                                .strong(),
                        );
                    });
                    ui.add_space(4.0);
                    ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for nick in nicks {
                                ui.label(
                                    RichText::new(nick)
                                        .size(th.type_scale.body)
                                        .color(p.text),
                                );
                            }
                        });
                });
        });

    if anchor.above_finger {
        let screen = ui.ctx().screen_rect();
        let r = popup.response.rect;
        let mut pos = anchor.pos;
        let mut moved = false;
        if r.left() < screen.left() + 4.0 {
            pos.x += (screen.left() + 4.0) - r.left();
            moved = true;
        } else if r.right() > screen.right() - 4.0 {
            pos.x -= r.right() - (screen.right() - 4.0);
            moved = true;
        }
        if r.top() < screen.top() + 4.0 {
            pos.y += (screen.top() + 4.0) - r.top();
            moved = true;
        }
        if moved {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    popup_id,
                    MenuAnchor {
                        pos,
                        above_finger: true,
                    },
                )
            });
        }
    }

    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    let press_outside = ui.input(|i| i.pointer.any_pressed())
        && !popup.response.contains_pointer()
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| !popup.response.rect.contains(p));
    if escape || (press_outside && !opening) {
        ui.memory_mut(|m| m.close_popup());
        ui.ctx().data_mut(|d| d.remove::<MenuAnchor>(popup_id));
    }
}

/// Right-click / long-press menu for a message bubble: React, Edit, Delete icons.
///
/// Uses raw secondary-button clicks (not [`Response::context_menu`]) so
/// selectable body text's drag-sense cannot swallow the right-click. On touch /
/// APK, a press-and-hold anywhere on the bubble opens the same icon menu.
fn message_context_menu(
    ui: &mut egui::Ui,
    th: &Theme,
    bubble_rect: Rect,
    menu_id: Id,
    body_long_touched: bool,
    can_react: bool,
    can_reply: bool,
    can_edit: bool,
    can_delete: bool,
    react_picker_open: bool,
    msg: &ChatMessage,
) -> Option<MessageBubbleAction> {
    if !can_react && !can_reply && !can_edit && !can_delete {
        return None;
    }

    let press_id = menu_id.with("press");
    let clipped = bubble_rect.intersect(ui.clip_rect());
    let over_bubble = clipped.is_positive() && ui.rect_contains_pointer(clipped);
    let secondary = ui.input(|i| i.pointer.button_clicked(PointerButton::Secondary));
    let touch_ui =
        cfg!(target_os = "android") || ui.input(|i| i.any_touches() || i.has_touch_screen());

    let (long_press_anywhere, _) = touch_long_press(ui, press_id, bubble_rect);

    let long_press = body_long_touched || long_press_anywhere;
    let opening = over_bubble && (secondary || long_press);

    if opening {
        let anchor = if let Some(finger) = ui.ctx().pointer_interact_pos() {
            if touch_ui || long_press {
                // Bottom-center of the menu sits above the contact point with
                // enough clearance that the icons are not under the fingertip.
                const FINGER_CLEARANCE: f32 = 72.0;
                MenuAnchor {
                    pos: Pos2::new(finger.x, finger.y - FINGER_CLEARANCE),
                    above_finger: true,
                }
            } else {
                MenuAnchor {
                    pos: finger,
                    above_finger: false,
                }
            }
        } else if long_press {
            // Touch can briefly clear interact_pos; fall back above the bubble.
            MenuAnchor {
                pos: Pos2::new(bubble_rect.center().x, bubble_rect.top() - 8.0),
                above_finger: true,
            }
        } else {
            MenuAnchor {
                pos: bubble_rect.left_top(),
                above_finger: false,
            }
        };
        ui.memory_mut(|m| m.open_popup(menu_id));
        ui.ctx().data_mut(|d| d.insert_temp(menu_id, anchor));
    }

    if !ui.memory(|m| m.is_popup_open(menu_id)) {
        return None;
    }

    let anchor = ui
        .ctx()
        .data(|d| d.get_temp::<MenuAnchor>(menu_id))
        .unwrap_or(MenuAnchor {
            pos: bubble_rect.left_top(),
            above_finger: false,
        });

    let mut action = None;
    let mut close = false;
    let metrics = message_action_bar_metrics(th, can_react, can_reply, can_edit, can_delete);

    let pivot = if anchor.above_finger {
        Align2::CENTER_BOTTOM
    } else {
        Align2::LEFT_TOP
    };
    let popup = egui::Area::new(menu_id.with("area"))
        .kind(egui::UiKind::Popup)
        .order(Order::Foreground)
        .fixed_pos(anchor.pos)
        .pivot(pivot)
        .default_width(metrics.bar_w)
        .sense(Sense::click())
        .show(ui.ctx(), |ui| {
            message_action_bar_frame(th, metrics.pad)
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    if let Some(a) = message_action_icons(
                        ui,
                        th,
                        metrics,
                        can_react,
                        can_reply,
                        can_edit,
                        can_delete,
                        react_picker_open,
                        msg,
                    ) {
                        action = Some(a);
                        close = true;
                    }
                });
        });

    // Keep a touch menu on-screen after the first layout (it grows upward).
    if anchor.above_finger {
        let screen = ui.ctx().screen_rect();
        let r = popup.response.rect;
        let mut pos = anchor.pos;
        let mut moved = false;
        if r.left() < screen.left() + 4.0 {
            pos.x += (screen.left() + 4.0) - r.left();
            moved = true;
        } else if r.right() > screen.right() - 4.0 {
            pos.x -= r.right() - (screen.right() - 4.0);
            moved = true;
        }
        if r.top() < screen.top() + 4.0 {
            pos.y += (screen.top() + 4.0) - r.top();
            moved = true;
        }
        if moved {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    menu_id,
                    MenuAnchor {
                        pos,
                        above_finger: true,
                    },
                )
            });
        }
    }

    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    // Close on a new press outside the menu (egui context-menu style). Prefer
    // `any_pressed` over `any_click` so the finger-*up* after a long-press open
    // does not immediately dismiss the menu on APK.
    let press_outside = ui.input(|i| i.pointer.any_pressed())
        && !popup.response.contains_pointer()
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| !popup.response.rect.contains(p));
    // Don't close on the same secondary / long-press that opened the menu.
    if close || escape || (press_outside && !opening) {
        ui.memory_mut(|m| m.close_popup());
        ui.ctx().data_mut(|d| {
            d.remove::<MenuAnchor>(menu_id);
            d.remove::<PressHold>(press_id);
        });
    }

    action
}

/// Message bubble / row in chat detail (with optional image / OG link embed).
///
/// `react_picker_open` highlights the hover / menu react control while the
/// modal picker ([`react_picker_overlay`]) is open for this message.
pub fn message_bubble(
    ui: &mut egui::Ui,
    th: &Theme,
    msg: &ChatMessage,
    reply_parent: Option<&ChatMessage>,
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
    let body_line = message_body_line(&body_text, embed.as_ref());
    let media_caption = media_embed_caption(&body_text, embed.as_ref());
    let can_mutate =
        !msg.id.is_empty() && !msg.id.starts_with("local-") && !msg.id.starts_with("sys-");
    let can_react = can_mutate;
    let can_reply = can_mutate;
    let can_edit = can_mutate && is_own;
    let can_delete = can_mutate && is_own;
    let mut body_long_touched = false;
    let hover_id = Id::new(("msg_hover", msg.id.as_str()));
    let menu_id = Id::new(("msg_ctx", msg.id.as_str()));
    let swipe_rect_id = Id::new(("msg_swipe_rect", msg.id.as_str()));
    let menu_open = ui.memory(|m| m.is_popup_open(menu_id));
    let show_actions = can_react || can_reply || can_edit || can_delete;
    let touch_ui =
        cfg!(target_os = "android") || ui.input(|i| i.has_touch_screen());
    let prev_swipe_rect = ui.ctx().data(|d| d.get_temp::<Rect>(swipe_rect_id));
    let (swipe_offset, swipe_reply) = prev_swipe_rect
        .map(|r| touch_swipe_reply(ui, &msg.id, r, can_reply))
        .unwrap_or((0.0, false));
    if swipe_reply {
        action = MessageBubbleAction::Reply {
            msgid: msg.id.clone(),
        };
    }
    // Desktop: reserve header space for the fade-in chip. Touch uses long-press
    // menu only — don't leave a permanent empty gap beside the timestamp.
    let action_metrics = if show_actions && !touch_ui && !menu_open {
        Some(message_action_bar_metrics(
            th,
            can_react,
            can_reply,
            can_edit,
            can_delete,
        ))
    } else {
        None
    };
    let hover_opacity = if action_metrics.is_some() {
        message_inline_hover_opacity(ui, hover_id, react_picker_open)
    } else {
        0.0
    };

    // Body frame only — reaction chips live *below* so bubble gestures can't
    // steal their clicks (later full-rect interacts would otherwise win).
    if let Some(parent) = reply_parent {
        reply_context_preview(ui, th, parent);
    }

    let mut bubble_rect = Rect::NOTHING;
    if swipe_offset > 0.0 {
        ui.add_space(swipe_offset);
    }
    let frame_resp = egui::Frame::new()
        .fill(bg)
        .stroke(stroke)
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2))
        .show(ui, |ui| {
            // Use the frame's inner width only — outer width overflows margins and
            // clips wrapped text on narrow APK screens (same as compose bar).
            let inner_w = ui.available_width().max(1.0);
            ui.set_max_width(inner_w);
            ui.set_min_width(inner_w);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&msg.from)
                        .size(th.type_scale.caption)
                        .color(p.accent)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // Meta first in RTL → far right; action chip sits to its left.
                    let mut meta = msg.time_label();
                    if msg.is_edited {
                        meta = format!("edited · {meta}");
                    }
                    if msg.is_signed {
                        meta = format!("✓ {meta}");
                    }
                    dim_label(ui, th, &meta);
                    // Inline action chip: always reserves space inside the bubble
                    // so hover fades in rather than shifting layout.
                    if let Some(metrics) = action_metrics {
                        ui.add_space(4.0);
                        if let Some(a) = message_inline_action_chip(
                            ui,
                            th,
                            metrics,
                            hover_opacity,
                            can_react,
                            can_reply,
                            can_edit,
                            can_delete,
                            react_picker_open,
                            msg,
                        ) {
                            action = a;
                        }
                    }
                });
            });
            ui.add_space(sp.xs);
            // Sense on the body — URL tap opens browser; double-click heart.
            // Hover / right-click action icons are handled on the whole bubble
            // (below) so selectable text drag-sense can't swallow the clicks.
            let mut body_resp = None;
            let mut galley = None;
            let mut galley_origin = egui::Pos2::ZERO;
            let mut link_spans = Vec::new();
            if let MessageBodyLine::Skip = body_line {
                // URL-only link — OG card below carries the content.
            } else {
                let (visible, cleared) = match &body_line {
                    MessageBodyLine::Text(t) => (t.as_str(), false),
                    MessageBodyLine::Cleared => ("[message cleared]", true),
                    MessageBodyLine::Skip => unreachable!(),
                };
                link_spans = preview::extract_message_link_spans(visible);
                let (resp, laid_out, origin) =
                    render_message_body(ui, th, visible, cleared, inner_w, &link_spans);
                body_long_touched = resp.long_touched();
                galley_origin = origin;
                galley = laid_out;
                body_resp = Some(resp);
            }
            let hovered_link = body_resp.as_ref().and_then(|body_resp| {
                body_resp.hover_pos().and_then(|pos| {
                    link_at_galley_pos(
                        galley.as_ref()?,
                        galley_origin,
                        match &body_line {
                            MessageBodyLine::Text(t) => t.as_str(),
                            MessageBodyLine::Cleared => "[message cleared]",
                            MessageBodyLine::Skip => return None,
                        },
                        &link_spans,
                        pos,
                    )
                })
            });
            if let Some(mut body_resp) = body_resp {
                let tip = if let Some(link) = hovered_link.as_ref() {
                    match link {
                        MessageLinkTarget::Url(url) => url.clone(),
                        MessageLinkTarget::Channel(ch) => format!("Open {ch}"),
                    }
                } else if cfg!(target_os = "android") || ui.input(|i| i.has_touch_screen()) {
                    if can_edit || can_delete {
                        "Long-press for React, Reply, Edit, Delete · swipe right to reply".to_string()
                    } else if can_react {
                        "Long-press for React, Reply · swipe right to reply".to_string()
                    } else if can_reply {
                        "Swipe right to reply".to_string()
                    } else {
                        String::new()
                    }
                } else if can_edit || can_delete {
                    "Hover or right-click for React, Edit, Delete".to_string()
                } else if can_react {
                    let heart = display_emoji(DEFAULT_REACT_EMOJI);
                    format!("Double-click {heart} · hover or right-click for React")
                } else {
                    String::new()
                };
                body_resp = body_resp.on_hover_text(tip);
                if hovered_link.is_some() {
                    body_resp = body_resp.on_hover_cursor(CursorIcon::PointingHand);
                }
                let mut opened_link = false;
                if body_resp.clicked() {
                    if let (Some(pos), Some(galley)) =
                        (body_resp.interact_pointer_pos(), galley.as_ref())
                    {
                        let visible = match &body_line {
                            MessageBodyLine::Text(t) => t.as_str(),
                            MessageBodyLine::Cleared => "[message cleared]",
                            MessageBodyLine::Skip => "",
                        };
                        if let Some(link) =
                            link_at_galley_pos(galley, galley_origin, visible, &link_spans, pos)
                        {
                            match link {
                                MessageLinkTarget::Url(url) => {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                                }
                                MessageLinkTarget::Channel(channel) => {
                                    action = MessageBubbleAction::OpenChannel { channel };
                                }
                            }
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
            }

            if let Some(embed) = embed.as_ref() {
                match embed {
                    Embed::Video { url } => {
                        ui.add_space(sp.sm);
                        inline_video_preview(
                            ui,
                            th,
                            media,
                            url,
                            &msg.id,
                            media_caption.as_deref(),
                        );
                    }
                    Embed::Image { url } => {
                        ui.add_space(sp.sm);
                        if inline_image_preview(
                            ui,
                            th,
                            media,
                            url,
                            &msg.id,
                            media_caption.as_deref(),
                        ) {
                            action = MessageBubbleAction::OpenImage {
                                url: url.clone(),
                            };
                        }
                    }
                    Embed::Link { url } => {
                        ui.add_space(sp.sm);
                        let seed = msg.link_meta.clone();
                        // Salt with message id so two bubbles with the same URL
                        // never share an interact / hover widget id.
                        og_link_preview(ui, th, media, url, seed.as_ref(), &msg.id);
                    }
                }
            }
        });
    bubble_rect = frame_resp.response.rect;
    let row_resp = frame_resp.response;

    if swipe_offset > 0.0 {
        let alpha = (swipe_offset / SWIPE_REPLY_THRESHOLD).clamp(0.0, 1.0);
        ui.painter().text(
            Pos2::new(row_resp.rect.left() + 12.0, row_resp.rect.center().y),
            Align2::LEFT_CENTER,
            "↩",
            FontId::proportional(18.0),
            p.accent.gamma_multiply(alpha),
        );
    }

    ui.ctx()
        .data_mut(|d| d.insert_temp(swipe_rect_id, row_resp.rect));

    // Store bubble rect for next-frame hover hit-testing (inline chip fade).
    if action_metrics.is_some() {
        ui.ctx().data_mut(|d| {
            d.insert_temp(message_hover_rect_id(hover_id), bubble_rect);
        });
    }

    // Right-click / long-press icon menu (React / Reply / Edit / Delete). Uses raw
    // secondary clicks so selectable body text doesn't eat the right-click.
    if let Some(menu_action) = message_context_menu(
        ui,
        th,
        bubble_rect,
        menu_id,
        body_long_touched,
        can_react,
        can_reply,
        can_edit,
        can_delete,
        react_picker_open,
        msg,
    ) {
        action = menu_action;
    }

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
                let mut names: Vec<_> = nicks.iter().cloned().collect();
                names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                let tip = {
                    let shown: Vec<_> = names.iter().take(12).cloned().collect();
                    let extra = count.saturating_sub(shown.len());
                    let mut s = format!("reacted with {shown_emoji}: {}", shown.join(", "));
                    if extra > 0 {
                        s.push_str(&format!(" +{extra} more"));
                    }
                    s
                };
                let chip_id = ui.id().with("react_chip").with(&msg.id).with(emoji.as_str());
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
                let (long_press, suppress_click) = if touch_ui {
                    touch_long_press(ui, chip_id.with("press"), chip.response.rect)
                } else {
                    (false, false)
                };
                let popup_id = chip_id.with("reactors");
                reaction_reactors_popup(
                    ui,
                    th,
                    popup_id,
                    chip.response.rect,
                    emoji,
                    &names,
                    long_press,
                );
                let resp = ui
                    .interact(chip.response.rect, chip_id, Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(tip);
                if can_react && resp.clicked() && !suppress_click && !long_press {
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

/// Inline video card width (message bubble is full-width; player stays compact).
const EMBED_VIDEO_MAX_W: f32 = 280.0;
/// Cap image height when filling the bubble; video uses 16:9 within its max W.
const EMBED_MEDIA_MAX_H: f32 = 420.0;
const EMBED_VIDEO_MAX_H: f32 = 220.0;

/// Letterbox size for an inline chat image (contain — never crop).
///
/// Sizes are in egui points: texel counts are divided by `pixels_per_point`
/// so high-DPI Android layouts match the lightbox viewer.
fn embed_image_display_size(
    width: usize,
    height: usize,
    max_w: f32,
    max_h: f32,
    pixels_per_point: f32,
) -> Vec2 {
    let ppp = pixels_per_point.max(1.0);
    let native_w = (width.max(1) as f32 / ppp).max(1.0);
    let native_h = (height.max(1) as f32 / ppp).max(1.0);
    let scale = (max_w / native_w).min(max_h / native_h);
    Vec2::new(
        (native_w * scale).max(1.0),
        (native_h * scale).max(1.0),
    )
}

/// Resolved message body line for bubble rendering.
enum MessageBodyLine {
    /// Visible text (caption-only for image/video when prose exists).
    Text(String),
    /// Whitespace-only / cleared message.
    Cleared,
    /// URL-only link message — OG card carries the content.
    Skip,
}

/// Caption text for image/video embeds (URL stripped). `None` when URL-only.
fn media_embed_caption(text: &str, embed: Option<&Embed>) -> Option<String> {
    let embed = embed?;
    let url = match embed {
        Embed::Image { url } | Embed::Video { url } => url.as_str(),
        Embed::Link { .. } => return None,
    };
    let caption = preview::caption_without_embed_url(text, url);
    if caption.is_empty() {
        None
    } else {
        Some(caption)
    }
}

/// Choose visible bubble body text (strip bare media URLs; flag cleared bodies).
fn message_body_line(text: &str, embed: Option<&Embed>) -> MessageBodyLine {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return MessageBodyLine::Cleared;
    }
    if let Some(embed) = embed {
        if preview::is_embed_only_message(trimmed, Some(embed)) {
            MessageBodyLine::Skip
        } else {
            match embed {
                Embed::Image { url } | Embed::Video { url } => {
                    MessageBodyLine::Text(preview::caption_without_embed_url(trimmed, url))
                }
                Embed::Link { .. } => MessageBodyLine::Text(trimmed.to_string()),
            }
        }
    } else {
        MessageBodyLine::Text(trimmed.to_string())
    }
}

/// Render a message body line. Plain text uses `RichText` (reliable on GLES);
/// linkified bodies use `Label::wrap()` (same as search hits) with a separate
/// galley for URL hit-testing.
fn render_message_body(
    ui: &mut egui::Ui,
    th: &Theme,
    visible: &str,
    cleared: bool,
    wrap_w: f32,
    link_spans: &[MessageLinkSpan],
) -> (egui::Response, Option<std::sync::Arc<egui::Galley>>, egui::Pos2) {
    let p = &th.palette;
    if link_spans.is_empty() {
        let resp = if cleared {
            ui.add(
                egui::Label::new(
                    RichText::new(visible)
                        .size(th.type_scale.caption)
                        .color(p.text_secondary)
                        .italics(),
                )
                .wrap(),
            )
        } else {
            ui.add(
                egui::Label::new(
                    RichText::new(visible)
                        .size(th.type_scale.body)
                        .color(p.text),
                )
                .wrap(),
            )
        };
        let origin = resp.rect.min;
        return (resp, None, origin);
    }

    let mut job = linkify_layout_job(visible, link_spans, th);
    if cleared {
        if let Some(section) = job.sections.first_mut() {
            section.format.color = p.text_secondary;
            section.format.italics = true;
        }
    }
    let resp = ui.add(
        egui::Label::new(job)
            .wrap()
            .sense(Sense::click())
            .selectable(false),
    );
    let origin = resp.rect.min;
    // Pre-layout galley at the bubble's inner width for URL tap hit-testing.
    let mut hit_job = linkify_layout_job(visible, link_spans, th);
    if cleared {
        if let Some(section) = hit_job.sections.first_mut() {
            section.format.color = p.text_secondary;
            section.format.italics = true;
        }
    }
    hit_job.wrap.max_width = wrap_w;
    let galley = ui.fonts(|f| f.layout_job(hit_job));
    (resp, Some(galley), origin)
}

/// LayoutJob with http(s) and channel spans styled as accent underlines.
fn linkify_layout_job(text: &str, spans: &[MessageLinkSpan], th: &Theme) -> LayoutJob {
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
        let start = span.start();
        if start > cursor && start <= text.len() {
            job.append(&text[cursor..start], 0.0, base.clone());
        }
        let end = span.end().min(text.len());
        if start < end {
            job.append(&text[start..end], 0.0, link.clone());
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

/// Click target under a pointer position within a laid-out message body galley.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MessageLinkTarget {
    Url(String),
    Channel(String),
}

/// URL or channel under a pointer position within a laid-out message body galley.
fn link_at_galley_pos(
    galley: &egui::Galley,
    galley_origin: egui::Pos2,
    text: &str,
    spans: &[MessageLinkSpan],
    pos: egui::Pos2,
) -> Option<MessageLinkTarget> {
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
    let hit = spans
        .iter()
        .find(|s| byte_idx >= s.start() && byte_idx < s.end())
        .or_else(|| {
            let prev = byte_idx.saturating_sub(1);
            spans
                .iter()
                .find(|s| byte_idx == s.end() && prev >= s.start() && prev < s.end())
        });
    hit.map(|s| match s {
        MessageLinkSpan::Url(u) => MessageLinkTarget::Url(u.url.clone()),
        MessageLinkSpan::Channel(c) => MessageLinkTarget::Channel(c.channel.clone()),
    })
}

/// Inline chat image. Returns `true` when the user taps to open the lightbox.
fn inline_image_preview(
    ui: &mut egui::Ui,
    th: &Theme,
    media: &mut MediaCache,
    url: &str,
    id_salt: &str,
    caption: Option<&str>,
) -> bool {
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
            media_embed_link_footer(ui, th, url, id_salt, caption);
            false
        }
        Some((tex, width, height)) => {
            let max_w = ui.available_width().max(80.0);
            let display = embed_image_display_size(
                width,
                height,
                max_w,
                EMBED_MEDIA_MAX_H,
                ui.ctx().pixels_per_point(),
            );
            // Paint like the lightbox: full UV, letterboxed rect. `egui::Image` +
            // `fit_to_exact_size` with a synthetic `SizedTexture` size can stretch
            // tall screenshots on high-DPI Android so only a horizontal slice shows.
            let mut resp = None;
            ui.horizontal(|ui| {
                let pad = (ui.available_width() - display.x).max(0.0) * 0.5;
                if pad > 0.0 {
                    ui.add_space(pad);
                }
                let (rect, r) = ui.allocate_exact_size(display, Sense::click());
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().add(
                    egui::epaint::RectShape::filled(rect, sp.radius_sm, Color32::WHITE)
                        .with_texture(tex.id(), uv),
                );
                resp = Some(r);
            });
            let resp = resp
                .expect("inline image row")
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Tap to enlarge");
            media_embed_link_footer(ui, th, url, id_salt, caption);
            resp.clicked()
        }
    }
}

/// Inline video card via [`vidya::video_player`] (muted H.264-in-MP4).
///
/// Unsupported formats (WebM, etc.) keep the play-card look and open externally.
fn inline_video_preview(
    ui: &mut egui::Ui,
    th: &Theme,
    media: &mut MediaCache,
    url: &str,
    id_salt: &str,
    caption: Option<&str>,
) {
    use vidya::{video_player, VideoPlayerAction, VideoPlayerOpts, VideoPlayerState};

    media.touch_video(url);

    let sp = &th.spacing;
    // Compact player — bubble is full-width; don't stretch the video surface.
    let max_w = ui.available_width().min(EMBED_VIDEO_MAX_W).max(120.0);
    let max_h = (max_w * 9.0 / 16.0).min(EMBED_VIDEO_MAX_H).max(72.0);

    match media.videos.get(url) {
        Some(crate::state::VideoState::Loading) | None => {
            egui::Frame::new()
                .fill(th.palette.headerbar_bg)
                .corner_radius(sp.radius_sm)
                .inner_margin(egui::Margin::symmetric(12, 16))
                .show(ui, |ui| {
                    ui.set_width(max_w);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(sp.sm);
                        dim_label(ui, th, "Loading video…");
                    });
                });
            media_embed_link_footer(ui, th, url, id_salt, caption);
            return;
        }
        Some(crate::state::VideoState::Failed) => {
            // Fall back to the open-in-browser card without bytes.
            video_open_fallback(ui, th, url, id_salt);
            return;
        }
        Some(crate::state::VideoState::Ready(_)) => {}
    }

    // Clone Arc so we can mutably borrow the player map separately.
    let bytes = match media.videos.get(url) {
        Some(crate::state::VideoState::Ready(b)) => b.clone(),
        _ => return,
    };

    // Per-bubble player state — same URL in two messages must not share playhead.
    let player_key = format!("{url}\0{id_salt}");
    let player = media
        .video_players
        .entry(player_key)
        .or_insert_with(VideoPlayerState::new);
    player.load_bytes(ui.ctx(), (url, id_salt), bytes);

    let opts = VideoPlayerOpts {
        max_width: max_w,
        max_height: max_h,
        title: caption
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        open_url_on_unsupported: Some(url.to_string()),
    };
    let playing = player.is_playing();
    let (resp, action) = video_player(ui, th, player, &opts);
    // High-contrast play affordance — theme accent blends into solid-blue
    // first frames; paint a dark circle + white triangle on top.
    if !playing {
        paint_video_play_affordance(ui, resp.rect);
    }
    if action == VideoPlayerAction::OpenExternally {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    media_embed_link_footer(ui, th, url, id_salt, caption);
}

/// Dark play circle that stays visible on blue / accent-tinted video frames.
fn paint_video_play_affordance(ui: &egui::Ui, rect: Rect) {
    if !rect.is_positive() {
        return;
    }
    let play_r = (rect.height() * 0.18).clamp(18.0, 32.0);
    let center = rect.center();
    let painter = ui.painter();
    painter.circle_filled(
        center,
        play_r,
        Color32::from_rgba_unmultiplied(20, 20, 24, 220),
    );
    painter.circle_stroke(
        center,
        play_r,
        Stroke::new(1.5_f32, Color32::from_rgb(245, 245, 250)),
    );
    let tri_w = play_r * 0.7;
    let tri_h = play_r * 0.85;
    let tip = Pos2::new(center.x + tri_w * 0.55, center.y);
    let top = Pos2::new(center.x - tri_w * 0.45, center.y - tri_h * 0.5);
    let bot = Pos2::new(center.x - tri_w * 0.45, center.y + tri_h * 0.5);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, bot, top],
        Color32::from_rgb(255, 255, 255),
        Stroke::NONE,
    ));
}

/// Play-card that only opens the URL (fetch/decode failed).
fn video_open_fallback(ui: &mut egui::Ui, th: &Theme, url: &str, id_salt: &str) {
    let p = &th.palette;
    let sp = &th.spacing;

    let max_w = ui.available_width().min(EMBED_VIDEO_MAX_W).max(120.0);
    let height = (max_w * 9.0 / 16.0).min(EMBED_VIDEO_MAX_H).max(72.0);
    let size = Vec2::new(max_w, height);
    let card_id = ui.id().with("video_fallback").with(id_salt);

    let frame_resp = egui::Frame::new()
        .fill(Color32::from_rgb(12, 12, 14))
        .stroke(Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_sm)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter()
                .rect_filled(rect, sp.radius_sm, Color32::from_rgb(18, 18, 22));
            paint_video_play_affordance(ui, rect);
            let name = preview::display_filename(url);
            let foot_h = (th.type_scale.caption + 10.0).min(height * 0.28);
            let foot = Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom() - foot_h),
                rect.right_bottom(),
            );
            let r = sp.radius_sm as u8;
            ui.painter().rect_filled(
                foot,
                egui::CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: r,
                    se: r,
                },
                Color32::from_rgba_unmultiplied(0, 0, 0, 160),
            );
            ui.painter().text(
                Pos2::new(foot.left() + 8.0, foot.center().y),
                Align2::LEFT_CENTER,
                name,
                FontId::proportional(th.type_scale.caption),
                Color32::from_rgb(230, 230, 235),
            );
        });

    let resp = ui
        .interact(frame_resp.response.rect, card_id, Sense::click())
        .on_hover_text("Open video")
        .on_hover_cursor(CursorIcon::PointingHand);
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// Clickable full URL under inline image/video embeds (optional caption above).
fn media_embed_link_footer(
    ui: &mut egui::Ui,
    th: &Theme,
    url: &str,
    id_salt: &str,
    caption: Option<&str>,
) {
    let p = &th.palette;
    let sp = &th.spacing;
    let link_line = url.trim();

    ui.add_space(sp.xs);
    ui.push_id(id_salt, |ui| {
        if let Some(caption) = caption.filter(|s| !s.is_empty()) {
            ui.label(
                RichText::new(caption)
                    .size(th.type_scale.caption)
                    .color(p.text)
                    .strong(),
            );
        }
        let resp = ui
            .add(
                egui::Label::new(
                    RichText::new(link_line)
                        .size(th.type_scale.caption)
                        .color(p.accent),
                )
                .wrap()
                .sense(Sense::click()),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Open in browser");
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    });
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

    // Area paints over the full screen (including under system bars). Pad
    // chrome / image by measured status + nav insets so Close / View original
    // clear the Android clock / cutout and the image clears the gesture bar.
    let safe = system_chrome(ctx);

    egui::Area::new(egui::Id::new("image_lightbox"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (full, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(full, 0.0, Color32::from_black_alpha(220));

            // Top chrome: Close (left) + View original (right), below status bar.
            let chrome_h = (sp.control_height + sp.md * 2.0).max(48.0);
            let chrome = egui::Rect::from_min_size(
                full.min + Vec2::new(sp.md, safe.top + sp.md),
                Vec2::new((full.width() - sp.md * 2.0).max(1.0), chrome_h),
            );

            // Image area: remaining viewport below chrome, with padding + nav inset.
            let img_pad = sp.md;
            let img_area = egui::Rect::from_min_max(
                egui::pos2(full.left() + img_pad, chrome.bottom() + sp.sm),
                egui::pos2(
                    full.right() - img_pad,
                    full.bottom() - img_pad - safe.bottom(),
                ),
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
                    //
                    // Size in *points* from texel count ÷ pixels_per_point so a
                    // high-DPI screen doesn't implicitly upscale (and blur) a
                    // texture that already fills the physical viewport.
                    let ppp = ui.ctx().pixels_per_point().max(1.0);
                    let max_w = img_area.width().max(1.0);
                    let max_h = img_area.height().max(1.0);
                    let nw = width.max(1) as f32;
                    let nh = height.max(1) as f32;
                    let native_w = (nw / ppp).max(1.0);
                    let native_h = (nh / ppp).max(1.0);
                    // Fit to the viewer; allow modest upscale for tiny embeds only.
                    let scale = (max_w / native_w).min(max_h / native_h).min(1.5);
                    let size = Vec2::new((native_w * scale).max(1.0), (native_h * scale).max(1.0));
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

#[cfg(test)]
mod embed_image_tests {
    use super::*;

    #[test]
    fn tall_image_shrinks_width_instead_of_cropping() {
        // Phone screenshot aspect — must not fill bubble width at max height.
        let size = embed_image_display_size(1080, 5000, 360.0, 420.0, 3.0);
        assert!(
            size.x < 360.0,
            "portrait preview should letterbox horizontally: {size:?}"
        );
        assert!((size.y - 420.0).abs() < 0.01, "height should hit cap: {size:?}");
        let aspect = size.x / size.y;
        let native_aspect = (1080.0 / 3.0) / (5000.0 / 3.0);
        assert!((aspect - native_aspect).abs() < 0.001, "aspect preserved");
    }

    #[test]
    fn wide_image_fills_width() {
        let size = embed_image_display_size(1600, 900, 360.0, 420.0, 2.0);
        assert!((size.x - 360.0).abs() < 0.01);
        assert!(size.y < 420.0);
    }

    #[test]
    fn pixels_per_point_does_not_change_aspect() {
        let a = embed_image_display_size(800, 2400, 320.0, 420.0, 1.0);
        let b = embed_image_display_size(800, 2400, 320.0, 420.0, 3.0);
        assert!((a.x / a.y - b.x / b.y).abs() < 0.001);
    }
}
