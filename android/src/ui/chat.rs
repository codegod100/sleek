//! Chat detail — messages + compose (freeq-android ChatDetailScreen inspired).

use eframe::egui::{self, Align, Align2, CursorIcon, Layout, RichText, Sense, ScrollArea, Vec2};
use vidya::{button, dim_label, primary_button, title_2, Theme};

use crate::clipboard;
use crate::state::AppState;
use crate::ui::widgets::{avatar_circle, empty_state, message_bubble};

pub enum ChatAction {
    None,
    Back,
    Send { target: String, text: String },
    Part(String),
    /// Open a direct message with this nick (from the user list).
    OpenDm(String),
    /// Start a freeq AV call in this channel.
    AvStart(String),
    /// Join the active call (`session_id`).
    AvJoin { channel: String, session_id: String },
    /// Leave our local call.
    AvLeave,
    /// Toggle mic mute.
    AvToggleMute,
    /// Toggle camera publish (desktop MoQ).
    AvToggleCamera,
    /// Jump to the channel that holds our active call.
    OpenCallChannel(String),
}

pub fn chat_screen(ui: &mut egui::Ui, th: &Theme, state: &mut AppState, channel: &str) -> ChatAction {
    let mut action = ChatAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    // Snapshot buffer data we need (avoid holding borrow across mut compose)
    let (topic, member_count, mut members, messages, is_channel, join_pending, join_error, channel_call) = {
        let buf = state.channels.get(channel);
        match buf {
            Some(b) => (
                b.topic.clone(),
                b.members.len(),
                b.members.clone(),
                b.messages.iter().cloned().collect::<Vec<_>>(),
                b.is_channel(),
                b.join_pending,
                b.join_error.clone(),
                b.call.clone(),
            ),
            None => (
                String::new(),
                0,
                Vec::new(),
                Vec::new(),
                channel.starts_with('#') || channel.starts_with('&'),
                false,
                None,
                None,
            ),
        }
    };
    let local_in_call = state
        .local_call
        .as_ref()
        .is_some_and(|lc| lc.channel.eq_ignore_ascii_case(channel));
    members.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    let own_nick = state.nick.clone();
    let is_guest = state.did.is_none();
    let joined = is_channel && join_error.is_none() && !join_pending;
    // Member list only when we actually got into the room.
    let show_members = state.show_members && is_channel && joined;
    let header_title = if is_channel {
        channel.to_string()
    } else {
        state.display_name_for(channel)
    };

    // Header
    ui.horizontal(|ui| {
        if button(ui, th, "←").clicked() {
            if show_members {
                state.show_members = false;
            } else {
                action = ChatAction::Back;
            }
        }
        ui.add_space(sp.sm);
        ui.vertical(|ui| {
            if show_members {
                title_2(ui, th, "Users");
                dim_label(ui, th, &format!("{member_count} in {channel}"));
            } else {
                title_2(ui, th, &header_title);
                if is_channel {
                    let sub = if let Some(err) = join_error.as_ref() {
                        // Short header status; full reason is in the body empty-state.
                        if err.to_ascii_lowercase().contains("authentication")
                            || err.to_ascii_lowercase().contains("sign in")
                        {
                            if is_guest {
                                "Guests not allowed".to_string()
                            } else {
                                "Join denied".to_string()
                            }
                        } else {
                            "Can't join".to_string()
                        }
                    } else if join_pending {
                        "Joining…".to_string()
                    } else if topic.is_empty() {
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
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if is_channel && !show_members {
                let leave_label = if join_error.is_some() {
                    "Dismiss"
                } else {
                    "Leave"
                };
                if button(ui, th, leave_label).clicked() {
                    action = ChatAction::Part(channel.to_string());
                }
                // freeq-android: Group icon toggles member list. Dismiss with ←.
                if joined {
                    let users_label = if member_count > 0 {
                        format!("Users ({})", member_count.min(999))
                    } else {
                        "Users".to_string()
                    };
                    if button(ui, th, &users_label).clicked() {
                        state.show_members = true;
                    }
                }
            }
        });
    });
    ui.add_space(sp.sm);
    ui.separator();
    ui.add_space(sp.sm);

    // ── AV call banner (start / join only; active call chrome is global) ─
    // The MoQ section for an open call lives in the app-level top panel so it
    // stays visible on every route — only idle/join controls stay here.
    if is_channel && joined && !local_in_call {
        let elsewhere = state
            .local_call
            .as_ref()
            .filter(|lc| !lc.channel.eq_ignore_ascii_case(channel))
            .cloned();
        if let Some(act) = av_call_banner(
            ui,
            th,
            channel,
            channel_call.as_ref(),
            elsewhere.as_ref(),
        ) {
            action = act;
        }
        ui.add_space(sp.sm);
    }

    // Join denied (e.g. guest on #policytest) — full-screen error, no compose.
    if let Some(err) = join_error.as_ref() {
        let guest_auth = is_guest
            && (err.to_ascii_lowercase().contains("authentication")
                || err.to_ascii_lowercase().contains("sign in")
                || err.to_ascii_lowercase().contains("policy"));
        let title = if guest_auth {
            "Guests can't join"
        } else {
            "Can't join this channel"
        };
        empty_state(ui, th, title, err);
        ui.add_space(sp.md);
        ui.vertical_centered(|ui| {
            if guest_auth {
                dim_label(
                    ui,
                    th,
                    "Sign in with Bluesky from Settings, then try again.",
                );
                ui.add_space(sp.sm);
            }
            if button(ui, th, "Dismiss").clicked() {
                action = ChatAction::Part(channel.to_string());
            }
        });
        return action;
    }

    // Optimistic open while JOIN is in flight.
    if join_pending && messages.is_empty() {
        empty_state(ui, th, "Joining…", &format!("Connecting to {channel}"));
        return action;
    }

    if show_members {
        // Member list (freeq MemberListSheet-inspired). Header already shows count.
        let list_h = ui.available_height().max(120.0);
        ScrollArea::vertical()
            .id_salt("members_scroll")
            .auto_shrink([false, false])
            .max_height(list_h)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    if members.is_empty() {
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            dim_label(ui, th, "No members yet — list still loading.");
                        });
                    } else {
                        let n = members.len();
                        for (i, nick) in members.iter().enumerate() {
                            let is_self = nick.eq_ignore_ascii_case(&own_nick);
                            let row_id = ui.id().with("member_row").with(nick);
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
                                .inner_margin(egui::Margin::symmetric(
                                    sp.md as i8,
                                    sp.sm as i8 + 2,
                                ))
                                .show(ui, |ui| {
                                    let w = ui.available_width();
                                    ui.set_min_width(w);
                                    ui.set_max_width(w);
                                    ui.horizontal(|ui| {
                                        avatar_circle(ui, th, nick, 36.0);
                                        ui.add_space(sp.md);
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(nick)
                                                    .size(th.type_scale.body)
                                                    .color(p.text)
                                                    .strong(),
                                            );
                                            if is_self {
                                                dim_label(ui, th, "you");
                                            } else {
                                                dim_label(ui, th, "tap to message");
                                            }
                                        });
                                    });
                                });
                            let resp = ui
                                .interact(inner.response.rect, row_id, Sense::click())
                                .on_hover_cursor(if is_self {
                                    CursorIcon::Default
                                } else {
                                    CursorIcon::PointingHand
                                });
                            if resp.clicked() && !is_self {
                                state.show_members = false;
                                action = ChatAction::OpenDm(nick.clone());
                            }
                            if i + 1 < n {
                                ui.add_space(sp.xs);
                                let y = ui.cursor().top();
                                ui.painter().hline(
                                    inner.response.rect.left()..=inner.response.rect.right(),
                                    y,
                                    egui::Stroke::new(1.0_f32, p.border_soft),
                                );
                                ui.add_space(sp.xs);
                            }
                        }
                    }
                    ui.add_space(sp.md);
                });
            });
        return action;
    }

    // Pin compose to the bottom of the remaining panel; messages fill above.
    //
    // TopBottomPanel::bottom().show_inside takes space from the bottom of this
    // Ui and shrinks available_height for what follows — so the scroll area
    // always gets the leftover strip. The earlier bottom_up + fixed-rect approach
    // left a huge dead zone under the input (compose sat under the first message).
    try_paste_compose_image(ui, state);

    let body_w = ui.available_width().max(80.0);

    let has_image = state.compose_image.is_some();
    let uploading = state.compose_uploading;
    let can_send = !uploading && (!state.compose.trim().is_empty() || has_image);
    let pick_busy = state.file_pick_busy();

    egui::TopBottomPanel::bottom("chat_compose")
        .resizable(false)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(p.headerbar_bg)
                .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
                .corner_radius(sp.radius_md)
                .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2)),
        )
        .show_inside(ui, |ui| {
            ui.set_min_width(body_w);
            ui.set_max_width(body_w);
            let inner_w = ui.available_width();
            ui.set_width(inner_w);

            if has_image {
                if let Some(send) = compose_image_composer(ui, th, state) {
                    action = ChatAction::Send {
                        target: channel.to_string(),
                        text: send,
                    };
                }
            } else {
                // One control-height row: attach | field | Send, cross-aligned center.
                ui.horizontal(|ui| {
                    ui.set_width(inner_w);
                    let control_h = th.spacing.control_height;
                    let attach_w = 40.0_f32;
                    let send_w = 72.0_f32;
                    let field_w =
                        (ui.available_width() - attach_w - send_w - sp.sm * 2.0).max(48.0);

                    let attach = ui
                        .add_sized(
                            Vec2::new(attach_w, control_h),
                            egui::Button::new(
                                RichText::new("🖼")
                                    .size(th.type_scale.body)
                                    .color(p.button_fg),
                            )
                            .fill(p.button_bg)
                            .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
                            .corner_radius(sp.radius_md),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(if pick_busy {
                            "Opening file picker…"
                        } else if cfg!(target_os = "android") {
                            "Attach image"
                        } else {
                            "Attach image (or paste with Ctrl+V)"
                        });
                    if attach.clicked() && !pick_busy {
                        state.start_file_pick();
                    }

                    ui.add_space(sp.sm);
                    // Fixed single-line height; Shift+Enter inserts a newline, bare Enter sends.
                    // Center text vertically so the hint matches attach/Send baselines
                    // (multiline defaults to TOP; button labels are centered).
                    let te = egui::TextEdit::multiline(&mut state.compose)
                        .margin(th.text_edit_margin())
                        .desired_width(field_w)
                        .desired_rows(1)
                        .min_size(Vec2::new(field_w, control_h))
                        .vertical_align(Align::Center)
                        .hint_text("Message…")
                        .return_key(egui::KeyboardShortcut::new(
                            egui::Modifiers::SHIFT,
                            egui::Key::Enter,
                        ));
                    let resp = ui.add_sized(Vec2::new(field_w, control_h), te);
                    if state.focus_compose {
                        resp.request_focus();
                        state.focus_compose = false;
                    }
                    let enter = resp.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

                    ui.add_space(sp.sm);
                    let send_clicked = ui
                        .add_sized(
                            Vec2::new(send_w, control_h),
                            egui::Button::new(
                                RichText::new("Send")
                                    .size(th.type_scale.body)
                                    .color(p.accent_fg),
                            )
                            .fill(p.accent)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(sp.radius_md)
                            .min_size(Vec2::new(send_w, control_h)),
                        )
                        .clicked()
                        && can_send;

                    if (send_clicked || enter) && can_send {
                        action = ChatAction::Send {
                            target: channel.to_string(),
                            text: state.compose.trim().to_string(),
                        };
                    }
                });
            }
        });

    // Messages take everything above the compose bar.
    let msg_h = ui.available_height().max(80.0);
    let jump_id = egui::Id::new("chat_jump_to_bottom").with(channel);
    let want_jump = ui.ctx().data_mut(|d| d.get_temp::<bool>(jump_id).unwrap_or(false));

    // Do **not** pair this with `vertical_scroll_offset(f32::MAX)`: that value
    // destroys f32 layout precision, so `scroll_to_cursor` computes a bogus
    // target (often ~top) and the jump appears to do nothing / go the wrong way.
    // Instant animation so kinetic fling velocity is cleared and we re-stick.
    let scroll_out = ScrollArea::vertical()
        .id_salt("chat_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .max_height(msg_h)
        .show(ui, |ui| {
            ui.set_min_width(body_w);
            if messages.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    dim_label(ui, th, "No messages yet — say hello.");
                });
            } else {
                for msg in &messages {
                    message_bubble(ui, th, msg, &own_nick, &mut state.media);
                    ui.add_space(sp.sm);
                }
            }
            ui.add_space(sp.md);
            if want_jump {
                ui.scroll_to_cursor_animation(
                    Some(Align::BOTTOM),
                    egui::style::ScrollAnimation::none(),
                );
            }
        });

    // Floating jump control when the user has scrolled away from the latest messages.
    let max_offset = (scroll_out.content_size.y - scroll_out.inner_rect.height()).max(0.0);
    const NEAR_BOTTOM_PX: f32 = 64.0;
    let near_bottom =
        max_offset <= NEAR_BOTTOM_PX || scroll_out.state.offset.y >= max_offset - NEAR_BOTTOM_PX;

    // Consume the jump flag only once we're actually near the bottom (or there
    // is nothing to scroll). Keep requesting repaint until the scroll lands.
    if want_jump {
        if near_bottom || messages.is_empty() {
            ui.ctx().data_mut(|d| d.insert_temp(jump_id, false));
        } else {
            ui.ctx().request_repaint();
        }
    }

    if !near_bottom && !messages.is_empty() {
        let anchor = egui::pos2(
            scroll_out.inner_rect.center().x,
            scroll_out.inner_rect.bottom() - sp.sm,
        );
        egui::Area::new(egui::Id::new("chat_jump_btn").with(channel))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .pivot(Align2::CENTER_BOTTOM)
            .show(ui.ctx(), |ui| {
                let label = RichText::new("↓ Jump to bottom")
                    .size(th.type_scale.caption)
                    .color(p.text)
                    .strong();
                let resp = ui
                    .add(
                        egui::Button::new(label)
                            .fill(p.card_bg)
                            .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
                            .corner_radius(sp.radius_md)
                            .min_size(Vec2::new(0.0, 36.0)),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(jump_id, true));
                    ui.ctx().request_repaint();
                }
            });
    }

    action
}

