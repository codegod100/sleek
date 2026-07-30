//! Chat detail — messages + compose (freeq-android ChatDetailScreen inspired).

use eframe::egui::{self, Align, Align2, CursorIcon, Layout, RichText, Sense, ScrollArea, Vec2};
use vidya::{button, dim_label, primary_button, Theme};

use crate::clipboard;
use crate::state::AppState;
use crate::ui::widgets::{avatar_circle, empty_state, message_bubble, MessageBubbleAction};

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
    /// Toggle our reaction on a message (`+react` / `+freeq.at/unreact`).
    React {
        target: String,
        msgid: String,
        emoji: String,
    },
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

    // Header — allocate right-side actions first so long channel names
    // (e.g. stream.place:…) truncate with an ellipsis instead of clipping
    // under Leave/Users or past the panel edge.
    ui.horizontal(|ui| {
        if button(ui, th, "←").clicked() {
            if show_members {
                state.show_members = false;
            } else {
                action = ChatAction::Back;
            }
        }
        ui.add_space(sp.sm);
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
            let title_w = ui.available_width().max(40.0);
            ui.allocate_ui_with_layout(
                Vec2::new(title_w, ui.available_height().max(th.type_scale.title_2 * 2.2)),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_max_width(title_w);
                    if show_members {
                        ui.add(
                            egui::Label::new(
                                RichText::new("Users")
                                    .size(th.type_scale.title_2)
                                    .strong()
                                    .color(p.text),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{member_count} in {channel}"))
                                    .size(th.type_scale.caption)
                                    .color(p.text_secondary),
                            )
                            .truncate(),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&header_title)
                                    .size(th.type_scale.title_2)
                                    .strong()
                                    .color(p.text),
                            )
                            .truncate(),
                        );
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
                            ui.add(
                                egui::Label::new(
                                    RichText::new(sub)
                                        .size(th.type_scale.caption)
                                        .color(p.text_secondary),
                                )
                                .truncate(),
                            );
                        } else {
                            dim_label(ui, th, "Direct message");
                        }
                    }
                },
            );
        });
    });
    ui.add_space(sp.sm);
    ui.separator();
    ui.add_space(sp.sm);

    // ── AV call banner (start / join only; active call chrome is global) ─
    // The MoQ section for an open call lives in the app-level top panel so it
    // stays visible on every route — only idle/join controls stay here.
    // Mic/camera prefs live on the banner so they can be set before joining.
    // When already in a call (this channel or another), skip entirely: the
    // global panel has mute/camera/leave/open — a second "In call on …" strip
    // was redundant.
    if is_channel && joined && state.local_call.is_none() {
        let prev_muted = state.av_pref_muted;
        let prev_camera = state.av_pref_camera;
        if let Some(act) = av_call_banner(
            ui,
            th,
            channel,
            channel_call.as_ref(),
            &mut state.av_pref_muted,
            &mut state.av_pref_camera,
        ) {
            action = act;
        }
        if state.av_pref_muted != prev_muted || state.av_pref_camera != prev_camera {
            state.persist_av_prefs();
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
            // Use the frame's *inner* width only. Forcing `body_w` (outer strip
            // before margins) overflows the clip rect and chops the Send button.
            let inner_w = ui.available_width().max(1.0);
            ui.set_min_width(inner_w);
            ui.set_max_width(inner_w);

            if has_image {
                if let Some(send) = compose_image_composer(ui, th, state) {
                    action = ChatAction::Send {
                        target: channel.to_string(),
                        text: send,
                    };
                }
            } else {
                let attach_tip = if pick_busy {
                    "Opening file picker…"
                } else if cfg!(target_os = "android") {
                    "Attach image"
                } else {
                    "Attach image (or paste with Ctrl+V)"
                };
                let (resp, attach_clicked, send_clicked) = compose_input_row(
                    ui,
                    th,
                    &mut state.compose,
                    "Message…",
                    "Send",
                    72.0,
                    attach_tip,
                    true,
                );
                if attach_clicked && !pick_busy {
                    state.start_file_pick();
                }
                if state.focus_compose {
                    resp.request_focus();
                    state.focus_compose = false;
                }
                let enter = resp.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if (send_clicked || enter) && can_send {
                    action = ChatAction::Send {
                        target: channel.to_string(),
                        text: state.compose.trim().to_string(),
                    };
                }
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
                    let picker_open = state
                        .react_picker_msg
                        .as_ref()
                        .is_some_and(|id| id == &msg.id);
                    match message_bubble(
                        ui,
                        th,
                        msg,
                        &own_nick,
                        &mut state.media,
                        picker_open,
                        &mut state.react_picker_search,
                        &mut state.react_picker_group,
                    ) {
                        MessageBubbleAction::None => {}
                        MessageBubbleAction::ToggleReaction { msgid, emoji } => {
                            state.close_react_picker();
                            action = ChatAction::React {
                                target: channel.to_string(),
                                msgid,
                                emoji,
                            };
                        }
                        MessageBubbleAction::OpenReactPicker { msgid } => {
                            state.open_react_picker(msgid);
                        }
                        MessageBubbleAction::CloseReactPicker => {
                            state.close_react_picker();
                        }
                    }
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
    let attach_tip = if pick_busy {
        "Opening file picker…"
    } else if cfg!(target_os = "android") {
        "Replace image"
    } else {
        "Replace image (or paste with Ctrl+V)"
    };
    let (resp, attach_clicked, send_clicked) = compose_input_row(
        ui,
        th,
        &mut state.compose,
        "Caption (optional)…",
        "Submit",
        80.0,
        attach_tip,
        !uploading,
    );
    if attach_clicked && !uploading && !pick_busy {
        state.start_file_pick();
    }
    if state.focus_compose && !uploading {
        resp.request_focus();
        state.focus_compose = false;
    }
    let enter = resp.has_focus()
        && !uploading
        && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if (send_clicked || enter) && can_send {
        submit = true;
    }

    if clear {
        state.compose_image = None;
    }

    if submit {
        Some(state.compose.trim().to_string())
    } else {
        None
    }
}

/// Fixed-height compose row: attach | field | action, vertically centered.
///
/// Returns `(text_response, attach_clicked, action_clicked)`.
///
/// Layout notes:
/// - Width is the *current* available width (already inside frame margins).
/// - Theme `item_spacing` is zeroed so gaps are only the explicit `sp.sm`
///   (otherwise spacing stacks and clips the action button on the right).
/// - All three controls share `control_height` so baselines match.
fn compose_input_row(
    ui: &mut egui::Ui,
    th: &Theme,
    text: &mut String,
    hint: &str,
    action_label: &str,
    action_w: f32,
    attach_tooltip: &str,
    field_interactive: bool,
) -> (egui::Response, bool, bool) {
    let p = &th.palette;
    let sp = &th.spacing;
    let control_h = sp.control_height;
    let gap = sp.sm;
    // Square attach control matching the row height.
    let attach_w = control_h;
    let row_w = ui.available_width().max(1.0);
    let field_w = (row_w - attach_w - action_w - gap * 2.0).max(48.0);

    let mut attach_clicked = false;
    let mut action_clicked = false;
    let mut text_resp = None;

    ui.allocate_ui_with_layout(
        Vec2::new(row_w, control_h),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(row_w);
            ui.set_height(control_h);
            // Explicit gaps only — theme item_spacing would double-count.
            ui.spacing_mut().item_spacing.x = 0.0;

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
                .on_hover_text(attach_tooltip);
            attach_clicked = attach.clicked();

            ui.add_space(gap);

            // Fixed single-line height; Shift+Enter inserts a newline, bare Enter sends.
            // Center text so the hint matches attach/action baselines
            // (multiline defaults to TOP; button labels are centered).
            let te = egui::TextEdit::multiline(text)
                .margin(th.text_edit_margin())
                .desired_width(field_w)
                .desired_rows(1)
                .min_size(Vec2::new(field_w, control_h))
                .vertical_align(Align::Center)
                .hint_text(hint)
                .interactive(field_interactive)
                .return_key(egui::KeyboardShortcut::new(
                    egui::Modifiers::SHIFT,
                    egui::Key::Enter,
                ));
            text_resp = Some(ui.add_sized(Vec2::new(field_w, control_h), te));

            ui.add_space(gap);

            action_clicked = ui
                .add_sized(
                    Vec2::new(action_w, control_h),
                    egui::Button::new(
                        RichText::new(action_label)
                            .size(th.type_scale.body)
                            .color(p.accent_fg),
                    )
                    .fill(p.accent)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(sp.radius_md)
                    .min_size(Vec2::new(action_w, control_h)),
                )
                .clicked();
        },
    );

    (
        text_resp.expect("compose row always allocates the text field"),
        attach_clicked,
        action_clicked,
    )
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
    let call_title = channel_call
        .as_ref()
        .and_then(|c| c.title.as_ref())
        .map(|t| t.as_str())
        .filter(|t| !t.is_empty());
    // Prefer a real call title; otherwise the channel (often long for stream.place).
    let headline = call_title.unwrap_or(lc.channel.as_str());
    let on_call_channel = matches!(
        &state.route,
        crate::state::Route::Chat(ch) if ch.eq_ignore_ascii_case(&lc.channel)
    );

    // One status line: count + media. Avoids "1 in call" next to a long title
    // (overlap) and a second "In call" row under it (redundant).
    let status_line = match &lc.media {
        crate::av::MediaStatus::Live => format!("{n} in call"),
        crate::av::MediaStatus::Idle => format!("{n} in call"),
        crate::av::MediaStatus::Connecting => format!("Connecting… · {n}"),
        crate::av::MediaStatus::Failed(e) => format!("Media failed · {e}"),
        crate::av::MediaStatus::BrowserOnly => "Open in browser for media".to_string(),
    };

    let frame = egui::Frame::new()
        .fill(p.accent.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0_f32, p.accent.gamma_multiply(0.45)))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8));

    frame.show(ui, |ui| {
        // Full panel width — long stream.place names stack (title → status)
        // instead of fighting for horizontal space with the count.
        let panel_w = ui.available_width();
        ui.set_width(panel_w);
        ui.spacing_mut().item_spacing.y = 2.0;

        let title_h = th.type_scale.body * 1.35;
        ui.allocate_ui_with_layout(
            Vec2::new(panel_w, title_h),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(panel_w);
                ui.set_max_width(panel_w);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("📞 {headline}"))
                            .size(th.type_scale.body)
                            .color(p.text)
                            .strong(),
                    )
                    .truncate()
                    .sense(Sense::hover()),
                )
                .on_hover_text(format!("{headline}\n{}", lc.channel));
            },
        );

        ui.add(
            egui::Label::new(
                RichText::new(&status_line)
                    .size(th.type_scale.caption)
                    .color(p.text_secondary),
            )
            .truncate(),
        );

        if matches!(lc.media, crate::av::MediaStatus::Live) {
            paint_av_video_tiles(ui, th, state);
        }

        ui.add_space(sp.xs);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = sp.sm;
            if av_icon_toggle(ui, th, "🎤", lc.muted, if lc.muted { "Unmute" } else { "Mute" })
                .clicked()
            {
                action = Some(ChatAction::AvToggleMute);
            }
            if lc.has_camera {
                if av_icon_toggle(
                    ui,
                    th,
                    "📷",
                    !lc.camera,
                    if lc.camera {
                        "Turn camera off"
                    } else {
                        "Turn camera on"
                    },
                )
                .clicked()
                {
                    action = Some(ChatAction::AvToggleCamera);
                }
            }
            if button(ui, th, "Leave").clicked() {
                action = Some(ChatAction::AvLeave);
            }
            if !on_call_channel {
                if button(ui, th, "Open chat").clicked() {
                    action = Some(ChatAction::OpenCallChannel(lc.channel.clone()));
                }
            }
        });
    });

    action
}

