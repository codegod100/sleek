//! Chat detail — messages + compose (freeq-android ChatDetailScreen inspired).

use eframe::egui::{self, text::CCursor, text::CCursorRange, Align, Align2, CursorIcon, Layout, RichText, Sense, ScrollArea, Vec2};
use vidya::{button, dim_label, primary_button, Theme};

use crate::clipboard;
use crate::state::{AppState, NickTabComplete};
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
    /// Select camera device (`None` = system first / default).
    AvSelectCamera(Option<String>),
    /// Select microphone by name (`None` = system default).
    AvSelectMic(Option<String>),
    /// Select speaker by name (`None` = system default).
    AvSelectSpeaker(Option<String>),
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
        let prev_cam_id = state.av_pref_camera_id.clone();
        let prev_mic = state.av_pref_mic_id.clone();
        let prev_spk = state.av_pref_speaker_id.clone();
        // Refresh device lists when the pre-call banner is shown (cheap).
        if state.av_device_cameras.is_empty()
            && state.av_device_mics.is_empty()
            && state.av_device_speakers.is_empty()
        {
            state.refresh_av_devices();
        }
        if let Some(act) = av_call_banner(ui, th, channel, channel_call.as_ref(), state) {
            action = act;
        }
        if state.av_pref_muted != prev_muted
            || state.av_pref_camera != prev_camera
            || state.av_pref_camera_id != prev_cam_id
            || state.av_pref_mic_id != prev_mic
            || state.av_pref_speaker_id != prev_spk
        {
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

            let compose_id = egui::Id::new(("chat_compose", channel));
            // Build nick candidates before mutably borrowing compose for Tab complete.
            let nick_candidates =
                nick_completion_candidates(&members, &messages, is_channel, channel, state);
            // Consume Tab before TextEdit so it does not insert `\t` / steal focus.
            try_nick_tab_complete(
                ui,
                compose_id,
                &mut state.compose,
                &mut state.compose_nick_tab,
                &nick_candidates,
            );

            if has_image {
                if let Some(send) = compose_image_composer(ui, th, state, compose_id) {
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
                    compose_id,
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
    compose_id: egui::Id,
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
        compose_id,
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
/// - `lock_focus(true)` keeps Tab in the field for nick completion (handled
///   by [`try_nick_tab_complete`] before this runs).
fn compose_input_row(
    ui: &mut egui::Ui,
    th: &Theme,
    text: &mut String,
    hint: &str,
    action_label: &str,
    action_w: f32,
    attach_tooltip: &str,
    field_interactive: bool,
    field_id: egui::Id,
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
            // lock_focus: Tab stays here for nick complete (not focus-next).
            let te = egui::TextEdit::multiline(text)
                .id(field_id)
                .lock_focus(true)
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

// ── Tab nick completion ──────────────────────────────────────────────────────

/// Characters that form an IRC-ish nick (and freeq display names).
fn is_nick_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '[' | ']' | '{' | '}' | '\\' | '|' | '^' | '-' | '_' | '`' | '.' | '/'
        )
}

/// Word ending at `cursor` (char index): `(start, text)` including a leading `@` if present.
fn word_before_cursor(text: &str, cursor: usize) -> (usize, String) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && is_nick_char(chars[start - 1]) {
        start -= 1;
    }
    if start > 0 && chars[start - 1] == '@' {
        start -= 1;
    }
    let word: String = chars[start..cursor].iter().collect();
    (start, word)
}

fn completion_text(nick: &str, leading: bool) -> String {
    if leading {
        format!("{nick}: ")
    } else {
        format!("{nick} ")
    }
}

/// Nicks available for Tab complete: room members, message authors, DM peer.
fn nick_completion_candidates(
    members: &[String],
    messages: &[crate::state::ChatMessage],
    is_channel: bool,
    channel: &str,
    state: &AppState,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |n: &str| {
        let n = n.trim();
        if n.is_empty() {
            return;
        }
        if out.iter().any(|e| e.eq_ignore_ascii_case(n)) {
            return;
        }
        out.push(n.to_string());
    };

    for m in members {
        push(m);
    }
    for msg in messages {
        if !msg.is_system && !msg.from.is_empty() {
            push(&msg.from);
        }
    }
    if !is_channel {
        push(&state.display_name_for(channel));
        push(channel);
    }
    out
}

fn expected_completion(tab: &NickTabComplete) -> String {
    let mut body = completion_text(&tab.matches[tab.index], tab.leading);
    if tab.had_at {
        body.insert(0, '@');
    }
    body
}

/// Tab / Shift+Tab: complete the nick under the cursor against `candidates`.
///
/// Must run **before** the compose [`egui::TextEdit`] so Tab is not inserted as
/// a tab character. Call with a stable `field_id` shared with that TextEdit.
fn try_nick_tab_complete(
    ui: &mut egui::Ui,
    field_id: egui::Id,
    compose: &mut String,
    tab: &mut NickTabComplete,
    candidates: &[String],
) {
    let focused = ui.memory(|m| m.has_focus(field_id));
    if !focused {
        return;
    }

    // Shift+Tab cycles backward; plain Tab forward. Match Shift first.
    let backward = ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
    let forward =
        !backward && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    if !forward && !backward {
        return;
    }

    let cursor = egui::TextEdit::load_state(ui.ctx(), field_id)
        .and_then(|s| s.cursor.char_range())
        .map(|r| r.primary.index)
        .unwrap_or_else(|| compose.chars().count());

    let cycling = tab.active
        && !tab.matches.is_empty()
        && cursor == tab.cursor_end
        && {
            let expected = expected_completion(tab);
            let have: String = compose
                .chars()
                .skip(tab.word_start)
                .take(cursor.saturating_sub(tab.word_start))
                .collect();
            have == expected
        };

    if cycling {
        let n = tab.matches.len();
        tab.index = if backward {
            (tab.index + n - 1) % n
        } else {
            (tab.index + 1) % n
        };
    } else {
        let (word_start, word) = word_before_cursor(compose, cursor);
        let had_at = word.starts_with('@');
        let prefix = if had_at {
            word[1..].to_string()
        } else {
            word
        };
        let prefix_lc = prefix.to_lowercase();
        let leading = compose.chars().take(word_start).all(|c| c.is_whitespace());

        let mut matches: Vec<String> = candidates
            .iter()
            .filter(|n| n.to_lowercase().starts_with(&prefix_lc))
            .cloned()
            .collect();
        // Prefer exact case-prefix matches, then alphabetical (case-insensitive).
        matches.sort_by(|a, b| {
            let a_exact = a.starts_with(&prefix);
            let b_exact = b.starts_with(&prefix);
            b_exact
                .cmp(&a_exact)
                .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
                .then_with(|| a.cmp(b))
        });
        matches.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        if matches.is_empty() {
            tab.clear();
            return;
        }

        // If the typed word already is a full match, advance to the next one.
        let mut index = 0;
        if !prefix.is_empty() {
            if let Some(i) = matches
                .iter()
                .position(|m| m.eq_ignore_ascii_case(&prefix))
            {
                index = if backward {
                    (i + matches.len() - 1) % matches.len()
                } else {
                    (i + 1) % matches.len()
                };
            }
        }

        *tab = NickTabComplete {
            active: true,
            word_start,
            matches,
            index,
            leading,
            had_at,
            cursor_end: cursor,
        };
    }

    let body = expected_completion(tab);
    let mut chars: Vec<char> = compose.chars().collect();
    let end = cursor.min(chars.len());
    let start = tab.word_start.min(end);
    chars.splice(start..end, body.chars());
    *compose = chars.into_iter().collect();

    let new_cursor = start + body.chars().count();
    tab.cursor_end = new_cursor;
    tab.word_start = start;
    tab.active = true;

    if let Some(mut te_state) = egui::TextEdit::load_state(ui.ctx(), field_id) {
        te_state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(new_cursor))));
        egui::TextEdit::store_state(ui.ctx(), field_id, te_state);
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

        // Paint tiles as soon as we are Connecting or Live so self-view can
        // appear the moment capture starts (not only after MoQ Live toast).
        if matches!(
            lc.media,
            crate::av::MediaStatus::Live | crate::av::MediaStatus::Connecting
        ) {
            paint_av_video_tiles(ui, th, state);
        }

        ui.add_space(sp.xs);
        // Primary chrome only: mute / camera / leave. Device pickers live under
        // a Devices toggle so long Cam/Mic/Out combos don't wrap over chat.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = sp.sm;
            if av_icon_toggle(
                ui,
                th,
                "🎤",
                lc.muted || !lc.has_mic,
                if !lc.has_mic {
                    "No microphone (listen-only)"
                } else if lc.muted {
                    "Unmute"
                } else {
                    "Mute"
                },
            )
            .clicked()
            {
                // Listen-only: toggling mute is a no-op (nothing to send).
                if lc.has_mic {
                    action = Some(ChatAction::AvToggleMute);
                }
            }
            // Live mic volume meter (updates while media is Live).
            if matches!(lc.media, crate::av::MediaStatus::Live) {
                let level = state
                    .av_mic_level
                    .as_ref()
                    .map(|m| m.get())
                    .unwrap_or(0.0);
                paint_mic_level_meter(ui, th, level, lc.muted || !lc.has_mic);
                // Capture thread writes levels continuously — keep the bar moving.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(33));
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
            av_devices_toggle_button(ui, th, state);
        });

        if state.av_show_devices {
            if state.av_device_cameras.is_empty()
                && state.av_device_mics.is_empty()
                && state.av_device_speakers.is_empty()
            {
                state.refresh_av_devices();
            }
            ui.add_space(sp.xs);
            if let Some(act) = av_device_selectors(ui, th, state, /*in_call=*/ true) {
                action = Some(act);
            }
        }
    });

    action
}