/// Image attachment preview + caption/submit row (mirrors the normal compose bar).
///
/// Returns `Some(caption)` when the user submits.
fn compose_image_composer(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
) -> Option<String> {
    let p = &th.palette;
    let sp = &th.spacing;

    // Snapshot texture / dims first so we don't hold a mut borrow across clicks
    // that clear `compose_image`.
    let (tex_id, width, height, uploading) = {
        let Some(img) = state.compose_image.as_mut() else {
            return None;
        };
        let tex = img.texture(ui.ctx()).clone();
        (tex.id(), img.width, img.height, state.compose_uploading)
    };

    // Compact thumb — keep the bar short so messages stay visible.
    const THUMB: f32 = 48.0;
    let scale = (THUMB / width.max(1) as f32)
        .min(THUMB / height.max(1) as f32)
        .min(1.0);
    let img_size = Vec2::new(
        (width as f32 * scale).max(1.0),
        (height as f32 * scale).max(1.0),
    );

    let kb = (width.saturating_mul(height).saturating_mul(4) / 1024).max(1);
    let dims = format!("{width}×{height} · ~{kb} KB");

    let mut clear = false;
    let mut submit = false;
    let pick_busy = state.file_pick_busy();
    let can_send = !uploading; // image alone is enough
    let bar_w = ui.available_width();

    // Preview card only (caption lives on the shared compose row below).
    egui::Frame::new()
        .fill(p.view_bg)
        .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(sp.sm as i8, sp.sm as i8))
        .show(ui, |ui| {
            let inner_w = ui.available_width().min(bar_w).max(48.0);
            ui.set_width(inner_w);
            ui.horizontal(|ui| {
                ui.set_width(inner_w);

                let (thumb_rect, _) =
                    ui.allocate_exact_size(Vec2::splat(THUMB), Sense::hover());
                ui.painter()
                    .rect_filled(thumb_rect, sp.radius_sm, p.card_bg);
                let img_rect = egui::Rect::from_center_size(thumb_rect.center(), img_size);
                ui.put(
                    img_rect,
                    egui::Image::new((tex_id, img_size))
                        .corner_radius(sp.radius_sm)
                        .sense(Sense::hover()),
                );

                ui.add_space(sp.sm);

                let dismiss_w = 32.0_f32;
                let meta_w = (ui.available_width() - dismiss_w - sp.xs).max(48.0);
                ui.vertical(|ui| {
                    ui.set_width(meta_w);
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(if uploading {
                            "Uploading…"
                        } else {
                            "Image attached"
                        })
                        .size(th.type_scale.body)
                        .color(if uploading { p.accent } else { p.text })
                        .strong(),
                    );
                    ui.add_space(2.0);
                    dim_label(ui, th, &dims);
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if uploading {
                        ui.spinner();
                    } else {
                        let dismiss = ui
                            .add_sized(
                                Vec2::splat(28.0),
                                egui::Button::new(
                                    RichText::new("✕")
                                        .size(14.0)
                                        .color(p.text_secondary),
                                )
                                .fill(p.button_bg)
                                .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
                                .corner_radius(sp.radius_sm),
                            )
                            .on_hover_text("Remove image")
                            .on_hover_cursor(CursorIcon::PointingHand);
                        if dismiss.clicked() {
                            clear = true;
                        }
                    }
                });
            });
        });

    ui.add_space(sp.sm);

    // Same row shape as the no-image compose bar: attach | caption | Submit.
    ui.horizontal(|ui| {
        ui.set_width(bar_w);
        let control_h = th.spacing.control_height;
        let attach_w = 40.0_f32;
        let send_w = 80.0_f32;
        let field_w = (ui.available_width() - attach_w - send_w - sp.sm * 2.0).max(48.0);

        let attach = ui
            .add_sized(
                Vec2::new(attach_w, control_h),
                egui::Button::new(
                    RichText::new("🖼")
                        .size(th.type_scale.body)
                        .color(p.button_fg),
                )
                .fill(p.button_bg)
                .stroke(egui::Stroke::new(1.0_f32, p.border_soft))
                .corner_radius(sp.radius_md),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(if pick_busy {
                "Opening file picker…"
            } else if cfg!(target_os = "android") {
                "Replace image"
            } else {
                "Replace image (or paste with Ctrl+V)"
            });
        if attach.clicked() && !uploading && !pick_busy {
            state.start_file_pick();
        }

        ui.add_space(sp.sm);

        // Single-line height matching attach/Submit; center text with the buttons.
        let te = egui::TextEdit::multiline(&mut state.compose)
            .margin(th.text_edit_margin())
            .desired_width(field_w)
            .desired_rows(1)
            .min_size(Vec2::new(field_w, control_h))
            .vertical_align(Align::Center)
            .hint_text("Caption (optional)…")
            .interactive(!uploading)
            .return_key(egui::KeyboardShortcut::new(
                egui::Modifiers::SHIFT,
                egui::Key::Enter,
            ));
        let resp = ui.add_sized(Vec2::new(field_w, control_h), te);
        if state.focus_compose && !uploading {
            resp.request_focus();
            state.focus_compose = false;
        }
        let enter = resp.has_focus()
            && !uploading
            && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

        ui.add_space(sp.sm);
        let send_clicked = ui
            .add_sized(
                Vec2::new(send_w, control_h),
                egui::Button::new(
                    RichText::new("Submit")
                        .size(th.type_scale.body)
                        .color(p.accent_fg),
                )
                .fill(p.accent)
                .stroke(egui::Stroke::NONE)
                .corner_radius(sp.radius_md)
                .min_size(Vec2::new(send_w, control_h)),
            )
            .clicked()
            && can_send;
        if (send_clicked || enter) && can_send {
            submit = true;
        }
    });

    if clear {
        state.compose_image = None;
    }

    if submit {
        Some(state.compose.trim().to_string())
    } else {
        None
    }
}

