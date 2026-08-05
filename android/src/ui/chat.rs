//! Chat detail — messages + compose (freeq-android ChatDetailScreen inspired).

use eframe::egui::{self, text::CCursor, text::CCursorRange, Align, Align2, CursorIcon, Key, Layout, RichText, Sense, ScrollArea, Vec2};
use vidya::{
    button, command_shortcut_label, consume_command, consume_escape, dim_label, primary_button,
    Theme,
};

use crate::clipboard;
use crate::state::{AppState, ComposeAttach, NickTabComplete, ReplyTarget};
use crate::ui::search::{message_search_panel, SearchAction};
use crate::ui::widgets::{
    avatar_circle, empty_state, message_bubble, react_picker_overlay, MessageBubbleAction,
};

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
    /// Toggle speaker (remote audio) mute.
    AvToggleSpeakerMute,
    /// Toggle camera publish (MoQ).
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
    /// Edit a previously sent message (`+draft/edit`).
    Edit {
        target: String,
        msgid: String,
        text: String,
    },
    /// Soft-delete a message (`+draft/delete`).
    Delete { target: String, msgid: String },
}

fn apply_message_bubble_action(
    bubble: MessageBubbleAction,
    state: &mut AppState,
    channel: &str,
    action: &mut ChatAction,
) {
    match bubble {
        MessageBubbleAction::None => {}
        MessageBubbleAction::ToggleReaction { msgid, emoji } => {
            state.close_react_picker();
            *action = ChatAction::React {
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
        MessageBubbleAction::OpenImage { url } => {
            state.open_image_lightbox(url);
        }
        MessageBubbleAction::Edit { msgid, text } => {
            state.begin_edit(msgid, text);
        }
        MessageBubbleAction::Reply { msgid } => {
            let reply_msg = state
                .channels
                .get(channel)
                .and_then(|b| b.messages.iter().find(|m| m.id == msgid))
                .cloned();
            if let Some(msg) = reply_msg {
                state.begin_reply(&msg);
            }
        }
        MessageBubbleAction::Delete { msgid } => {
            *action = ChatAction::Delete {
                target: channel.to_string(),
                msgid,
            };
        }
    }
}

pub fn chat_screen(ui: &mut egui::Ui, th: &Theme, state: &mut AppState, channel: &str) -> ChatAction {
    let mut action = ChatAction::None;
    let sp = &th.spacing;
    let p = &th.palette;

    state.tick_search_highlight();

    // Message search hotkeys (vidya): Cmd/Ctrl+F open/refocus, Esc close.
    // Lightbox / react-picker Esc are handled in their overlays.
    let overlay_open =
        state.image_lightbox.is_some() || state.react_picker_msg.is_some();
    if !overlay_open && consume_command(ui, Key::F) {
        if state.show_message_search {
            state.focus_message_search = true;
        } else if !state.show_members {
            state.open_message_search();
        }
    } else if !overlay_open && state.show_message_search && consume_escape(ui) {
        state.close_message_search();
    }

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
    let show_search = state.show_message_search;
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
            if state.image_lightbox.is_some() {
                state.close_image_lightbox();
            } else if state.react_picker_msg.is_some() {
                state.close_react_picker();
            } else if show_search {
                state.close_message_search();
            } else if show_members {
                state.show_members = false;
            } else {
                action = ChatAction::Back;
            }
        }
        ui.add_space(sp.sm);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !show_search && is_channel && !show_members {
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
            // Close lives next to the search field; keep header Search only when closed.
            if !show_members && !show_search {
                if button(ui, th, "Search")
                    .on_hover_text(format!(
                        "Search messages ({})",
                        command_shortcut_label(Key::F)
                    ))
                    .clicked()
                {
                    state.open_message_search();
                }
            }
            let title_w = ui.available_width().max(40.0);
            ui.allocate_ui_with_layout(
                Vec2::new(title_w, ui.available_height().max(th.type_scale.title_2 * 2.2)),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_max_width(title_w);
                    if show_search {
                        ui.add(
                            egui::Label::new(
                                RichText::new("Search")
                                    .size(th.type_scale.title_2)
                                    .strong()
                                    .color(p.text),
                            )
                            .truncate(),
                        );
                        dim_label(ui, th, "Across chats");
                    } else if show_members {
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

    if show_search {
        match message_search_panel(ui, th, state) {
            SearchAction::None => {}
            SearchAction::Open { channel, msgid } => {
                state.navigate_to_message(&channel, &msgid);
            }
        }
        return action;
    }

    // ── AV call banner (start / join only; active call chrome is global) ─
    // The MoQ section for an open call lives in the app-level top panel so it
    // stays visible on every route — only idle/join controls stay here.
    // Idle is a single compact row; join is a short accent strip. When already
    // in a call, skip entirely: the global panel has mute/camera/leave/open.
    if is_channel && joined && state.local_call.is_none() {
        let prev_muted = state.av_pref_muted;
        let prev_speaker_muted = state.av_pref_speaker_muted;
        let prev_camera = state.av_pref_camera;
        let prev_cam_id = state.av_pref_camera_id.clone();
        let prev_mic = state.av_pref_mic_id.clone();
        let prev_spk = state.av_pref_speaker_id.clone();
        if let Some(act) = av_call_banner(ui, th, channel, channel_call.as_ref(), state) {
            action = act;
        }
        if state.av_pref_muted != prev_muted
            || state.av_pref_speaker_muted != prev_speaker_muted
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
    try_paste_compose_attach(ui, state);

    let body_w = ui.available_width().max(80.0);

    let has_attach = state.compose_attach.is_some();
    let uploading = state.compose_uploading;
    let can_send = !uploading && (!state.compose.trim().is_empty() || has_attach);
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

            if has_attach {
                if let Some(send) = compose_attach_composer(ui, th, state, compose_id) {
                    action = ChatAction::Send {
                        target: channel.to_string(),
                        text: send,
                    };
                }
            } else {
                let is_editing = state.editing_msgid.is_some();
                let is_replying = state.replying_to.is_some();
                if is_replying {
                    if let Some(reply) = state.replying_to.clone() {
                        let body = reply_body_preview(&reply);
                        if compose_context_dismiss_bar(
                            ui,
                            th,
                            &format!("Replying to {}", reply.from),
                            Some(&body),
                            "Cancel reply (Esc)",
                        ) {
                            state.cancel_reply();
                        }
                        ui.add_space(sp.xs);
                        if state.react_picker_msg.is_none() && consume_escape(ui) {
                            state.cancel_reply();
                        }
                    }
                }
                if is_editing {
                    if compose_context_dismiss_bar(
                        ui,
                        th,
                        "Editing message",
                        None,
                        "Cancel edit (Esc)",
                    ) {
                        state.cancel_edit();
                    }
                    ui.add_space(sp.xs);
                    if state.react_picker_msg.is_none() && consume_escape(ui) {
                        state.cancel_edit();
                    }
                }
                let attach_tip = if pick_busy {
                    "Opening file picker…"
                } else if cfg!(target_os = "android") {
                    "Attach image or video"
                } else {
                    "Attach image or video (or paste image with Ctrl+V)"
                };
                let (hint, action_label, action_w) = if is_editing {
                    ("Edit message…", "Save", 72.0_f32)
                } else if is_replying {
                    ("Reply…", "Send", 72.0_f32)
                } else {
                    ("Message…", "Send", 72.0_f32)
                };
                let (resp, attach_clicked, send_clicked) = compose_input_row(
                    ui,
                    th,
                    &mut state.compose,
                    hint,
                    action_label,
                    action_w,
                    attach_tip,
                    true,
                    compose_id,
                );
                if attach_clicked && !pick_busy && !is_editing && !is_replying {
                    state.start_file_pick();
                }
                if state.focus_compose {
                    resp.request_focus();
                    state.focus_compose = false;
                }
                let enter = resp.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if (send_clicked || enter) && can_send {
                    let text = state.compose.trim().to_string();
                    if let Some(msgid) = state.editing_msgid.clone() {
                        action = ChatAction::Edit {
                            target: channel.to_string(),
                            msgid,
                            text,
                        };
                    } else {
                        action = ChatAction::Send {
                            target: channel.to_string(),
                            text,
                        };
                    }
                }
            }
        });

    // Messages take everything above the compose bar.
    let msg_h = ui.available_height().max(80.0);
    let jump_id = egui::Id::new("chat_jump_to_bottom").with(channel);
    let want_jump = ui.ctx().data_mut(|d| d.get_temp::<bool>(jump_id).unwrap_or(false));
    // Keep stick-to-bottom off for the whole highlight window, not just the
    // one-shot `scroll_to_msgid` frame. egui remembers `scroll_stuck_to_end`
    // across frames: if we re-enable stick while still marked stuck, end()
    // yanks us back to the bottom before stuck is recomputed, and the search
    // jump looks like a no-op.
    //
    // Keep ScrollArea animation enabled during the hold: `animated(false)`
    // applies the jump without clearing kinetic `vel`, so a prior fling undoes
    // it. `ScrollAnimation::none()` still lands in one frame via offset_target,
    // which zeroes velocity.
    let hold_search_scroll =
        state.scroll_to_msgid.is_some() || state.highlight_msgid.is_some();

    // Do **not** pair this with `vertical_scroll_offset(f32::MAX)`: that value
    // destroys f32 layout precision, so `scroll_to_cursor` computes a bogus
    // target (often ~top) and the jump appears to do nothing / go the wrong way.
    // Per-channel id: shared "chat_scroll" reused offset/stuck state across rooms
    // and made search jumps land in the wrong place after switching buffers.
    let scroll_out = ScrollArea::vertical()
        .id_salt(("chat_scroll", channel))
        .stick_to_bottom(!hold_search_scroll)
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
                let scroll_target = state.scroll_to_msgid.clone();
                let highlight_id = state.highlight_msgid.clone();
                let mut did_scroll = false;
                let mut target_in_view = false;
                let msg_by_id: std::collections::HashMap<&str, &crate::state::ChatMessage> =
                    messages.iter().map(|m| (m.id.as_str(), m)).collect();
                for msg in &messages {
                    let highlighted = highlight_id.as_ref().is_some_and(|id| id == &msg.id);
                    let picker_open = state
                        .react_picker_msg
                        .as_ref()
                        .is_some_and(|id| id == &msg.id);
                    let want_scroll = scroll_target.as_ref().is_some_and(|id| id == &msg.id);
                    let reply_parent = msg
                        .reply_to
                        .as_deref()
                        .and_then(|id| msg_by_id.get(id).copied());
                    let outer = ui.push_id(("chat_msg", msg.id.as_str()), |ui| {
                        apply_message_bubble_action(
                            message_bubble(
                                ui,
                                th,
                                msg,
                                reply_parent,
                                &own_nick,
                                &mut state.media,
                                picker_open,
                                highlighted,
                            ),
                            state,
                            channel,
                            &mut action,
                        );
                    });
                    if want_scroll {
                        // Keep requesting until the bubble intersects the viewport;
                        // clearing on the first frame often no-ops (layout not ready).
                        outer.response.scroll_to_me_animation(
                            Some(Align::Center),
                            egui::style::ScrollAnimation::none(),
                        );
                        did_scroll = true;
                        let clip = ui.clip_rect();
                        // Any overlap is enough — requiring the center in-view
                        // never clears for tall (image) bubbles.
                        target_in_view = clip.intersects(outer.response.rect)
                            && outer.response.rect.height() > 1.0;
                        ui.ctx().request_repaint();
                    }
                    ui.add_space(sp.sm);
                }
                // Only consume once the hit is on-screen (or missing from buffer).
                if scroll_target.is_some() {
                    if !did_scroll {
                        state.scroll_to_msgid = None;
                        state.clear_search_highlight();
                    } else if target_in_view {
                        state.scroll_to_msgid = None;
                    }
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

    // Modal reaction picker (above chat; Esc / backdrop dismiss).
    if let Some((msgid, emoji)) = react_picker_overlay(ui.ctx(), th, state, channel) {
        action = ChatAction::React {
            target: channel.to_string(),
            msgid,
            emoji,
        };
    }

    action
}

/// Media attachment preview + caption/submit row (mirrors the normal compose bar).
///
/// Returns `Some(caption)` when the user submits.
/// Body line for the reply compose bar (`ReplyTarget::preview` includes the nick).
fn reply_body_preview(reply: &ReplyTarget) -> String {
    let prefix = format!("{}: ", reply.from);
    if reply.preview.starts_with(&prefix) {
        reply.preview[prefix.len()..].to_string()
    } else {
        reply.preview.clone()
    }
}

/// Compose-bar context row (reply / edit) with a fixed dismiss control on the right.
///
/// Matches the attach composer layout: meta text uses bounded width + truncation so
/// the ✕ button stays visible on narrow APK screens (a single `horizontal` with
/// unbounded labels was clipping "Cancel" off the right edge).
fn compose_context_dismiss_bar(
    ui: &mut egui::Ui,
    th: &Theme,
    title: &str,
    subtitle: Option<&str>,
    dismiss_tooltip: &str,
) -> bool {
    let p = &th.palette;
    let sp = &th.spacing;
    let dismiss_w = 32.0_f32;
    let bar_w = ui.available_width().max(48.0);
    let meta_w = (bar_w - dismiss_w - sp.xs).max(48.0);
    let mut dismissed = false;

    ui.horizontal(|ui| {
        ui.set_width(bar_w);
        ui.vertical(|ui| {
            ui.set_width(meta_w);
            ui.add(
                egui::Label::new(
                    RichText::new(title)
                        .size(th.type_scale.caption)
                        .color(p.accent)
                        .strong(),
                )
                .truncate(),
            );
            if let Some(sub) = subtitle.filter(|s| !s.is_empty()) {
                ui.add(
                    egui::Label::new(
                        RichText::new(sub)
                            .size(th.type_scale.caption)
                            .color(p.text_secondary),
                    )
                    .truncate(),
                );
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                .on_hover_text(dismiss_tooltip)
                .on_hover_cursor(CursorIcon::PointingHand);
            dismissed = dismiss.clicked();
        });
    });

    dismissed
}

fn compose_attach_composer(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    compose_id: egui::Id,
) -> Option<String> {
    let p = &th.palette;
    let sp = &th.spacing;

    enum Thumb {
        Image {
            tex_id: egui::TextureId,
            width: usize,
            height: usize,
            dims: String,
        },
        Video {
            filename: String,
            dims: String,
        },
    }

    let (thumb, kind_label, uploading) = {
        let Some(attach) = state.compose_attach.as_mut() else {
            return None;
        };
        let uploading = state.compose_uploading;
        let kind = attach.kind_label();
        let thumb = match attach {
            ComposeAttach::Image(img) => {
                let tex = img.texture(ui.ctx()).clone();
                let kb = (img.width.saturating_mul(img.height).saturating_mul(4) / 1024).max(1);
                Thumb::Image {
                    tex_id: tex.id(),
                    width: img.width,
                    height: img.height,
                    dims: format!("{}×{} · ~{kb} KB", img.width, img.height),
                }
            }
            ComposeAttach::Video(video) => Thumb::Video {
                filename: video.filename.clone(),
                dims: format!("{} · {}", video.content_type, video.size_label()),
            },
        };
        (thumb, kind, uploading)
    };

    const THUMB: f32 = 48.0;
    let mut clear = false;
    let mut submit = false;
    let pick_busy = state.file_pick_busy();
    let can_send = !uploading;
    let bar_w = ui.available_width();

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
                match &thumb {
                    Thumb::Image {
                        tex_id,
                        width,
                        height,
                        ..
                    } => {
                        let scale = (THUMB / (*width).max(1) as f32)
                            .min(THUMB / (*height).max(1) as f32)
                            .min(1.0);
                        let img_size = Vec2::new(
                            (*width as f32 * scale).max(1.0),
                            (*height as f32 * scale).max(1.0),
                        );
                        let img_rect = egui::Rect::from_center_size(thumb_rect.center(), img_size);
                        ui.put(
                            img_rect,
                            egui::Image::new((*tex_id, img_size))
                                .corner_radius(sp.radius_sm)
                                .sense(Sense::hover()),
                        );
                    }
                    Thumb::Video { .. } => {
                        // Play glyph stand-in (matches chat video preview).
                        ui.painter().circle_filled(
                            thumb_rect.center(),
                            12.0,
                            p.accent.gamma_multiply(0.92),
                        );
                        let c = thumb_rect.center();
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(c.x + 6.0, c.y),
                                egui::pos2(c.x - 5.0, c.y + 5.5),
                                egui::pos2(c.x - 5.0, c.y - 5.5),
                            ],
                            egui::Color32::WHITE,
                            egui::Stroke::NONE,
                        ));
                    }
                }

                ui.add_space(sp.sm);

                let dismiss_w = 32.0_f32;
                let meta_w = (ui.available_width() - dismiss_w - sp.xs).max(48.0);
                let dims = match &thumb {
                    Thumb::Image { dims, .. } | Thumb::Video { dims, .. } => dims.clone(),
                };
                let title = match &thumb {
                    Thumb::Video { filename, .. } if !uploading => filename.clone(),
                    _ if uploading => "Uploading…".into(),
                    _ => format!("{kind_label} attached"),
                };
                ui.vertical(|ui| {
                    ui.set_width(meta_w);
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(title)
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
                        ui.add_space(sp.xs);
                    }
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
                        .on_hover_text(if uploading {
                            "Cancel upload"
                        } else {
                            "Remove attachment"
                        })
                        .on_hover_cursor(CursorIcon::PointingHand);
                    if dismiss.clicked() {
                        clear = true;
                    }
                });
            });
        });

    ui.add_space(sp.sm);

    let attach_tip = if pick_busy {
        "Opening file picker…"
    } else if cfg!(target_os = "android") {
        "Replace attachment"
    } else {
        "Replace attachment (or paste image with Ctrl+V)"
    };
    let (resp, attach_clicked, send_clicked) = compose_input_row(
        ui,
        th,
        &mut state.compose,
        "Caption (optional)…",
        "Submit",
        80.0,
        attach_tip,
        true,
        compose_id,
    );
    if attach_clicked && !uploading && !pick_busy {
        state.start_file_pick();
    }
    if state.focus_compose {
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
        if uploading {
            state.cancel_compose_upload();
        } else {
            state.clear_compose_attach();
        }
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
///
/// Clipboard image reads run with a short timeout off the UI thread (see
/// [`clipboard::try_get_image`]) so a stuck X11/Wayland conversion cannot
/// freeze egui's immediate-mode loop.
fn try_paste_compose_attach(ui: &mut egui::Ui, state: &mut AppState) {
    // Don't replace the attachment mid-upload; text paste still reaches the
    // interactive caption field via `Event::Paste`.
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

    state.compose_attach = Some(ComposeAttach::Image(img));
    state.focus_compose = true;
}

/// Global MoQ call chrome — always painted while `local_call` is set so the
/// section stays visible on every route (chat, tabs, settings). One call only.
pub fn active_call_panel(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) -> Option<ChatAction> {
    let Some(lc) = state.local_call.clone() else {
        return None;
    };
    active_call_panel_body(ui, th, state, &lc)
}

/// Call strip (top panel while chat/tabs stay visible). Video height is
/// user-resizable via the drag handle under the tiles.
fn active_call_panel_body(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    lc: &crate::av::LocalCall,
) -> Option<ChatAction> {
    let sp = &th.spacing;
    let p = &th.palette;
    let mut action = None;

    let (headline, status_line, on_call_channel) = av_call_chrome_meta(state, lc);

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
        // Also keep the stage up while MoQ re-dials (status stays Connecting).
        if matches!(
            lc.media,
            crate::av::MediaStatus::Live | crate::av::MediaStatus::Connecting
        ) || state.media_reconnect_at.is_some()
            || state.av_video.as_ref().is_some_and(|s| !s.is_empty())
        {
            let screen_h = ui.ctx().screen_rect().height();
            let min_h = 96.0;
            let max_h = (screen_h * 0.72).max(min_h + 40.0);
            state.av_video_height = state.av_video_height.clamp(min_h, max_h);

            let video_size = Vec2::new(panel_w, state.av_video_height);
            ui.add_space(sp.sm);
            ui.allocate_ui_with_layout(video_size, Layout::top_down(Align::Center), |ui| {
                ui.set_min_size(video_size);
                ui.set_max_size(video_size);
                let stage = ui.max_rect();
                ui.painter()
                    .rect_filled(stage, sp.radius_sm, p.card_bg.gamma_multiply(0.85));
                paint_av_video_tiles(ui, th, state, Some(video_size));
            });
            paint_av_video_resize_handle(ui, th, state, min_h, max_h);
        }

        ui.add_space(sp.xs);
        if let Some(act) = av_call_controls_row(ui, th, state, lc, on_call_channel) {
            action = Some(act);
        }

        if state.av_show_devices {
            av_devices_expanded(ui, th, state, /*in_call=*/ true, &mut action);
        }
    });

    action
}