/// Square mic/camera toggle: icon only, diagonal slash when off/muted.
fn av_icon_toggle(
    ui: &mut egui::Ui,
    th: &Theme,
    icon: &str,
    slashed: bool,
    tooltip: &str,
) -> egui::Response {
    let p = &th.palette;
    let sp = &th.spacing;
    let size = sp.control_height;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());

    let fill = if slashed {
        p.button_bg
    } else {
        p.accent.gamma_multiply(0.22)
    };
    let border = if slashed {
        p.border_soft
    } else {
        p.accent.gamma_multiply(0.55)
    };

    let painter = ui.painter();
    painter.rect(
        rect,
        sp.radius_md,
        fill,
        egui::Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional((th.type_scale.body * 1.15).max(16.0)),
        p.button_fg,
    );
    if slashed {
        let pad = size * 0.22;
        painter.line_segment(
            [
                rect.left_top() + Vec2::new(pad, pad),
                rect.right_bottom() - Vec2::new(pad, pad),
            ],
            egui::Stroke::new(2.0_f32, p.destructive),
        );
    }

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.on_hover_text(tooltip)
}

/// Pre-call mic/camera toggles (icons match in-call controls).
fn av_media_prefs_row(
    ui: &mut egui::Ui,
    th: &Theme,
    pref_muted: &mut bool,
    pref_camera: &mut bool,
) {
    let sp = &th.spacing;
    ui.horizontal(|ui| {
        if av_icon_toggle(
            ui,
            th,
            "🎤",
            *pref_muted,
            if *pref_muted { "Unmute" } else { "Mute" },
        )
        .clicked()
        {
            *pref_muted = !*pref_muted;
        }
        ui.add_space(sp.sm);
        if av_icon_toggle(
            ui,
            th,
            "📷",
            !*pref_camera,
            if *pref_camera {
                "Turn camera off"
            } else {
                "Turn camera on"
            },
        )
        .clicked()
        {
            *pref_camera = !*pref_camera;
        }
    });
}