/// Compact Devices ▾ / ▴ control (no extra row of pickers).
fn av_devices_toggle_button(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) {
    let devices_label = if state.av_show_devices {
        "Devices ▴"
    } else {
        "Devices ▾"
    };
    if button(ui, th, devices_label).clicked() {
        state.av_show_devices = !state.av_show_devices;
        if state.av_show_devices {
            state.refresh_av_devices();
        }
    }
}

/// Horizontal mic volume bar next to the mute control (0.0..=1.0).
///
/// Dimmed when muted; green→yellow→red segments at higher levels so peaks
/// are obvious. Level is measured pre-mute so the bar still moves while muted.
fn paint_mic_level_meter(ui: &mut egui::Ui, th: &Theme, level: f32, muted: bool) {
    let p = &th.palette;
    let sp = &th.spacing;
    let level = level.clamp(0.0, 1.0);
    let h = (sp.control_height * 0.42).max(8.0);
    let w = (sp.control_height * 2.4).max(48.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());

    let painter = ui.painter();
    let rounding = sp.radius_sm.min(h * 0.5);
    let track = if muted {
        p.button_bg.gamma_multiply(0.85)
    } else {
        p.button_bg
    };
    painter.rect(
        rect,
        rounding,
        track,
        egui::Stroke::new(1.0_f32, p.border_soft),
        egui::StrokeKind::Inside,
    );

    if level > 0.01 {
        let fill_w = (rect.width() * level).max(2.0);
        let fill_rect = egui::Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
        // Quiet = accent; mid = warm; hot peaks = destructive.
        let fill = if muted {
            p.text_secondary.gamma_multiply(0.55)
        } else if level < 0.55 {
            p.accent
        } else if level < 0.82 {
            egui::Color32::from_rgb(230, 180, 60)
        } else {
            p.destructive
        };
        painter.rect(
            fill_rect,
            rounding,
            fill,
            egui::Stroke::NONE,
            egui::StrokeKind::Inside,
        );
    }

    let pct = (level * 100.0).round() as i32;
    let tip = if muted {
        format!("Mic level {pct}% (muted — still showing input)")
    } else {
        format!("Mic level {pct}%")
    };
    response.on_hover_text(tip);
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
fn av_media_prefs_row(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) {
    let sp = &th.spacing;
    ui.horizontal(|ui| {
        if av_icon_toggle(
            ui,
            th,
            "🎤",
            state.av_pref_muted,
            if state.av_pref_muted {
                "Unmute"
            } else {
                "Mute"
            },
        )
        .clicked()
        {
            state.av_pref_muted = !state.av_pref_muted;
        }
        ui.add_space(sp.sm);
        if av_icon_toggle(
            ui,
            th,
            "📷",
            !state.av_pref_camera,
            if state.av_pref_camera {
                "Turn camera off"
            } else {
                "Turn camera on"
            },
        )
        .clicked()
        {
            state.av_pref_camera = !state.av_pref_camera;
        }
    });
}

/// Camera / mic / speaker combo boxes. Emits `AvSelect*` when the choice changes
/// mid-call so the media plane can switch immediately; pre-call only updates prefs.
///
/// Stacked full-width rows (not horizontal wrap) so long device names don't
/// pile Cam + Mic on one line and shove Out under chat text.
fn av_device_selectors(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    in_call: bool,
) -> Option<ChatAction> {
    let sp = &th.spacing;
    let mut action = None;

    // Hide entirely when nothing was discovered (Android / headless).
    if state.av_device_cameras.is_empty()
        && state.av_device_mics.is_empty()
        && state.av_device_speakers.is_empty()
    {
        return None;
    }

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = sp.xs;
        ui.set_width(ui.available_width());

        // ── Camera ──
        if !state.av_device_cameras.is_empty() {
            let current = state
                .av_pref_camera_id
                .as_ref()
                .and_then(|id| {
                    state
                        .av_device_cameras
                        .iter()
                        .find(|d| &d.id == id)
                        .map(|d| d.name.clone())
                })
                .unwrap_or_else(|| "Default".to_string());
            let options: Vec<(String, String, bool)> = state
                .av_device_cameras
                .iter()
                .map(|d| (d.id.clone(), d.name.clone(), d.is_default))
                .collect();
            let selected_id = state.av_pref_camera_id.clone();
            if let Some(id) = av_device_combo_row_selected(
                ui,
                th,
                "Cam",
                "av_sel_camera",
                &current,
                "Default",
                selected_id.is_none(),
                &options,
                selected_id.as_deref(),
            ) {
                if in_call {
                    action = Some(ChatAction::AvSelectCamera(id));
                } else {
                    state.av_pref_camera_id = id;
                }
            }
        }

        // ── Mic ──
        if !state.av_device_mics.is_empty() {
            let current = state
                .av_pref_mic_id
                .as_ref()
                .and_then(|id| {
                    state
                        .av_device_mics
                        .iter()
                        .find(|d| &d.id == id)
                        .map(|d| d.name.clone())
                })
                .or_else(|| {
                    state
                        .av_device_mics
                        .iter()
                        .find(|d| d.is_default)
                        .map(|d| d.name.clone())
                })
                .unwrap_or_else(|| "Default".to_string());
            let options: Vec<(String, String, bool)> = state
                .av_device_mics
                .iter()
                .map(|d| (d.id.clone(), d.name.clone(), d.is_default))
                .collect();
            let selected_id = state.av_pref_mic_id.clone();
            if let Some(id) = av_device_combo_row_selected(
                ui,
                th,
                "Mic",
                "av_sel_mic",
                &current,
                "System default",
                selected_id.is_none(),
                &options,
                selected_id.as_deref(),
            ) {
                if in_call {
                    action = Some(ChatAction::AvSelectMic(id));
                } else {
                    state.av_pref_mic_id = id;
                }
            }
        }

        // ── Speaker ──
        if !state.av_device_speakers.is_empty() {
            let current = state
                .av_pref_speaker_id
                .as_ref()
                .and_then(|id| {
                    state
                        .av_device_speakers
                        .iter()
                        .find(|d| &d.id == id)
                        .map(|d| d.name.clone())
                })
                .or_else(|| {
                    state
                        .av_device_speakers
                        .iter()
                        .find(|d| d.is_default)
                        .map(|d| d.name.clone())
                })
                .unwrap_or_else(|| "Default".to_string());
            let options: Vec<(String, String, bool)> = state
                .av_device_speakers
                .iter()
                .map(|d| (d.id.clone(), d.name.clone(), d.is_default))
                .collect();
            let selected_id = state.av_pref_speaker_id.clone();
            if let Some(id) = av_device_combo_row_selected(
                ui,
                th,
                "Out",
                "av_sel_speaker",
                &current,
                "System default",
                selected_id.is_none(),
                &options,
                selected_id.as_deref(),
            ) {
                if in_call {
                    action = Some(ChatAction::AvSelectSpeaker(id));
                } else {
                    state.av_pref_speaker_id = id;
                }
            }
        }
    });

    action
}

