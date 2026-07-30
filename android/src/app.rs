//! Sleek eframe app — desktop + Android.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Stroke, Vec2};
use freeq_sdk::event::Event;
use vidya::{apply, body, button, dim_label, reserve_system_chrome, title, Mode, Theme};

use crate::auth::{self, AuthTokens};
use crate::av::{self, LocalCall, MediaStatus};
use crate::clipboard;
use crate::net::{NetBridge, NetCmd, NetEvent};
use crate::preview;
use crate::state::{
    api_base_for_server, AppState, CachedPixels, ChatMessage, ConnectionState, LinkMeta, MediaFetch,
    Route, Tab,
};
use crate::ui::{
    self, ChatAction, ChatsAction, ConnectAction, DiscoverAction, SettingsAction,
};

/// Desktop / host entry.
pub fn run_desktop() -> eframe::Result {
    #[cfg(not(target_os = "android"))]
    {
        // Default info; clipboard miss noise stays at debug (vendored egui-winit).
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .try_init();
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([390.0, 780.0])
            .with_min_inner_size([320.0, 560.0])
            .with_title("Sleek"),
        ..Default::default()
    };
    eframe::run_native(
        "Sleek",
        options,
        Box::new(|cc| Ok(Box::new(SleekApp::new(cc)))),
    )
}

/// Android NativeActivity entry.
#[cfg(target_os = "android")]
pub fn run_android(android_app: winit::platform::android::activity::AndroidApp) -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Sleek"),
        ..Default::default()
    };
    options.android_app = Some(android_app);
    eframe::run_native(
        "Sleek",
        options,
        Box::new(|cc| Ok(Box::new(SleekApp::new(cc)))),
    )
}

struct SleekApp {
    mode: Mode,
    state: AppState,
    net: NetBridge,
}