/// If the user pastes (Ctrl/Cmd+V) and the clipboard holds an image, attach it
/// to the compose bar and drop text paste events for this frame.
///
/// Stock egui-winit swallows Ctrl+V when it only tries text clipboard; our
/// vendored patch re-emits `Key::V` when text is missing so image paste works.
/// When both text and image are present, a `Paste` event arrives — we prefer
/// the image and strip the text paste for this frame.
fn try_paste_compose_image(ui: &mut egui::Ui, state: &mut AppState) {
    if state.compose_uploading {
        return;
    }

    let wants_paste = ui.input(|i| {
        let cmd_v = i.key_pressed(egui::Key::V)
            && (i.modifiers.command || i.modifiers.ctrl);
        let paste_ev = i
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::Paste(_)));
        // Dedicated Paste key (some keyboards / OS bindings).
        let paste_key = i.key_pressed(egui::Key::Paste);
        cmd_v || paste_ev || paste_key
    });
    if !wants_paste {
        return;
    }

    let Some(img) = clipboard::try_get_image() else {
        return;
    };

    // Suppress text paste for this frame so garbage doesn't land in compose.
    ui.input_mut(|i| {
        i.events
            .retain(|e| !matches!(e, egui::Event::Paste(_)));
        i.consume_key(egui::Modifiers::COMMAND, egui::Key::V);
        i.consume_key(egui::Modifiers::CTRL, egui::Key::V);
    });

    state.compose_image = Some(img);
    state.focus_compose = true;
}