/// Per-channel call strip when we are not in any local call:
/// start (idle) or join (session present on this channel).
/// Active-call chrome is global (`active_call_panel`); do not duplicate it here.
fn av_call_banner(
    ui: &mut egui::Ui,
    th: &Theme,
    channel: &str,
    channel_call: Option<&crate::av::ChannelCall>,
    pref_muted: &mut bool,
    pref_camera: &mut bool,
) -> Option<ChatAction> {
    let sp = &th.spacing;
    let p = &th.palette;
    let mut action = None;

    // Idle: offer start. One call at a time is enforced when starting.
    if channel_call.is_none() {
        ui.horizontal(|ui| {
            if button(ui, th, "📞 Call").clicked() {
                action = Some(ChatAction::AvStart(channel.to_string()));
            }
            dim_label(ui, th, "Voice & video over MoQ");
        });
        ui.add_space(sp.xs);
        av_media_prefs_row(ui, th, pref_muted, pref_camera);
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
        // Stack title + count (same pattern as active_call_panel) so long
        // stream.place names don't paint over "N in call".
        let panel_w = ui.available_width();
        ui.set_width(panel_w);
        ui.spacing_mut().item_spacing.y = 2.0;

        let title_h = th.type_scale.body * 1.35;
        ui.allocate_ui_with_layout(
            Vec2::new(panel_w, title_h),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(panel_w);
                ui.set_max_width(panel_w);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("📞 {title}"))
                            .size(th.type_scale.body)
                            .color(p.text)
                            .strong(),
                    )
                    .truncate(),
                );
            },
        );
        dim_label(ui, th, &format!("{n} in call"));

        ui.add_space(sp.sm);
        ui.horizontal(|ui| {
            if primary_button(ui, th, "Join call").clicked() {
                action = Some(ChatAction::AvJoin {
                    channel: channel.to_string(),
                    session_id: call.session_id.clone(),
                });
            }
        });
        ui.add_space(sp.xs);
        av_media_prefs_row(ui, th, pref_muted, pref_camera);
    });

    action
}