/// Drag handle under the video stage — vertical resize of `av_video_height`.
fn paint_av_video_resize_handle(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    min_h: f32,
    max_h: f32,
) {
    let p = &th.palette;
    let handle_h = 10.0;
    let w = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(w, handle_h), Sense::click_and_drag());

    let active = response.hovered() || response.dragged();
    if active {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }

    let bar_w = (w * 0.18).clamp(28.0, 56.0);
    let bar = egui::Rect::from_center_size(rect.center(), Vec2::new(bar_w, 3.0));
    let color = if active {
        p.accent
    } else {
        p.border_soft
    };
    ui.painter().rect_filled(bar, 1.5, color);

    if response.dragged() {
        state.av_video_height =
            (state.av_video_height + response.drag_delta().y).clamp(min_h, max_h);
        ui.ctx().request_repaint();
    }

    response.on_hover_text("Drag to resize video");
}

fn av_call_chrome_meta(
    state: &AppState,
    lc: &crate::av::LocalCall,
) -> (String, String, bool) {
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
    let headline = call_title.unwrap_or(lc.channel.as_str()).to_string();
    let on_call_channel = matches!(
        &state.route,
        crate::state::Route::Chat(ch) if ch.eq_ignore_ascii_case(&lc.channel)
    );
    let status_line = if state.media_reconnect_at.is_some() {
        let delay = state
            .media_reconnect_at
            .map(|at| {
                at.saturating_duration_since(std::time::Instant::now())
                    .as_secs()
            })
            .unwrap_or(0);
        format!("Reconnecting media in {delay}s… · {n}")
    } else {
        match &lc.media {
            crate::av::MediaStatus::Live => format!("{n} in call"),
            crate::av::MediaStatus::Idle => format!("{n} in call"),
            crate::av::MediaStatus::Connecting => format!("Connecting… · {n}"),
            crate::av::MediaStatus::Failed(e) => format!("Media failed · {e}"),
            crate::av::MediaStatus::BrowserOnly => "Open in browser for media".to_string(),
        }
    };
    (headline, status_line, on_call_channel)
}