/// Global MoQ call chrome — always painted while `local_call` is set so the
/// section stays visible on every route (chat, tabs, settings). One call only.
pub fn active_call_panel(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> Option<ChatAction> {
    let Some(lc) = state.local_call.clone() else {
        return None;
    };
    let sp = &th.spacing;
    let p = &th.palette;
    let mut action = None;

    let channel_call = state
        .channels
        .get(&lc.channel)
        .and_then(|b| b.call.clone());
    let n = channel_call
        .as_ref()
        .map(|c| c.participants.max(1))
        .unwrap_or(1);
    let title = channel_call
        .as_ref()
        .and_then(|c| c.title.as_ref())
        .map(|t| t.as_str())
        .unwrap_or("Voice & video");
    let on_call_channel = matches!(
        &state.route,
        crate::state::Route::Chat(ch) if ch.eq_ignore_ascii_case(&lc.channel)
    );

    let frame = egui::Frame::new()
        .fill(p.accent.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0_f32, p.accent.gamma_multiply(0.45)))
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8));

    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("📞 {title}"))
                    .size(th.type_scale.body)
                    .color(p.text)
                    .strong(),
            );
            ui.add_space(sp.sm);
            dim_label(ui, th, &lc.channel);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                dim_label(ui, th, &format!("{n} in call"));
            });
        });
        ui.add_space(sp.xs);
        dim_label(ui, th, &lc.media.label());

        if matches!(lc.media, crate::av::MediaStatus::Live) {
            paint_av_video_tiles(ui, th, state);
        }

        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            let mute_label = if lc.muted { "Unmute" } else { "Mute" };
            if button(ui, th, mute_label).clicked() {
                action = Some(ChatAction::AvToggleMute);
            }
            ui.add_space(sp.sm);
            if lc.has_camera {
                let cam_label = if lc.camera {
                    "Camera off"
                } else {
                    "Camera on"
                };
                if button(ui, th, cam_label).clicked() {
                    action = Some(ChatAction::AvToggleCamera);
                }
                ui.add_space(sp.sm);
            }
            if button(ui, th, "Leave call").clicked() {
                action = Some(ChatAction::AvLeave);
            }
            if !on_call_channel {
                ui.add_space(sp.sm);
                if button(ui, th, "Open chat").clicked() {
                    action = Some(ChatAction::OpenCallChannel(lc.channel.clone()));
                }
            }
        });
    });

    action
}