/// Paint remote (+ local preview) video tiles from the MoQ frame store.
/// Click a tile to enlarge/focus it; click the focused tile again to restore the grid.
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
    ui.add_space(sp.sm);

    let live: std::collections::HashSet<String> = frames.iter().map(|(n, _)| n.clone()).collect();
    state.av_video_textures.retain(|k, _| live.contains(k));
    // Drop focus if that participant left / stopped publishing.
    if state
        .av_focused_video
        .as_ref()
        .is_some_and(|n| !live.contains(n))
    {
        state.av_focused_video = None;
    }

    // Upload / refresh GPU textures first so paint helpers only need ids + dims.
    let mut tiles: Vec<(String, egui::TextureId, u32, u32)> = Vec::with_capacity(frames.len());
    for (nick, frame) in &frames {
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            frame.rgba.as_ref(),
        );
        let tex_id = match state.av_video_textures.entry(nick.clone()) {
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
        };
        tiles.push((nick.clone(), tex_id, frame.width, frame.height));
    }

    let focused = state.av_focused_video.clone();
    let mut clicked: Option<String> = None;

    if let Some(focus_nick) = focused.as_ref() {
        // Enlarged primary + filmstrip of the rest.
        let avail = ui.available_width();
        let primary_w = avail;
        let primary_h = (primary_w * 9.0 / 16.0).clamp(140.0, 420.0);
        let thumb_w = ((avail - sp.sm) / 4.0).clamp(72.0, 140.0);
        let thumb_h = thumb_w * 9.0 / 16.0;

        if let Some((nick, tex_id, w, h)) = tiles.iter().find(|(n, _, _, _)| n == focus_nick) {
            if paint_av_video_tile(
                ui,
                th,
                nick,
                *tex_id,
                *w,
                *h,
                Vec2::new(primary_w, primary_h),
                true,
            )
            .clicked()
            {
                clicked = Some(nick.clone());
            }
        }

        let others: Vec<_> = tiles
            .iter()
            .filter(|(n, _, _, _)| n != focus_nick)
            .cloned()
            .collect();
        if !others.is_empty() {
            ui.add_space(sp.xs);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(sp.sm, sp.sm);
                for (nick, tex_id, w, h) in others {
                    if paint_av_video_tile(
                        ui,
                        th,
                        &nick,
                        tex_id,
                        w,
                        h,
                        Vec2::new(thumb_w, thumb_h),
                        false,
                    )
                    .clicked()
                    {
                        clicked = Some(nick);
                    }
                }
            });
        }
    } else {
        // Equal grid — use full panel width so a single stream isn't a tiny 280px tile
        // with a huge empty gutter (looked like the AV section was truncated).
        let avail = ui.available_width();
        let n = tiles.len().max(1);
        let tile_w = if n == 1 {
            avail
        } else if n == 2 {
            ((avail - sp.sm) / 2.0).max(120.0)
        } else {
            ((avail - sp.sm) / 2.0).clamp(100.0, 280.0)
        };
        let tile_h = (tile_w * 9.0 / 16.0).clamp(90.0, 420.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(sp.sm, sp.sm);
            for (nick, tex_id, w, h) in &tiles {
                if paint_av_video_tile(
                    ui,
                    th,
                    nick,
                    *tex_id,
                    *w,
                    *h,
                    Vec2::new(tile_w, tile_h),
                    false,
                )
                .clicked()
                {
                    clicked = Some(nick.clone());
                }
            }
        });
    }

    if let Some(nick) = clicked {
        // Toggle: click focused tile to restore grid; otherwise switch focus.
        if state.av_focused_video.as_deref() == Some(nick.as_str()) {
            state.av_focused_video = None;
        } else {
            state.av_focused_video = Some(nick);
        }
    }
}