impl SleekApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let theme = Theme::dark();
        apply(&cc.egui_ctx, &theme);
        let state = AppState::new();
        let net = NetBridge::start();
        let mut app = Self {
            mode: Mode::Dark,
            state,
            net,
        };
        // freeq-android FreeqApp: auto-reconnect saved broker session on launch.
        // Guest path: same idea — reconnect with the remembered nick.
        if app.state.has_saved_session() {
            app.do_reconnect_session();
        } else if app.state.auto_guest_connect {
            app.state.auto_guest_connect = false;
            app.do_connect();
        }
        app
    }

    fn theme(&self) -> Theme {
        match self.mode {
            Mode::Dark => Theme::dark(),
            Mode::Light => Theme::light(),
        }
    }

    fn set_mode(&mut self, ctx: &egui::Context, mode: Mode) {
        self.mode = mode;
        apply(ctx, &self.theme());
    }

    fn poll_net(&mut self, ctx: &egui::Context) {
        for ev in self.net.poll() {
            self.handle_net_event(ev);
        }
        // Kick off any image / OG fetches requested while painting chat.
        self.flush_media_fetches();
        // Keep UI live while connecting / connected / media loading / in a video call.
        let in_live_call = self
            .state
            .local_call
            .as_ref()
            .is_some_and(|lc| matches!(lc.media, MediaStatus::Live | MediaStatus::Connecting));
        if self.state.connection == ConnectionState::Connecting
            || self.state.connection.is_live()
            || self.state.awaiting_oauth
            || self.state.media.has_loading()
            || in_live_call
        {
            // ~30 fps while a call is live so remote tiles update smoothly.
            let ms = if in_live_call { 33 } else { 50 };
            ctx.request_repaint_after(std::time::Duration::from_millis(ms));
        }
    }

    /// Send queued media fetches to the net thread.
    fn flush_media_fetches(&mut self) {
        for req in self.state.media.drain_pending() {
            match req {
                MediaFetch::Image(url) => self.net.send(NetCmd::FetchImage { url }),
                MediaFetch::LinkPreview(url) => self.net.send(NetCmd::FetchLinkPreview { url }),
            }
        }
    }

    /// Drain finished OS file-dialog results; keep repainting while the dialog is open.
    fn poll_file_pick(&mut self, ctx: &egui::Context) {
        if self.state.poll_file_pick() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn handle_net_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Status(s) => {
                self.state.status_line = s;
            }
            NetEvent::Failed(s) => {
                // Av-start / av-join failure: drop the optimistic one-call claim.
                if s.starts_with("av-start") || s.starts_with("av-join") {
                    if self.state.local_call.as_ref().is_some_and(|lc| {
                        lc.awaiting_start || lc.instance.is_empty() || lc.session_id.is_empty()
                    }) {
                        self.state.local_call = None;
                        self.state.av_video = None;
                        self.state.av_video_textures.clear();
                    }
                }
                self.state.error = Some(s.clone());
                self.state.show_toast(s);
                self.state.awaiting_oauth = false;
                if self.state.connection == ConnectionState::Connecting {
                    self.state.connection = ConnectionState::Disconnected;
                }
            }
            NetEvent::AuthReady(tokens) => {
                self.state.awaiting_oauth = false;
                self.apply_auth_tokens(tokens, /*connect=*/ true);
            }
            NetEvent::UploadFinished { error, sent } => {
                self.state.compose_uploading = false;
                if let Some(err) = error {
                    self.state.error = Some(err.clone());
                    self.state.show_toast(err);
                    // Keep compose_image so the user can retry Send.
                } else {
                    self.state.compose_image = None;
                    self.state.compose.clear();
                    self.state.show_toast("Image sent");
                    if let Some(media) = sent {
                        self.do_send_local_echo(&media.target, media.text);
                    }
                }
            }
            NetEvent::ImageFetched {
                url,
                width,
                height,
                rgba,
            } => {
                self.state
                    .media
                    .set_image_ready(url, CachedPixels::new(width, height, rgba));
            }
            NetEvent::ImageFetchFailed { url } => {
                self.state.media.set_image_failed(url);
            }
            NetEvent::LinkPreviewFetched {
                url,
                title,
                description,
                thumb_url,
                site_name,
            } => {
                self.state.media.set_link_ready(
                    url,
                    LinkMeta {
                        title,
                        description,
                        thumb_url,
                        site_name,
                    },
                );
            }
            NetEvent::LinkPreviewFailed { url } => {
                self.state.media.set_link_failed(url);
            }
            NetEvent::AvSignalingSent {
                channel,
                session_id,
                instance,
                started,
            } => {
                let sid = session_id.clone().unwrap_or_default();
                // Fill in the instance (and session for join) on the single slot
                // we claimed in do_av_start / do_av_join.
                if let Some(lc) = self.state.local_call.as_mut() {
                    lc.channel = channel.clone();
                    if !sid.is_empty() {
                        lc.session_id = sid.clone();
                    }
                    lc.instance = instance.clone();
                    lc.awaiting_start = started;
                } else {
                    self.state.local_call = Some(LocalCall {
                        channel: channel.clone(),
                        session_id: sid.clone(),
                        instance: instance.clone(),
                        token: None,
                        muted: false,
                        camera: false,
                        has_camera: false,
                        media: MediaStatus::Idle,
                        awaiting_start: started,
                    });
                }
                if started {
                    self.state.show_toast("Starting call…");
                } else {
                    self.state.show_toast("Joining call…");
                    // Join already has a session id — dial SFU (token may arrive next).
                    if !sid.is_empty() {
                        self.try_start_av_media(&channel, &sid, &instance, None);
                    }
                }
            }
            NetEvent::AvMediaStatus {
                status,
                video,
                has_camera,
            } => {
                if let Some(lc) = self.state.local_call.as_mut() {
                    lc.media = status.clone();
                    if matches!(status, MediaStatus::Live) {
                        lc.has_camera = has_camera;
                        // Camera starts enabled when hardware opened.
                        lc.camera = has_camera;
                    }
                    if matches!(
                        status,
                        MediaStatus::Idle | MediaStatus::Failed(_) | MediaStatus::BrowserOnly
                    ) {
                        lc.has_camera = false;
                        lc.camera = false;
                    }
                }
                if let Some(store) = video {
                    self.state.av_video = Some(store);
                }
                if matches!(
                    status,
                    MediaStatus::Idle | MediaStatus::Failed(_) | MediaStatus::BrowserOnly
                ) {
                    self.state.av_video = None;
                    self.state.av_video_textures.clear();
                }
                match &status {
                    MediaStatus::Live => {
                        let cam = if has_camera { " + camera" } else { "" };
                        self.state
                            .show_toast(format!("Call media connected{cam}"));
                    }
                    MediaStatus::Failed(e) => {
                        self.state.show_toast(format!("Call media failed: {e}"));
                    }
                    _ => {}
                }
            }
            NetEvent::Sdk(event) => self.handle_sdk_event(event),
        }
    }

    /// Apply broker tokens to state (and optionally IRC-connect with the web-token).
    fn apply_auth_tokens(&mut self, tokens: AuthTokens, connect: bool) {
        self.state.broker_token = Some(tokens.broker_token.clone());
        self.state.did = Some(tokens.did.clone());
        if !tokens.handle.is_empty() {
            self.state.handle = Some(tokens.handle.clone());
            self.state.form_handle = tokens.handle.clone();
        }
        if !tokens.nick.is_empty() {
            self.state.nick = tokens.nick.clone();
            self.state.form_nick = tokens.nick.clone();
        }
        self.state.error = None;
        self.state.form_callback.clear();
        self.state.persist_session();

        if connect {
            self.do_connect_with_token(Some(tokens.token));
        }
    }

    fn handle_sdk_event(&mut self, event: Event) {
        match event {
            Event::Connected => {
                self.state.connection = ConnectionState::Connected;
                self.state.status_line = "Connected".into();
                self.state.error = None;
            }
            Event::Registered { nick } => {
                // freeq-android: DID user who got Guest nick → stale web-token; refresh.
                if self.state.did.is_some() && nick.starts_with("Guest") {
                    self.state
                        .show_toast("Guest nick after auth — refreshing broker session…");
                    self.net.send(NetCmd::Quit);
                    self.state.connection = ConnectionState::Disconnected;
                    if self.state.has_saved_session() {
                        self.do_reconnect_session();
                    }
                    return;
                }
                self.state.connection = ConnectionState::Registered;
                self.state.nick = nick.clone();
                self.state.form_nick = nick.clone();
                self.state.status_line = format!("Online as {nick}");
                self.state.error = None;
                self.state.show_toast(format!("Connected as {nick}"));
                self.state.persist_session();
                // Seed status buffer
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(format!("Registered as {nick}")));
                // freeq-android: pull DM conversation list so existing threads
                // appear before anyone messages again.
                self.net.send(NetCmd::HistoryTargets { limit: 50 });
            }
            Event::Authenticated { did } => {
                self.state.did = Some(did.clone());
                self.state.show_toast("Authenticated");
                self.state.persist_session();
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(format!("DID {did}")));
            }
            Event::AuthFailed { reason } => {
                self.state.error = Some(format!("Auth failed: {reason}"));
                self.state.show_toast(format!("Auth failed: {reason}"));
            }
            Event::Joined {
                channel,
                nick,
                account: _,
            } => {
                let own = nick.eq_ignore_ascii_case(&self.state.nick);
                let buf = self.state.ensure_buffer(&channel);
                if own {
                    buf.join_pending = false;
                    buf.join_error = None;
                }
                let need_history = own && !buf.has_chat_messages();
                // freeq can deliver more than one JOIN for the same nick
                // (server auto-rejoin racing a client JOIN, ghost attach +
                // explicit join, double-click, etc.). Only announce the first
                // time we see them as a member — otherwise "Joined #chan"
                // shows twice with no real second join.
                let already_member = buf
                    .members
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&nick));
                if !already_member {
                    if own {
                        buf.append(ChatMessage::system(format!("Joined {channel}")));
                        // Stay on tabs; user opens the channel from the list.
                    } else {
                        buf.append(ChatMessage::system(format!("{nick} joined")));
                    }
                    buf.members.push(nick);
                }
                // freeq-android: CHATHISTORY LATEST on own join when empty.
                if need_history {
                    self.net.send(NetCmd::HistoryLatest {
                        target: channel,
                        count: 100,
                    });
                }
            }
            Event::Parted { channel, nick } => {
                if let Some(buf) = self.state.channels.get_mut(&channel) {
                    buf.members.retain(|n| !n.eq_ignore_ascii_case(&nick));
                    buf.append(ChatMessage::system(format!("{nick} left")));
                }
                if nick.eq_ignore_ascii_case(&self.state.nick) {
                    self.state.channels.remove(&channel);
                    self.state.channel_order.retain(|n| n != &channel);
                    if self.state.active_channel.as_deref() == Some(channel.as_str()) {
                        self.state.close_chat();
                    }
                }
            }
            Event::Message {
                from,
                target,
                text,
                tags,
                dm_key,
            } => {
                // CTCP ACTION: \x01ACTION …\x01 (must be str prefixes, not char literals).
                let is_action = text.starts_with("\u{1}ACTION ") && text.ends_with('\u{1}');
                let body = if is_action {
                    text.trim_start_matches("\u{1}ACTION ")
                        .trim_end_matches('\u{1}')
                        .to_string()
                } else {
                    text
                };
                let msgid = tags
                    .get("msgid")
                    .cloned()
                    .unwrap_or_else(|| format!("m-{}", chrono::Utc::now().timestamp_millis()));
                let is_signed = tags.contains_key("+freeq.at/sig")
                    || tags.contains_key("freeq.at/sig")
                    || tags.contains_key("account");
                let reply_to = tags
                    .get("+draft/reply")
                    .or_else(|| tags.get("draft/reply"))
                    .or_else(|| tags.get("+reply"))
                    .or_else(|| tags.get("reply"))
                    .cloned();
                let timestamp = server_time_from_tags(&tags).unwrap_or_else(chrono::Local::now);

                let is_own = from.eq_ignore_ascii_case(&self.state.nick);
                // Own echoed DM: target = recipient nick, dm_key = their DID.
                // Adopt binding first so a nick-opened thread folds into the
                // DID-keyed thread the echo (and peer replies) land in.
                self.state
                    .adopt_echo_binding(is_own, &target, dm_key.as_deref());

                // SDK canonical key (peer DID when known, else nick). Fallback
                // preserves pre-dm_key behavior against older SDKs / guests.
                let buffer_name = if let Some(key) = dm_key {
                    key
                } else if target.starts_with('#') || target.starts_with('&') {
                    target.clone()
                } else if target.eq_ignore_ascii_case(&self.state.nick) {
                    // Incoming DM without dm_key: file under peer nick, then
                    // resolve any known DID binding.
                    self.state.dm_buffer_key(&from)
                } else {
                    // Own send echo without dm_key: file under target peer.
                    self.state.dm_buffer_key(&target)
                };

                let embed = preview::embed_for_message(&body, &tags);
                let link_meta =
                    preview::link_preview_from_tags(&tags).map(|lp| LinkMeta {
                        title: lp.title,
                        description: lp.description,
                        thumb_url: lp.thumb_url,
                        site_name: lp.site_name,
                    });
                // Prefetch embeds as soon as the message arrives (history + live).
                match &embed {
                    Some(preview::Embed::Image { url }) => {
                        self.state.media.touch_image(url);
                    }
                    Some(preview::Embed::Link { url }) => {
                        if let Some(meta) = link_meta.clone() {
                            self.state.media.seed_link(url, meta);
                        } else {
                            self.state.media.touch_link(url);
                        }
                    }
                    None => {}
                }

                let msg = ChatMessage {
                    id: msgid,
                    from: from.clone(),
                    text: body,
                    is_system: false,
                    is_action,
                    is_edited: false,
                    is_deleted: false,
                    timestamp,
                    reply_to,
                    is_signed,
                    embed,
                    link_meta,
                };

                let viewing = self
                    .state
                    .active_channel
                    .as_ref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&buffer_name));

                let buf = self.state.ensure_buffer(&buffer_name);
                buf.append(msg);
                if !viewing && !is_own {
                    buf.unread = buf.unread.saturating_add(1);
                }
            }
            Event::MemberDid { nick, did } => {
                // Fold nick-keyed DM into DID-keyed thread when peer identity
                // is learned (join / whois / account tag on first message).
                self.state.adopt_dm_binding(&nick, &did);
            }
            Event::ChatHistoryTarget {
                nick,
                timestamp,
                partner_did,
            } => {
                // Cold-launch DM list: key by stable partner DID when known.
                if let Some(ref did) = partner_did {
                    self.state.adopt_dm_binding(&nick, did);
                }
                let key = partner_did
                    .clone()
                    .unwrap_or_else(|| self.state.dm_buffer_key(&nick));
                let buf = self.state.ensure_buffer(&key);
                buf.seed_activity_from_target(timestamp.as_deref());
                // Prefer the server's display nick over a raw DID label.
                if freeq_sdk::address::is_did(&key) {
                    self.state.did_to_nick.insert(key, nick);
                }
            }
            Event::TopicChanged {
                channel,
                topic,
                set_by: _,
            } => {
                let buf = self.state.ensure_buffer(&channel);
                buf.topic = topic.clone();
                buf.append(ChatMessage::system(format!("Topic: {topic}")));
            }
            Event::Names { channel, nicks } => {
                let buf = self.state.ensure_buffer(&channel);
                if !buf.names_pending {
                    buf.members.clear();
                    buf.names_pending = true;
                }
                for n in nicks {
                    let clean = n.trim_start_matches(['@', '+', '%', '~', '&']).to_string();
                    if !buf.members.iter().any(|m| m.eq_ignore_ascii_case(&clean)) {
                        buf.members.push(clean);
                    }
                }
            }
            Event::NamesEnd { channel } => {
                if let Some(buf) = self.state.channels.get_mut(&channel) {
                    buf.names_pending = false;
                }
            }
            Event::NickChanged { old_nick, new_nick } => {
                if old_nick.eq_ignore_ascii_case(&self.state.nick) {
                    self.state.nick = new_nick.clone();
                }
                for buf in self.state.channels.values_mut() {
                    for m in &mut buf.members {
                        if m.eq_ignore_ascii_case(&old_nick) {
                            *m = new_nick.clone();
                        }
                    }
                    buf.append(ChatMessage::system(format!(
                        "{old_nick} is now {new_nick}"
                    )));
                }
            }
            Event::Kicked {
                channel,
                nick,
                by,
                reason,
            } => {
                if let Some(buf) = self.state.channels.get_mut(&channel) {
                    buf.members.retain(|n| !n.eq_ignore_ascii_case(&nick));
                    buf.append(ChatMessage::system(format!(
                        "{nick} kicked by {by} ({reason})"
                    )));
                }
                if nick.eq_ignore_ascii_case(&self.state.nick) {
                    self.state.show_toast(format!("Kicked from {channel}"));
                    if self.state.active_channel.as_deref() == Some(channel.as_str()) {
                        self.state.close_chat();
                    }
                }
            }
            Event::ServerNotice { text } => {
                self.state.status_line = text.clone();
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(text.clone()));
                // freeq 477 (guest / policy gate) and other JOIN denials arrive
                // as ServerNotice: "#policytest This channel requires authentication — …"
                if let Some((channel, reason)) = parse_join_denial(&text) {
                    self.apply_join_denial(&channel, &reason);
                }
            }
            Event::Disconnected { reason } => {
                self.state.clear_session();
                if reason != "quit" {
                    self.state.show_toast(format!("Disconnected: {reason}"));
                    self.state.error = Some(reason);
                }
            }
            Event::UserQuit { nick, reason } => {
                for buf in self.state.channels.values_mut() {
                    if buf.members.iter().any(|n| n.eq_ignore_ascii_case(&nick)) {
                        buf.members.retain(|n| !n.eq_ignore_ascii_case(&nick));
                        buf.append(ChatMessage::system(format!("{nick} quit ({reason})")));
                    }
                }
            }
            Event::Invited { channel, by } => {
                self.state.show_toast(format!("Invited to {channel} by {by}"));
            }
            Event::TagMsg {
                from,
                target,
                tags,
                dm_key,
            } => {
                let is_own = from.eq_ignore_ascii_case(&self.state.nick);
                self.state
                    .adopt_echo_binding(is_own, &target, dm_key.as_deref());
                let key = if let Some(k) = dm_key {
                    k
                } else if target.starts_with('#') || target.starts_with('&') {
                    target
                } else if target.eq_ignore_ascii_case(&self.state.nick) {
                    self.state.dm_buffer_key(&from)
                } else {
                    self.state.dm_buffer_key(&target)
                };
                self.handle_av_tags(&key, &tags);
            }
            // batches, history, etc. — not yet rendered
            _ => {}
        }
    }

    /// Process freeq AV TAGMSG tags (av-state, av-token, av-error).
    fn handle_av_tags(&mut self, channel: &str, tags: &std::collections::HashMap<String, String>) {
        // Directed av-token (often target = our nick; freeq may also use the channel).
        if let Some(token) = tags.get("+freeq.at/av-token") {
            let sid = tags
                .get("+freeq.at/av-id")
                .cloned()
                .unwrap_or_default();
            let media_args = self.state.local_call.as_mut().and_then(|lc| {
                if !(sid.is_empty() || lc.session_id.is_empty() || lc.session_id == sid) {
                    return None;
                }
                if !sid.is_empty() {
                    lc.session_id = sid.clone();
                }
                lc.token = Some(token.clone());
                lc.awaiting_start = false;
                Some((
                    lc.channel.clone(),
                    lc.session_id.clone(),
                    lc.instance.clone(),
                    lc.token.clone(),
                ))
            });
            if let Some((ch, session_id, instance, tok)) = media_args {
                self.try_start_av_media(&ch, &session_id, &instance, tok.as_deref());
            }
            return;
        }

        if let Some(code) = tags.get("+freeq.at/av-error") {
            let reason = tags
                .get("+freeq.at/av-reason")
                .cloned()
                .unwrap_or_else(|| code.clone());
            self.state.show_toast(format!("Call error: {reason}"));
            if let Some(lc) = &self.state.local_call {
                if lc.awaiting_start || lc.session_id.is_empty() {
                    self.state.local_call = None;
                    self.state.av_video = None;
                    self.state.av_video_textures.clear();
                }
            }
            return;
        }

        if let Some(st) = freeq_sdk::av::parse_av_state(tags) {
            let buf = self.state.ensure_buffer(channel);
            let ended = av::apply_av_state(&mut buf.call, &st);
            buf.append(ChatMessage::system(av::av_state_message(&st)));

            // Sync local call session id from start/join; maybe dial media.
            let mut dial: Option<(String, String, String, Option<String>)> = None;
            if let Some(lc) = self.state.local_call.as_mut() {
                if lc.channel.eq_ignore_ascii_case(channel) {
                    match st.action {
                        freeq_sdk::av::AvAction::Started if lc.awaiting_start => {
                            lc.session_id = st.session_id.clone();
                            lc.awaiting_start = false;
                            dial = Some((
                                lc.channel.clone(),
                                lc.session_id.clone(),
                                lc.instance.clone(),
                                lc.token.clone(),
                            ));
                        }
                        freeq_sdk::av::AvAction::Ended => {}
                        _ => {
                            if lc.session_id.is_empty() {
                                lc.session_id = st.session_id.clone();
                            }
                        }
                    }
                }
            }
            if let Some((ch, sid, inst, tok)) = dial {
                self.try_start_av_media(&ch, &sid, &inst, tok.as_deref());
            }

            if ended {
                if self
                    .state
                    .local_call
                    .as_ref()
                    .is_some_and(|lc| lc.channel.eq_ignore_ascii_case(channel))
                {
                    self.net.send(NetCmd::AvMediaStop);
                    self.state.local_call = None;
                    self.state.av_video = None;
                    self.state.av_video_textures.clear();
                }
            }
        }
    }

    /// Dial MoQ media (desktop) or mark browser-only (Android).
    fn try_start_av_media(
        &mut self,
        _channel: &str,
        session_id: &str,
        instance: &str,
        token: Option<&str>,
    ) {
        if session_id.is_empty() {
            return;
        }
        let sfu = match av::sfu_moq_url(&self.state.server, token) {
            Ok(u) => u.to_string(),
            Err(e) => {
                if let Some(lc) = self.state.local_call.as_mut() {
                    lc.media = MediaStatus::Failed(e.clone());
                }
                self.state.show_toast(format!("SFU URL: {e}"));
                return;
            }
        };
        if let Some(lc) = self.state.local_call.as_mut() {
            lc.media = MediaStatus::Connecting;
        }
        self.net.send(NetCmd::AvMediaConnect {
            sfu_url: sfu,
            session_id: session_id.to_string(),
            nick: self.state.nick.clone(),
            instance: instance.to_string(),
        });
    }

    fn do_av_start(&mut self, channel: String) {
        if self.state.local_call.is_some() {
            self.state.show_toast("Already in a call — leave it first");
            return;
        }
        // Claim the single call slot immediately so a second press cannot race.
        self.state.local_call = Some(LocalCall {
            channel: channel.clone(),
            session_id: String::new(),
            instance: String::new(),
            token: None,
            muted: false,
            camera: false,
            has_camera: false,
            media: MediaStatus::Idle,
            awaiting_start: true,
        });
        self.net.send(NetCmd::AvStart { channel });
    }

    fn do_av_join(&mut self, channel: String, session_id: String) {
        if self.state.local_call.is_some() {
            self.state.show_toast("Already in a call — leave it first");
            return;
        }
        self.state.local_call = Some(LocalCall {
            channel: channel.clone(),
            session_id: session_id.clone(),
            instance: String::new(),
            token: None,
            muted: false,
            camera: false,
            has_camera: false,
            media: MediaStatus::Idle,
            awaiting_start: false,
        });
        self.net.send(NetCmd::AvJoin {
            channel,
            session_id,
        });
    }

    fn do_av_leave(&mut self) {
        let Some(lc) = self.state.local_call.take() else {
            return;
        };
        self.state.av_video = None;
        self.state.av_video_textures.clear();
        self.net.send(NetCmd::AvLeave {
            channel: lc.channel,
            session_id: lc.session_id,
            instance: lc.instance,
        });
        self.net.send(NetCmd::AvMediaStop);
    }

    fn do_av_toggle_mute(&mut self) {
        let Some(lc) = self.state.local_call.as_mut() else {
            return;
        };
        lc.muted = !lc.muted;
        self.net.send(NetCmd::AvMute { muted: lc.muted });
    }

    fn do_av_toggle_camera(&mut self) {
        let Some(lc) = self.state.local_call.as_mut() else {
            return;
        };
        if !lc.has_camera {
            self.state.show_toast("No camera available");
            return;
        }
        lc.camera = !lc.camera;
        self.net.send(NetCmd::AvCamera {
            enabled: lc.camera,
        });
    }

    fn do_connect(&mut self) {
        // Guest path — no SASL web-token. Wipe any leftover Bluesky session so
        // we don't stay half-authenticated (DID/handle in UI, guest on wire).
        if self.state.has_cached_identity() {
            let prior = self
                .state
                .handle
                .clone()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| self.state.nick.clone());
            self.state.clear_auth();
            // Don't try to register as the old handle without SASL (server → Guest*).
            if !prior.is_empty() && self.state.form_nick.eq_ignore_ascii_case(&prior) {
                let nick = crate::state::default_nick();
                self.state.form_nick = nick.clone();
                self.state.nick = nick;
            }
            self.state
                .show_toast("Cleared saved account for guest connect");
        }
        self.do_connect_with_token(None);
    }

    fn do_connect_with_token(&mut self, web_token: Option<String>) {
        let nick = self.state.form_nick.trim().to_string();
        let server = self.state.form_server.trim().to_string();
        if nick.is_empty() || server.is_empty() {
            self.state.error = Some("Nick and server are required".into());
            return;
        }
        self.state.error = None;
        self.state.connection = ConnectionState::Connecting;
        self.state.awaiting_oauth = false;
        self.state.nick = nick.clone();
        self.state.server = server.clone();
        self.state.use_tls = self.state.form_tls;
        self.state.use_websocket = self.state.form_websocket;
        self.state.status_line = "Connecting…".into();

        // Match freeq-android: default lobby is #general (public). Do not force
        // #freeq — it can be policy-gated and guests then land on a join-denied
        // error screen for a channel they never chose.
        let auto_join = vec!["#general".into()];

        self.net.send(NetCmd::Connect {
            nick,
            server,
            tls: self.state.form_tls,
            websocket: self.state.form_websocket,
            auto_join,
            web_token,
        });
    }

    fn do_bluesky_login(&mut self) {
        let handle = self
            .state
            .form_handle
            .trim()
            .trim_start_matches('@')
            .to_string();
        if handle.is_empty() {
            self.state.error = Some("Enter your Bluesky handle".into());
            return;
        }
        self.state.error = None;
        self.state.awaiting_oauth = true;
        self.state.connection = ConnectionState::Connecting;
        self.state.status_line = "Waiting for browser sign-in…".into();
        self.net.send(NetCmd::BlueskyLogin {
            handle,
            auth_broker: self.state.auth_broker.clone(),
        });
    }

    fn do_apply_callback(&mut self) {
        let raw = self.state.form_callback.trim().to_string();
        if raw.is_empty() {
            self.state.error = Some("Paste a freeq://auth?… link from the browser".into());
            return;
        }
        match auth::parse_freeq_auth_url(&raw) {
            Ok(tokens) => {
                self.state.awaiting_oauth = false;
                self.apply_auth_tokens(tokens, /*connect=*/ true);
            }
            Err(e) => {
                self.state.error = Some(format!("Invalid callback: {e}"));
            }
        }
    }

    fn do_reconnect_session(&mut self) {
        let Some(broker_token) = self.state.broker_token.clone() else {
            self.state.error = Some("No saved session".into());
            return;
        };
        self.state.error = None;
        self.state.connection = ConnectionState::Connecting;
        self.state.status_line = "Restoring session…".into();
        self.net.send(NetCmd::ReconnectSession {
            broker_token,
            auth_broker: self.state.auth_broker.clone(),
        });
    }

    fn do_join(&mut self, channel: String) {
        let ch = AppState::normalize_channel(&channel);
        if ch.is_empty() {
            return;
        }
        {
            let buf = self.state.ensure_buffer(&ch);
            buf.join_pending = true;
            buf.join_error = None;
        }
        self.net.send(NetCmd::Join(ch.clone()));
        self.state.open_chat(&ch);
        self.state.tab = Tab::Chats;
    }

    /// Apply a server join denial (477 guest/policy gate, invite-only, ban, …).
    fn apply_join_denial(&mut self, channel: &str, reason: &str) {
        let ch = AppState::normalize_channel(channel);
        if ch.is_empty() {
            return;
        }
        let reason = reason.trim();
        let reason = if reason.is_empty() {
            "You are not allowed to join this channel".to_string()
        } else {
            reason.to_string()
        };

        // User-initiated joins set join_pending and usually open the chat first.
        // Background auto-joins do neither — don't dump the user into a
        // "Guests can't join" screen for a channel they never tapped.
        let user_initiated = self
            .state
            .channels
            .get(&ch)
            .is_some_and(|b| b.join_pending)
            || self.state.active_channel.as_deref() == Some(ch.as_str());

        if !user_initiated {
            self.state.channels.remove(&ch);
            self.state.channel_order.retain(|n| n != &ch);
            self.state.show_toast(format!("{ch}: {reason}"));
            return;
        }

        {
            let buf = self.state.ensure_buffer(&ch);
            buf.join_pending = false;
            buf.join_error = Some(reason.clone());
            buf.append(ChatMessage::system(reason.clone()));
        }
        self.state.error = Some(format!("{ch}: {reason}"));
        self.state.show_toast(format!("{ch}: {reason}"));
        // Keep the chat open so the error empty-state is visible.
        if self.state.active_channel.as_deref() != Some(ch.as_str()) {
            self.state.open_chat(&ch);
        }
        self.state.tab = Tab::Chats;
    }

    fn do_send(&mut self, target: String, text: String) {
        // Image attached: upload then PRIVMSG the freeq media URL.
        if self.state.compose_image.is_some() {
            self.do_send_with_image(target, text);
            return;
        }

        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.state.compose.clear();
        self.do_send_local_echo(&target, text.clone());
        self.net.send(NetCmd::Privmsg { target, text });
    }

    fn do_send_with_image(&mut self, target: String, caption: String) {
        if self.state.compose_uploading {
            return;
        }
        let Some(img) = self.state.compose_image.clone() else {
            return;
        };
        let Some(did) = self.state.did.clone() else {
            self.state
                .show_toast("Sign in with Bluesky to send images");
            return;
        };
        if self.state.connection != ConnectionState::Registered
            && self.state.connection != ConnectionState::Connected
        {
            self.state.show_toast("Connect before sending images");
            return;
        }

        let bytes = match clipboard::encode_png(&img) {
            Ok(b) => b,
            Err(e) => {
                self.state.show_toast(e);
                return;
            }
        };

        self.state.compose_uploading = true;
        self.state.show_toast("Uploading image…");
        self.net.send(NetCmd::UploadAndSend {
            target,
            caption,
            bytes,
            content_type: "image/png".into(),
            did,
            api_base: api_base_for_server(&self.state.server),
        });
    }

    fn do_send_local_echo(&mut self, target: &str, text: String) {
        // Optimistic local echo for snappy UI. When the server echoes the
        // PRIVMSG back (echo-message), Buffer::append drops this local-* row
        // and keeps the real msgid / signed copy — so the user never sees
        // the line twice.
        //
        // File under the canonical DM key (DID when known) so the echo lands
        // in the same thread as the server echo and peer replies.
        let buffer_key = self.state.dm_buffer_key(target);
        let embed = preview::embed_from_text(&text);
        if let Some(preview::Embed::Image { ref url }) = embed {
            self.state.media.touch_image(url);
        } else if let Some(preview::Embed::Link { ref url }) = embed {
            self.state.media.touch_link(url);
        }
        let msg = ChatMessage {
            id: format!("local-{}", chrono::Utc::now().timestamp_millis()),
            from: self.state.nick.clone(),
            text,
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: chrono::Local::now(),
            reply_to: None,
            is_signed: false,
            embed,
            link_meta: None,
        };
        let buf = self.state.ensure_buffer(&buffer_key);
        buf.append(msg);
    }
}