/// Per-channel call strip when we are *not* the local participant of that call:
/// start (idle), join (session present), or point at our single active call elsewhere.
fn av_call_banner(
    ui: &mut egui::Ui,
    th: &Theme,
    channel: &str,
    channel_call: Option<&crate::av::ChannelCall>,
    elsewhere: Option<&crate::av::LocalCall>,
) -> Option<ChatAction> {
    let sp = &th.spacing;
    let p = &th.palette;
    let mut action = None;

    // Already in the one allowed call (on another channel) — no second start/join.
    if let Some(lc) = elsewhere {
        let frame = egui::Frame::new()
            .fill(p.accent.gamma_multiply(0.10))
            .stroke(egui::Stroke::new(1.0_f32, p.accent.gamma_multiply(0.35)))
            .corner_radius(sp.radius_md)
            .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                dim_label(ui, th, &format!("In call on {}", lc.channel));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button(ui, th, "Leave").clicked() {
                        action = Some(ChatAction::AvLeave);
                    }
                    ui.add_space(sp.sm);
                    if button(ui, th, "Open").clicked() {
                        action = Some(ChatAction::OpenCallChannel(lc.channel.clone()));
                    }
                });
            });
        });
        return action;
    }

    // Idle: offer start. One call at a time is enforced when starting.
    if channel_call.is_none() {
        ui.horizontal(|ui| {
            if button(ui, th, "📞 Call").clicked() {
                action = Some(ChatAction::AvStart(channel.to_string()));
            }
            dim_label(ui, th, "Voice & video over MoQ");
        });
        return action;
    }

    // Session already open on this channel — join only (no parallel "Start new").
    let Some(call) = channel_call else {
        return action;
    };
    let n = call.participants.max(1);
    let title = call
        .title
        .as_ref()
        .map(|t| t.as_str())
        .unwrap_or("Voice & video");

    let frame = egui::Frame::new()
        .fill(p.accent.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0_f32, p.accent.gamma_multiply(0.45)))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8));

    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("📞 {title} · {n} in call"))
                    .size(th.type_scale.body)
                    .color(p.text)
                    .strong(),
            );
        });
        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            if primary_button(ui, th, "Join call").clicked() {
                action = Some(ChatAction::AvJoin {
                    channel: channel.to_string(),
                    session_id: call.session_id.clone(),
                });
            }
        });
    });

    action
}