/// Device picker row with correct id-based selection highlighting.
fn av_device_combo_row_selected(
    ui: &mut egui::Ui,
    th: &Theme,
    label: &str,
    id_salt: &str,
    current: &str,
    none_label: &str,
    none_selected: bool,
    options: &[(String, String, bool)],
    selected_id: Option<&str>,
) -> Option<Option<String>> {
    let p = &th.palette;
    let sp = &th.spacing;
    let label_w = 36.0_f32;
    let mut picked: Option<Option<String>> = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = sp.sm;
        let row_h = sp.control_height.max(th.type_scale.caption * 1.6);
        ui.allocate_ui_with_layout(
            Vec2::new(label_w, row_h),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(
                    RichText::new(label)
                        .size(th.type_scale.caption)
                        .color(p.text_secondary),
                );
            },
        );
        let combo_w = (ui.available_width() - sp.sm).clamp(120.0, 420.0);
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(current)
            .width(combo_w)
            .show_ui(ui, |ui| {
                if ui.selectable_label(none_selected, none_label).clicked() {
                    picked = Some(None);
                }
                for (id, name, is_default) in options {
                    let sel = selected_id == Some(id.as_str());
                    let row_label = if *is_default {
                        format!("{name} (default)")
                    } else {
                        name.clone()
                    };
                    if ui.selectable_label(sel, row_label).clicked() {
                        picked = Some(Some(id.clone()));
                    }
                }
            });
    });

    picked
}