/// Mic / speaker / camera / leave / Devices strip.
fn av_call_controls_row(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    lc: &crate::av::LocalCall,
    on_call_channel: bool,
) -> Option<ChatAction> {
    let sp = &th.spacing;
    let mut action = None;

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
                "Unmute mic"
            } else {
                "Mute mic"
            },
        )
        .clicked()
        {
            if lc.has_mic {
                action = Some(ChatAction::AvToggleMute);
            }
        }
        if matches!(lc.media, crate::av::MediaStatus::Live) {
            let level = state
                .av_mic_level
                .as_ref()
                .map(|m| m.get())
                .unwrap_or(0.0);
            paint_mic_level_meter(ui, th, level, lc.muted || !lc.has_mic);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        if av_icon_toggle(
            ui,
            th,
            "🔊",
            lc.speaker_muted,
            if lc.speaker_muted {
                "Unmute speaker"
            } else {
                "Mute speaker"
            },
        )
        .clicked()
        {
            action = Some(ChatAction::AvToggleSpeakerMute);
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

/// Pre-call mic/speaker/camera toggles (icons match in-call controls).
fn av_media_prefs_row(ui: &mut egui::Ui, th: &Theme, state: &mut AppState) {
    let sp = &th.spacing;
    ui.horizontal(|ui| {
        if av_icon_toggle(
            ui,
            th,
            "🎤",
            state.av_pref_muted,
            if state.av_pref_muted {
                "Unmute mic"
            } else {
                "Mute mic"
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
            "🔊",
            state.av_pref_speaker_muted,
            if state.av_pref_speaker_muted {
                "Unmute speaker"
            } else {
                "Mute speaker"
            },
        )
        .clicked()
        {
            state.av_pref_speaker_muted = !state.av_pref_speaker_muted;
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
///
/// Idle stays a single compact row so chat keeps vertical space; join gets a
/// short accent strip (title + Join + prefs) without stacking empty chrome.
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

    // Idle: one row — Call + mic/speaker/camera + Devices. No subtitle block.
    if channel_call.is_none() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = sp.sm;
            if button(ui, th, "📞 Call")
                .on_hover_text("Start voice & video call")
                .clicked()
            {
                action = Some(ChatAction::AvStart(channel.to_string()));
            }
            av_media_prefs_row(ui, th, state);
            av_devices_toggle_button(ui, th, state);
        });
        // Pickers expand below the strip so they don't fight the wrap layout.
        if state.av_show_devices {
            av_devices_expanded(ui, th, state, /*in_call=*/ false, &mut action);
        }
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
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.xs as i8));

    frame.show(ui, |ui| {
        let panel_w = ui.available_width();
        ui.set_width(panel_w);

        // Title · count on one line, then Join + prefs on the next.
        ui.horizontal(|ui| {
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
            ui.add_space(sp.sm);
            dim_label(ui, th, &format!("{n} in call"));
        });

        ui.add_space(sp.xs);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = sp.sm;
            if primary_button(ui, th, "Join call").clicked() {
                action = Some(ChatAction::AvJoin {
                    channel: channel.to_string(),
                    session_id: call.session_id.clone(),
                });
            }
            av_media_prefs_row(ui, th, state);
            av_devices_toggle_button(ui, th, state);
        });
        if state.av_show_devices {
            av_devices_expanded(ui, th, state, /*in_call=*/ false, &mut action);
        }
    });

    action
}