/// Paint remote (+ local preview) video tiles from the MoQ frame store.
fn paint_av_video_tiles(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) {
    let Some(store) = state.av_video.clone() else {
        return;
    };
    let mut frames = store.snapshot();
    if frames.is_empty() {
        return;
    }
    // Local preview first, then remotes alphabetically.
    frames.sort_by(|a, b| {
        let a_local = a.0 == "__local__";
        let b_local = b.0 == "__local__";
        b_local
            .cmp(&a_local)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let sp = &th.spacing;
    let p = &th.palette;
    ui.add_space(sp.sm);

    let avail = ui.available_width();
    let tile_w = if frames.len() == 1 {
        avail.min(280.0)
    } else {
        ((avail - sp.sm) / 2.0).clamp(100.0, 200.0)
    };
    let tile_h = tile_w * 9.0 / 16.0;

    let live: std::collections::HashSet<String> = frames.iter().map(|(n, _)| n.clone()).collect();
    state.av_video_textures.retain(|k, _| live.contains(k));

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(sp.sm, sp.sm);
        for (nick, frame) in frames {
            let label = if nick == "__local__" {
                "You".to_string()
            } else {
                nick.clone()
            };
            let tex_id = {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    frame.rgba.as_ref(),
                );
                match state.av_video_textures.entry(nick.clone()) {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        e.get_mut().set(color, egui::TextureOptions::LINEAR);
                        e.get().id()
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let tex = ui.ctx().load_texture(
                            format!("av_video_{nick}"),
                            color,
                            egui::TextureOptions::LINEAR,
                        );
                        let id = tex.id();
                        e.insert(tex);
                        id
                    }
                }
            };

            ui.vertical(|ui| {
                let size = Vec2::new(tile_w, tile_h);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let aspect = frame.width as f32 / frame.height.max(1) as f32;
                let fit = if aspect > size.x / size.y {
                    Vec2::new(size.x, size.x / aspect)
                } else {
                    Vec2::new(size.y * aspect, size.y)
                };
                let img_rect = egui::Rect::from_center_size(rect.center(), fit);
                ui.painter()
                    .rect_filled(rect, sp.radius_sm, p.card_bg);
                ui.painter().image(
                    tex_id,
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                ui.painter().text(
                    rect.left_bottom() + Vec2::new(6.0, -4.0),
                    Align2::LEFT_BOTTOM,
                    &label,
                    egui::FontId::proportional(th.type_scale.caption),
                    p.text,
                );
            });
        }
    });
}