/// Per-channel call strip when we are not in any local call:
/// start (idle) or join (session present on this channel).
/// Active-call chrome is global (`active_call_panel`); do not duplicate it here.
fn av_call_banner(
    ui: &mut egui::Ui,
    th: &Theme,
    channel: &str,
    channel_call: Option<&crate::av::ChannelCall>,
    state: &mut AppState,
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
        av_media_prefs_row(ui, th, state);
        // Device pickers stay behind Devices so Cam/Mic/Out don't dominate idle chat.
        ui.add_space(sp.xs);
        av_devices_disclosure(ui, th, state, /*in_call=*/ false, &mut action);
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
        av_media_prefs_row(ui, th, state);
        ui.add_space(sp.xs);
        av_devices_disclosure(ui, th, state, /*in_call=*/ false, &mut action);
    });

    action
}

/// Devices ▾ toggle + stacked Cam/Mic/Out rows when open.
fn av_devices_disclosure(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    in_call: bool,
    action: &mut Option<ChatAction>,
) {
    av_devices_toggle_button(ui, th, state);
    if !state.av_show_devices {
        return;
    }
    if state.av_device_cameras.is_empty()
        && state.av_device_mics.is_empty()
        && state.av_device_speakers.is_empty()
    {
        state.refresh_av_devices();
    }
    ui.add_space(th.spacing.xs);
    if let Some(act) = av_device_selectors(ui, th, state, in_call) {
        *action = Some(act);
    }
}