/// Parse IRCv3 `time` / `server-time` tag into local wall-clock.
fn server_time_from_tags(
    tags: &std::collections::HashMap<String, String>,
) -> Option<chrono::DateTime<chrono::Local>> {
    let raw = tags.get("time").or_else(|| tags.get("server-time"))?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local))
}

/// Lift all UI above the soft keyboard while a text field has focus.
///
/// Soft keyboards on phones are typically ~35–45% of portrait height (Pixel
/// logcat showed ~988px IME on a 2400px display). We only reserve while
/// `wants_keyboard_input` so dismissing the keyboard restores full layout.
fn reserve_ime_chrome(ctx: &egui::Context, th: &Theme) {
    #[cfg(target_os = "android")]
    {
        if !ctx.wants_keyboard_input() {
            return;
        }
        let h = ctx.screen_rect().height();
        // Total clearance ≈ 40% of screen (typical soft keyboard). Subtract
        // vidya's fixed nav chrome (~48) so we don't double-pad.
        let nav_chrome = 48.0_f32;
        let total = (h * 0.40).clamp(240.0, h * 0.52);
        let ime_h = (total - nav_chrome).max(0.0);
        if ime_h < 1.0 {
            return;
        }
        let fill = th.palette.headerbar_bg;
        egui::TopBottomPanel::bottom("sleek_ime_chrome")
            .exact_height(ime_h)
            .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::ZERO))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            });
        ctx.request_repaint();
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (ctx, th);
    }
}