/// One clickable video tile. Returns the interaction response (for click handling).
fn paint_av_video_tile(
    ui: &mut egui::Ui,
    th: &Theme,
    nick: &str,
    tex_id: egui::TextureId,
    frame_w: u32,
    frame_h: u32,
    size: Vec2,
    focused: bool,
) -> egui::Response {
    let sp = &th.spacing;
    let p = &th.palette;
    let label = if nick == "__local__" {
        "You"
    } else {
        nick
    };

    ui.vertical(|ui| {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.hovered() || focused {
            // Subtle ring when hovered or currently focused.
            let stroke_c = if focused {
                p.accent
            } else {
                p.accent.gamma_multiply(0.55)
            };
            ui.painter().rect_stroke(
                rect,
                sp.radius_sm,
                egui::Stroke::new(if focused { 2.0_f32 } else { 1.5_f32 }, stroke_c),
                egui::StrokeKind::Outside,
            );
        }
        let aspect = frame_w as f32 / frame_h.max(1) as f32;
        let fit = if aspect > size.x / size.y {
            Vec2::new(size.x, size.x / aspect)
        } else {
            Vec2::new(size.y * aspect, size.y)
        };
        let img_rect = egui::Rect::from_center_size(rect.center(), fit);
        ui.painter().rect_filled(rect, sp.radius_sm, p.card_bg);
        ui.painter().image(
            tex_id,
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        // Nick chip — truncate long nicks so they stay inside the tile.
        let nick_font = egui::FontId::proportional(th.type_scale.caption);
        let max_nick_w = (rect.width() - 12.0).max(24.0);
        let galley = ui.fonts(|f| {
            f.layout_no_wrap(label.to_string(), nick_font.clone(), p.text)
        });
        let nick_text = if galley.size().x > max_nick_w {
            // Binary-search a prefix that fits with an ellipsis.
            let chars: Vec<char> = label.chars().collect();
            let mut lo = 0usize;
            let mut hi = chars.len();
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                let candidate: String = chars[..mid].iter().collect::<String>() + "…";
                let g = ui.fonts(|f| f.layout_no_wrap(candidate, nick_font.clone(), p.text));
                if g.size().x <= max_nick_w {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            if lo == 0 {
                "…".to_string()
            } else {
                chars[..lo].iter().collect::<String>() + "…"
            }
        } else {
            label.to_string()
        };
        ui.painter().text(
            rect.left_bottom() + Vec2::new(6.0, -4.0),
            Align2::LEFT_BOTTOM,
            nick_text,
            nick_font,
            p.text,
        );
        resp
    })
    .inner
}