/// Column count for an equal video grid (classic call layouts).
fn av_grid_cols(n: usize) -> usize {
    match n {
        0 | 1 => 1,
        2 => 2,
        3..=4 => 2,
        5..=9 => 3,
        _ => 4,
    }
}

/// Paint remote (+ local preview) video tiles from the MoQ frame store.
///
/// Streams are laid out in a fixed multi-column grid (or focus + filmstrip)
/// so frames never share the same rect. Click a tile to enlarge/focus it;
/// click the focused tile again to restore the grid.
fn paint_av_video_tiles(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) {
    use std::sync::Arc;

    let store = state.av_video.clone();
    let mut frames = store
        .as_ref()
        .map(|s| s.snapshot())
        .unwrap_or_default();
    // Local camera open + on, but no frame yet — keep a "You" slot while
    // capture warms up. Only when has_camera is true (device actually opened);
    // otherwise a dark blank square looks like broken video.
    let want_local = state
        .local_call
        .as_ref()
        .is_some_and(|lc| lc.has_camera && lc.camera);
    let has_local = frames.iter().any(|(k, _)| k == "__local__");
    if want_local && !has_local {
        // Distinct warm-up pattern (not solid black) so users can tell
        // "waiting for first frame" from a dead tile.
        let mut px = Vec::with_capacity(8 * 8 * 4);
        for y in 0..8 {
            for x in 0..8 {
                let on = (x + y) % 2 == 0;
                let v = if on { 48u8 } else { 28u8 };
                px.extend_from_slice(&[v, v, v.saturating_add(12), 255]);
            }
        }
        frames.push((
            "__local__".into(),
            crate::av::RgbaVideoFrame {
                width: 8,
                height: 8,
                rgba: Arc::<[u8]>::from(px),
            },
        ));
    }
    if frames.is_empty() {
        return;
    }
    // Local preview first, then remotes alphabetically (key = nick or nick~instance).
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
    for (key, frame) in &frames {
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            frame.rgba.as_ref(),
        );
        let tex_id = match state.av_video_textures.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().set(color, egui::TextureOptions::LINEAR);
                e.get().id()
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                let tex = ui.ctx().load_texture(
                    format!("av_video_{key}"),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                let id = tex.id();
                e.insert(tex);
                id
            }
        };
        tiles.push((key.clone(), tex_id, frame.width, frame.height));
    }

    let focused = state.av_focused_video.clone();
    let mut clicked: Option<String> = None;

    if let Some(focus_key) = focused.as_ref() {
        // Enlarged primary + filmstrip of the rest.
        let avail = ui.available_width().max(1.0);
        let primary_w = avail;
        let primary_h = (primary_w * 9.0 / 16.0).clamp(140.0, 360.0);

        if let Some((key, tex_id, w, h)) = tiles.iter().find(|(n, _, _, _)| n == focus_key) {
            if paint_av_video_tile(
                ui,
                th,
                key,
                *tex_id,
                *w,
                *h,
                Vec2::new(primary_w, primary_h),
                true,
            )
            .clicked()
            {
                clicked = Some(key.clone());
            }
        }

        let others: Vec<_> = tiles
            .iter()
            .filter(|(n, _, _, _)| n != focus_key)
            .cloned()
            .collect();
        if !others.is_empty() {
            ui.add_space(sp.xs);
            let n_thumbs = others.len();
            let thumb_cols = n_thumbs.min(4).max(1);
            let gaps = sp.sm * (thumb_cols.saturating_sub(1) as f32);
            let thumb_w = ((avail - gaps) / thumb_cols as f32).clamp(64.0, 160.0);
            let thumb_h = thumb_w * 9.0 / 16.0;
            paint_av_video_grid(
                ui,
                th,
                &others,
                thumb_cols,
                Vec2::new(thumb_w, thumb_h),
                &mut clicked,
            );
        }
    } else {
        // Equal grid: columns from stream count, cell size from full panel width
        // so tiles sit side-by-side instead of stacking/overlapping.
        let avail = ui.available_width().max(1.0);
        let n = tiles.len().max(1);
        let cols = av_grid_cols(n);
        let gaps = sp.sm * (cols.saturating_sub(1) as f32);
        let tile_w = if n == 1 {
            avail
        } else {
            ((avail - gaps) / cols as f32).max(72.0)
        };
        let tile_h = if n == 1 {
            (tile_w * 9.0 / 16.0).clamp(140.0, 360.0)
        } else {
            (tile_w * 9.0 / 16.0).clamp(72.0, 280.0)
        };
        let size = Vec2::new(tile_w, tile_h);

        // Cap height when many streams so the call bar doesn't eat the chat.
        let rows = n.div_ceil(cols);
        let grid_h = rows as f32 * tile_h + (rows.saturating_sub(1) as f32) * sp.sm;
        const MAX_GRID_H: f32 = 420.0;
        if grid_h > MAX_GRID_H {
            ScrollArea::vertical()
                .id_salt("av_video_grid_scroll")
                .max_height(MAX_GRID_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    paint_av_video_grid(ui, th, &tiles, cols, size, &mut clicked);
                });
        } else {
            paint_av_video_grid(ui, th, &tiles, cols, size, &mut clicked);
        }
    }

    if let Some(key) = clicked {
        // Toggle: click focused tile to restore grid; otherwise switch focus.
        if state.av_focused_video.as_deref() == Some(key.as_str()) {
            state.av_focused_video = None;
        } else {
            state.av_focused_video = Some(key);
        }
    }
}