/// Stacked Cam/Mic/Out rows (call after the Devices toggle when open).
fn av_devices_expanded(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    in_call: bool,
    action: &mut Option<ChatAction>,
) {
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

/// Compact call-bar video height budget — scale down on short (phone landscape)
/// viewports so the strip doesn't push chat/compose off-screen.
fn av_compact_video_caps(ui: &egui::Ui) -> (f32, f32, f32) {
    // (single max_h, multi max_h, grid max_h)
    let h = ui.ctx().screen_rect().height();
    if h < 420.0 {
        let single = (h * 0.28).clamp(88.0, 140.0);
        (single, (single * 0.75).max(72.0), (h * 0.36).clamp(100.0, 160.0))
    } else if h < 520.0 {
        let single = (h * 0.34).clamp(120.0, 200.0);
        (single, (single * 0.8).max(80.0), (h * 0.42).clamp(140.0, 220.0))
    } else {
        (360.0, 280.0, 420.0)
    }
}

/// Paint remote (+ local preview) video tiles from the MoQ frame store.
///
/// Streams are laid out in a fixed multi-column grid (or focus + filmstrip)
/// so frames never share the same rect. Click a tile to enlarge/focus it;
/// click the focused tile again to restore the grid.
///
/// `fill_size`: when `Some`, tiles expand to fill that exact stage (theater
/// mode). When `None`, compact call-bar caps keep chat usable.
fn paint_av_video_tiles(
    ui: &mut egui::Ui,
    th: &Theme,
    state: &mut AppState,
    fill_size: Option<Vec2>,
) {
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
    let has_local = frames
        .iter()
        .any(|(k, _)| k == crate::av::LOCAL_PREVIEW_KEY);
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
            crate::av::LOCAL_PREVIEW_KEY.into(),
            crate::av::RgbaVideoFrame {
                width: 8,
                height: 8,
                rgba: Arc::<[u8]>::from(px),
                gen: 0,
            },
        ));
    }
    if frames.is_empty() {
        return;
    }
    // Keep painting while frames arrive (software GL + MoQ can stall idle ticks).
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    // Local preview first, then remotes alphabetically (key = nick or nick~instance).
    frames.sort_by(|a, b| {
        let a_local = a.0 == crate::av::LOCAL_PREVIEW_KEY;
        let b_local = b.0 == crate::av::LOCAL_PREVIEW_KEY;
        b_local
            .cmp(&a_local)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let sp = &th.spacing;
    if fill_size.is_none() {
        ui.add_space(sp.sm);
    }

    let live: std::collections::HashSet<String> = frames.iter().map(|(n, _)| n.clone()).collect();
    state.av_video_textures.retain(|k| live.contains(k));
    // Drop focus if that participant left / stopped publishing.
    if state
        .av_focused_video
        .as_ref()
        .is_some_and(|n| !live.contains(n))
    {
        state.av_focused_video = None;
    }

    // Upload / refresh GPU textures first so paint helpers only need ids + dims.
    // Skip `TextureHandle::set` when the frame gen is unchanged — full RGBA
    // re-uploads every tick were thrashing software GL and looked like drops.
    let mut tiles: Vec<(String, egui::TextureId, u32, u32)> = Vec::with_capacity(frames.len());
    for (key, frame) in &frames {
        let tex_id = match state.av_video_textures.0.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let (tex, uploaded_gen) = e.get_mut();
                if *uploaded_gen != frame.gen {
                    let color = color_image_opaque_rgb(
                        frame.width as usize,
                        frame.height as usize,
                        frame.rgba.as_ref(),
                    );
                    tex.set(color, egui::TextureOptions::LINEAR);
                    *uploaded_gen = frame.gen;
                }
                tex.id()
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                let color = color_image_opaque_rgb(
                    frame.width as usize,
                    frame.height as usize,
                    frame.rgba.as_ref(),
                );
                let tex = ui.ctx().load_texture(
                    format!("av_video_{key}"),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                let id = tex.id();
                e.insert((tex, frame.gen));
                id
            }
        };
        tiles.push((key.clone(), tex_id, frame.width, frame.height));
    }

    // Theater stage size is explicit; compact mode uses call-bar caps.
    let (compact_single_max, compact_multi_max, compact_grid_max) = av_compact_video_caps(ui);
    let (stage_w, height_budget, fill) = if let Some(sz) = fill_size {
        (sz.x.max(1.0), sz.y.max(1.0), true)
    } else {
        (ui.available_width().max(1.0), compact_grid_max, false)
    };

    let focused = state.av_focused_video.clone();
    let mut clicked: Option<String> = None;

    if let Some(focus_key) = focused.as_ref() {
        // Enlarged primary + filmstrip of the rest.
        let avail = stage_w;
        let n_others = tiles.iter().filter(|(n, _, _, _)| n != focus_key).count();
        let strip_h = if n_others > 0 {
            let thumb_cols = n_others.min(4).max(1);
            let gaps = sp.sm * (thumb_cols.saturating_sub(1) as f32);
            let thumb_cap = if fill { 220.0 } else { 160.0 };
            let thumb_w = ((avail - gaps) / thumb_cols as f32).clamp(64.0, thumb_cap);
            // On short viewports keep thumbs squat so primary still fits.
            let thumb_aspect = if fill { 9.0 / 16.0 } else {
                (compact_multi_max / thumb_w.max(1.0)).min(9.0 / 16.0)
            };
            thumb_w * thumb_aspect + sp.xs
        } else {
            0.0
        };
        let primary_w = avail;
        let primary_h = if fill {
            (height_budget - strip_h).max(120.0)
        } else {
            (primary_w * 9.0 / 16.0).clamp(96.0, compact_single_max)
        };

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
            let thumb_cap = if fill { 220.0 } else { 160.0 };
            let thumb_w = ((avail - gaps) / thumb_cols as f32).clamp(64.0, thumb_cap);
            let thumb_h = if fill {
                thumb_w * 9.0 / 16.0
            } else {
                (thumb_w * 9.0 / 16.0).min(compact_multi_max * 0.55)
            };
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
        // Equal grid: columns from stream count, cell size from stage width.
        let avail = stage_w;
        let n = tiles.len().max(1);
        let cols = av_grid_cols(n);
        let gaps = sp.sm * (cols.saturating_sub(1) as f32);
        let rows = n.div_ceil(cols);
        let row_gaps = (rows.saturating_sub(1) as f32) * sp.sm;

        let (tile_w, tile_h) = if fill {
            // Fill the stage: each cell uses the full cell rect; frames
            // letterbox inside paint_av_video_tile.
            let cell_w = if n == 1 {
                avail
            } else {
                ((avail - gaps) / cols as f32).max(72.0)
            };
            let cell_h = ((height_budget - row_gaps) / rows as f32).max(72.0);
            (cell_w, cell_h)
        } else {
            let tile_w = if n == 1 {
                avail
            } else {
                ((avail - gaps) / cols as f32).max(72.0)
            };
            let tile_h = if n == 1 {
                (tile_w * 9.0 / 16.0).clamp(96.0, compact_single_max)
            } else {
                (tile_w * 9.0 / 16.0).clamp(72.0, compact_multi_max)
            };
            (tile_w, tile_h)
        };
        let size = Vec2::new(tile_w, tile_h);

        // Cap height when many streams so the call bar doesn't eat the chat.
        let grid_h = rows as f32 * tile_h + row_gaps;
        let max_grid_h = if fill { height_budget } else { compact_grid_max };
        if !fill && grid_h > max_grid_h {
            ScrollArea::vertical()
                .id_salt("av_video_grid_scroll")
                .max_height(max_grid_h)
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

/// Build a fully opaque `ColorImage` via the shipped [`crate::av::prepare_opaque_rgba_for_upload`].
fn color_image_opaque_rgb(width: usize, height: usize, rgba: &[u8]) -> egui::ColorImage {
    let buf = crate::av::prepare_opaque_rgba_for_upload(width, height, rgba);
    egui::ColorImage::from_rgba_unmultiplied([width, height], &buf)
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
    let label = if key == crate::av::LOCAL_PREVIEW_KEY {
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