impl eframe::App for SleekApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_net(ctx);
        self.poll_file_pick(ctx);

        let th = self.theme();
        let p = &th.palette;
        let sp = &th.spacing;
        let screen = ctx.screen_rect();
        let phone = screen.height() >= screen.width() || screen.width() < 640.0;

        reserve_system_chrome(ctx, &th);
        // NativeActivity rarely resizes the GL surface for the soft keyboard
        // (adjustResize is a no-op for the surface). When a text field is
        // focused, reserve extra bottom space so compose / forms sit above IME.
        reserve_ime_chrome(ctx, &th);

        let connected = self.state.connection == ConnectionState::Registered
            || self.state.connection == ConnectionState::Connected
            || self.state.connection == ConnectionState::Connecting;

        // ── Toast (auto-dismiss after a few seconds) ───────────────
        if self.state.tick_toast() {
            if let Some(until) = self.state.toast_until {
                ctx.request_repaint_after(until.saturating_duration_since(std::time::Instant::now()));
            }
            if let Some(msg) = self.state.toast.clone() {
                egui::TopBottomPanel::bottom("toast")
                    .frame(
                        egui::Frame::new()
                            .fill(p.accent.gamma_multiply(0.22))
                            .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8))
                            .stroke(Stroke::new(1.0_f32, p.accent.gamma_multiply(0.55))),
                    )
                    .show_separator_line(false)
                    .show(ctx, |ui| {
                        // RTL so Dismiss is allocated first on the right; message
                        // wraps in the remaining width and never sits under the button.
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if button(ui, &th, "Dismiss").clicked() {
                                self.state.clear_toast();
                            }
                            ui.add_space(sp.sm);
                            body(ui, &th, &msg);
                        });
                    });
            }
        }

        // ── Active MoQ call (always visible while open; one at a time) ─
        if self.state.local_call.is_some() {
            egui::TopBottomPanel::top("av_moq_global")
                .frame(
                    egui::Frame::new()
                        .fill(p.window_bg)
                        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8))
                        .stroke(Stroke::new(1.0_f32, p.border_soft)),
                )
                .show_separator_line(false)
                .show(ctx, |ui| {
                    // Cap width on wide desktop so tiles don't stretch endlessly.
                    let max_w = if phone {
                        ui.available_width()
                    } else {
                        ui.available_width().min(480.0)
                    };
                    ui.set_max_width(max_w);
                    ui.set_min_width(max_w.min(ui.available_width()));
                    if let Some(act) = ui::active_call_panel(ui, &th, &mut self.state) {
                        match act {
                            ChatAction::AvLeave => self.do_av_leave(),
                            ChatAction::AvToggleMute => self.do_av_toggle_mute(),
                            ChatAction::AvToggleCamera => self.do_av_toggle_camera(),
                            ChatAction::OpenCallChannel(ch) => {
                                self.state.open_chat(&ch);
                            }
                            _ => {}
                        }
                    }
                });
        }

        // ── Bottom tabs (connected, not in chat detail) ────────────
        // Hide while the soft keyboard is up so the compose / focused field
        // has room above the IME (tabs would otherwise sit under the keys).
        let show_tabs = connected
            && matches!(self.state.route, Route::Tabs)
            && self.state.connection == ConnectionState::Registered
            && !ctx.wants_keyboard_input();

        if show_tabs {
            egui::TopBottomPanel::bottom("nav_bottom")
                .frame(
                    egui::Frame::new()
                        .fill(p.headerbar_bg)
                        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8))
                        .stroke(Stroke::new(1.0_f32, p.border_soft)),
                )
                .show_separator_line(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = sp.sm;
                        let total_unread = self.state.total_unread();
                        for tab in Tab::ALL {
                            let selected = self.state.tab == tab;
                            let fill = if selected { p.accent } else { p.button_bg };
                            let fg = if selected {
                                p.accent_fg
                            } else {
                                p.button_fg
                            };
                            let mut label = tab.short().to_string();
                            if tab == Tab::Chats && total_unread > 0 {
                                label = format!("{label} ({})", total_unread.min(99));
                            }
                            let text = RichText::new(label)
                                .size(th.type_scale.caption)
                                .color(fg);
                            let btn = egui::Button::new(text)
                                .fill(fill)
                                .stroke(if selected {
                                    Stroke::NONE
                                } else {
                                    Stroke::new(1.0_f32, p.border_soft)
                                })
                                .corner_radius(sp.radius_md)
                                .min_size(Vec2::new(72.0, sp.control_height));
                            if ui.add(btn).clicked() {
                                self.state.tab = tab;
                            }
                        }
                    });
                });
        }

        // ── Header (compact when in chat) ──────────────────────────
        if !matches!(self.state.route, Route::Chat(_)) {
            let header_frame = th.header_frame().inner_margin(egui::Margin {
                left: sp.page as i8,
                right: sp.page as i8,
                top: (sp.md + 4.0) as i8,
                bottom: (sp.md + 2.0) as i8,
            });
            egui::TopBottomPanel::top("header")
                .frame(header_frame)
                .show_separator_line(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            title(ui, &th, "Sleek");
                            // Subtitle only when connected or mid-connect — idle connect
                            // already has its own form; avoid stacking the same chrome.
                            if connected || self.state.connection == ConnectionState::Connecting {
                                ui.add_space(sp.xs + 2.0);
                                let blurb = if self.state.connection == ConnectionState::Registered
                                {
                                    format!("{} · {}", self.state.nick, self.state.server)
                                } else {
                                    "Connecting…".into()
                                };
                                dim_label(ui, &th, &blurb);
                            }
                        });
                    });
                });
        }

        // ── Main content ───────────────────────────────────────────
        let page = if phone {
            egui::Frame::new()
                .fill(p.window_bg)
                .inner_margin(egui::Margin::symmetric(10_i8, sp.page as i8))
        } else {
            th.page_frame()
        };

        egui::CentralPanel::default().frame(page).show(ctx, |ui| {
            let col_w = if phone {
                ui.available_width()
            } else {
                ui.available_width().min(480.0)
            };
            ui.set_max_width(col_w);
            ui.set_min_width(col_w);

            // Chat owns its own message ScrollArea with fixed header (← / Leave)
            // and compose bar. Nesting it in main_scroll let those controls scroll away.
            if let Route::Chat(ch) = self.state.route.clone() {
                match ui::chat_screen(ui, &th, &mut self.state, &ch) {
                    ChatAction::None => {}
                    ChatAction::Back => {
                        self.state.close_chat();
                    }
                    ChatAction::Send { target, text } => {
                        self.do_send(target, text);
                    }
                    ChatAction::Part(channel) => {
                        // Only PART the server if we actually joined; denied
                        // joins (guest on #policytest, etc.) are local-only.
                        let need_part = self
                            .state
                            .channels
                            .get(&channel)
                            .is_some_and(|b| b.is_joined());
                        if need_part {
                            self.net.send(NetCmd::Part(channel.clone()));
                        }
                        self.state.channels.remove(&channel);
                        self.state.channel_order.retain(|n| n != &channel);
                        self.state.close_chat();
                    }
                    ChatAction::OpenDm(nick) => {
                        let key = self.state.dm_buffer_key(&nick);
                        let need_history = self
                            .state
                            .channels
                            .get(&key)
                            .map(|b| !b.has_chat_messages())
                            .unwrap_or(true);
                        self.state.open_chat(&key);
                        if need_history {
                            self.net.send(NetCmd::HistoryLatest {
                                target: key,
                                count: 100,
                            });
                        }
                    }
                    ChatAction::AvStart(channel) => {
                        self.do_av_start(channel);
                    }
                    ChatAction::AvJoin {
                        channel,
                        session_id,
                    } => {
                        self.do_av_join(channel, session_id);
                    }
                    ChatAction::AvLeave => {
                        self.do_av_leave();
                    }
                    ChatAction::AvToggleMute => {
                        self.do_av_toggle_mute();
                    }
                    ChatAction::AvToggleCamera => {
                        self.do_av_toggle_camera();
                    }
                    ChatAction::OpenCallChannel(ch) => {
                        self.state.open_chat(&ch);
                    }
                }
                return;
            }

            let panel_h = ui.available_height();
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(panel_h)
                .id_salt("main_scroll")
                .show(ui, |ui| {
                    ui.set_max_width(col_w);
                    ui.set_min_width(col_w);

                    // Route::Chat is rendered above without this outer scroll.
                    if self.state.connection == ConnectionState::Registered {
                        match self.state.tab {
                            Tab::Chats => match ui::chats_tab(ui, &th, &mut self.state) {
                                ChatsAction::None => {}
                                ChatsAction::Open(name) => {
                                    let need_history = self
                                        .state
                                        .channels
                                        .get(&name)
                                        .map(|b| !b.has_chat_messages())
                                        .unwrap_or(true);
                                    self.state.open_chat(&name);
                                    if need_history {
                                        self.net.send(NetCmd::HistoryLatest {
                                            target: name,
                                            count: 100,
                                        });
                                    }
                                }
                                ChatsAction::Join(ch) => self.do_join(ch),
                            },
                            Tab::Discover => {
                                match ui::discover_tab(ui, &th, &mut self.state) {
                                    DiscoverAction::None => {}
                                    DiscoverAction::Join(ch) => self.do_join(ch),
                                }
                            }
                            Tab::Settings => {
                                match ui::settings_tab(ui, &th, &mut self.state, self.mode) {
                                    SettingsAction::None => {}
                                    SettingsAction::Disconnect => {
                                        self.net.send(NetCmd::Quit);
                                        self.state.clear_session();
                                    }
                                    SettingsAction::Logout => {
                                        // Full clear: IRC + disk session → real guest next time.
                                        self.net.send(NetCmd::Quit);
                                        self.state.logout();
                                    }
                                    SettingsAction::ToggleTheme => {
                                        let next = match self.mode {
                                            Mode::Dark => Mode::Light,
                                            Mode::Light => Mode::Dark,
                                        };
                                        self.set_mode(ctx, next);
                                    }
                                }
                            }
                        }
                    } else if self.state.connection == ConnectionState::Connecting
                        || self.state.connection == ConnectionState::Connected
                    {
                        // Waiting for registration
                        ui.vertical_centered(|ui| {
                            ui.add_space(sp.xl * 2.0);
                            title(ui, &th, "Connecting");
                            ui.add_space(sp.md);
                            dim_label(ui, &th, &self.state.status_line);
                            ui.add_space(sp.lg);
                            if button(ui, &th, "Cancel").clicked() {
                                self.net.send(NetCmd::Quit);
                                self.state.clear_session();
                            }
                        });
                    } else {
                        match ui::connect_screen(ui, &th, &mut self.state) {
                            ConnectAction::None => {}
                            ConnectAction::ConnectGuest => self.do_connect(),
                            ConnectAction::BlueskyLogin => self.do_bluesky_login(),
                            ConnectAction::ApplyCallback => self.do_apply_callback(),
                            ConnectAction::ReconnectSession => self.do_reconnect_session(),
                            ConnectAction::ClearAccount => {
                                self.net.send(NetCmd::Quit);
                                self.state.logout();
                            }
                        }
                    }

                    if phone {
                        ui.add_space(sp.xl + sp.lg);
                    }
                });
        });

    }
}