/// Place tiles in a fixed column grid (no wrap race / no shared rects).
fn paint_av_video_grid(
    ui: &mut egui::Ui,
    th: &Theme,
    tiles: &[(String, egui::TextureId, u32, u32)],
    cols: usize,
    size: Vec2,
    clicked: &mut Option<String>,
) {
    let cols = cols.max(1);
    let sp = th.spacing.sm;
    egui::Grid::new("av_video_grid")
        .num_columns(cols)
        .spacing(Vec2::new(sp, sp))
        .min_col_width(size.x)
        .max_col_width(size.x)
        .show(ui, |ui| {
            for (i, (key, tex_id, w, h)) in tiles.iter().enumerate() {
                if paint_av_video_tile(ui, th, key, *tex_id, *w, *h, size, false).clicked() {
                    *clicked = Some(key.clone());
                }
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
            if !tiles.is_empty() && tiles.len() % cols != 0 {
                ui.end_row();
            }
        });
}

/// One clickable video tile. Allocates an exact cell so neighbours cannot share space.
fn paint_av_video_tile(
    ui: &mut egui::Ui,
    th: &Theme,
    key: &str,
    tex_id: egui::TextureId,
    frame_w: u32,
    frame_h: u32,
    size: Vec2,
    focused: bool,
) -> egui::Response {
    let sp = &th.spacing;
    let p = &th.palette;
    // Store keys are `nick`, `nick~instance`, or `__local__`.
    let label = if key == "__local__" {
        "You".to_string()
    } else {
        crate::av::path_nick(key).to_string()
    };

    // Exact cell size — no nested vertical (those expand to full row width and
    // made multi-stream tiles stack on the same rect).
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, sp.radius_sm, p.card_bg);

    if resp.hovered() || focused {
        let stroke_c = if focused {
            p.accent
        } else {
            p.accent.gamma_multiply(0.55)
        };
        painter.rect_stroke(
            rect,
            sp.radius_sm,
            egui::Stroke::new(if focused { 2.0_f32 } else { 1.5_f32 }, stroke_c),
            egui::StrokeKind::Inside,
        );
    }

    let aspect = frame_w as f32 / frame_h.max(1) as f32;
    let fit = if aspect > size.x / size.y {
        Vec2::new(size.x, size.x / aspect)
    } else {
        Vec2::new(size.y * aspect, size.y)
    };
    let img_rect = egui::Rect::from_center_size(rect.center(), fit).intersect(rect);
    painter.image(
        tex_id,
        img_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    // Nick chip — truncate long nicks so they stay inside the tile.
    let nick_font = egui::FontId::proportional(th.type_scale.caption);
    let max_nick_w = (rect.width() - 12.0).max(24.0);
    let galley = ui.fonts(|f| f.layout_no_wrap(label.clone(), nick_font.clone(), p.text));
    let nick_text = if galley.size().x > max_nick_w {
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
        label
    };
    painter.text(
        rect.left_bottom() + Vec2::new(6.0, -4.0),
        Align2::LEFT_BOTTOM,
        nick_text,
        nick_font,
        p.text,
    );
    resp
}