/// Parse freeq/IRC join-denial notices into `(channel, reason)`.
///
/// freeq-sdk emits error numerics as `ServerNotice` with text like:
/// `#policytest This channel requires authentication — sign in to join`
/// (ERR 477 for guest/policy-gated channels).
fn parse_join_denial(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // IRCv3 FAIL JOIN <code> <channel> [:reason]
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() >= 3
        && (tokens[0].eq_ignore_ascii_case("FAIL") || tokens[0].eq_ignore_ascii_case("JOIN"))
    {
        let start = if tokens[0].eq_ignore_ascii_case("FAIL")
            && tokens.get(1).is_some_and(|t| t.eq_ignore_ascii_case("JOIN"))
        {
            2
        } else if tokens[0].eq_ignore_ascii_case("JOIN") {
            1
        } else {
            0
        };
        if start > 0 {
            // Skip optional code token; find channel-shaped arg.
            for (i, t) in tokens.iter().enumerate().skip(start) {
                if t.starts_with('#') || t.starts_with('&') {
                    let channel = (*t).to_string();
                    let rest = tokens[i + 1..]
                        .join(" ")
                        .trim_start_matches(':')
                        .trim()
                        .to_string();
                    let reason = if rest.is_empty() {
                        "Join denied".into()
                    } else {
                        rest
                    };
                    return Some((channel, reason));
                }
            }
        }
    }

    // Numeric / freeq form: "#channel reason…"
    let (channel, rest) = text.split_once(char::is_whitespace)?;
    if !(channel.starts_with('#') || channel.starts_with('&')) {
        return None;
    }
    let reason = rest.trim();
    if reason.is_empty() {
        return None;
    }
    let lower = reason.to_ascii_lowercase();
    // Only treat channel-first notices that look like join denials — avoid
    // swallowing unrelated server notices that happen to mention a channel.
    let looks_like_denial = [
        "authentication",
        "sign in",
        "policy",
        "invite",
        "banned",
        "cannot join",
        "can't join",
        "not allowed",
        "need ",
        "requires",
        "registered",
        "full",
        "key",
        "password",
        "mode +",
        "not on channel",
        "no such channel",
        "illegal channel",
    ]
    .iter()
    .any(|k| lower.contains(k));

    if looks_like_denial {
        Some((channel.to_string(), reason.to_string()))
    } else {
        None
    }
}
