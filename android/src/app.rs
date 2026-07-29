//! Sleek eframe app — desktop + Android.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Stroke, Vec2};
use freeq_sdk::event::Event;
use vidya::{apply, body, button, dim_label, reserve_system_chrome, title, Mode, Theme};

use crate::auth::{self, AuthTokens};
use crate::net::{NetBridge, NetCmd, NetEvent};
use crate::state::{
    AppState, ChatMessage, ConnectionState, Route, Tab,
};
use crate::ui::{
    self, ChatAction, ChatsAction, ConnectAction, DiscoverAction, SettingsAction,
};

/// Desktop / host entry.
pub fn run_desktop() -> eframe::Result {
    #[cfg(not(target_os = "android"))]
    {
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
        if app.state.has_saved_session() {
            app.do_reconnect_session();
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
        // Keep UI live while connecting / connected so events paint promptly.
        if self.state.connection == ConnectionState::Connecting
            || self.state.connection.is_live()
            || self.state.awaiting_oauth
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn handle_net_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Status(s) => {
                self.state.status_line = s;
            }
            NetEvent::Failed(s) => {
                self.state.error = Some(s.clone());
                self.state.toast = Some(s);
                self.state.awaiting_oauth = false;
                if self.state.connection == ConnectionState::Connecting {
                    self.state.connection = ConnectionState::Disconnected;
                }
            }
            NetEvent::AuthReady(tokens) => {
                self.state.awaiting_oauth = false;
                self.apply_auth_tokens(tokens, /*connect=*/ true);
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
                    self.state.toast =
                        Some("Guest nick after auth — refreshing broker session…".into());
                    self.net.send(NetCmd::Quit);
                    self.state.connection = ConnectionState::Disconnected;
                    if self.state.has_saved_session() {
                        self.do_reconnect_session();
                    }
                    return;
                }
                self.state.connection = ConnectionState::Registered;
                self.state.nick = nick.clone();
                self.state.status_line = format!("Online as {nick}");
                self.state.error = None;
                self.state.toast = Some(format!("Connected as {nick}"));
                self.state.persist_session();
                // Seed status buffer
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(format!("Registered as {nick}")));
            }
            Event::Authenticated { did } => {
                self.state.did = Some(did.clone());
                self.state.toast = Some("Authenticated".into());
                self.state.persist_session();
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(format!("DID {did}")));
            }
            Event::AuthFailed { reason } => {
                self.state.error = Some(format!("Auth failed: {reason}"));
                self.state.toast = Some(format!("Auth failed: {reason}"));
            }
            Event::Joined {
                channel,
                nick,
                account: _,
            } => {
                let own = nick.eq_ignore_ascii_case(&self.state.nick);
                let buf = self.state.ensure_buffer(&channel);
                if own {
                    buf.append(ChatMessage::system(format!("Joined {channel}")));
                    // Stay on tabs; user opens the channel from the list.
                } else {
                    buf.append(ChatMessage::system(format!("{nick} joined")));
                }
                if !buf.members.iter().any(|n| n.eq_ignore_ascii_case(&nick)) {
                    buf.members.push(nick);
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
                let reply_to = tags.get("+draft/reply").or_else(|| tags.get("draft/reply")).cloned();

                let buffer_name = if let Some(key) = dm_key {
                    key
                } else if target.starts_with('#') || target.starts_with('&') {
                    target.clone()
                } else if target.eq_ignore_ascii_case(&self.state.nick) {
                    from.clone()
                } else {
                    target.clone()
                };

                let msg = ChatMessage {
                    id: msgid,
                    from: from.clone(),
                    text: body,
                    is_system: false,
                    is_action,
                    is_edited: false,
                    is_deleted: false,
                    timestamp: chrono::Local::now(),
                    reply_to,
                    is_signed,
                };

                let viewing = self
                    .state
                    .active_channel
                    .as_ref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&buffer_name));
                let is_own = from.eq_ignore_ascii_case(&self.state.nick);

                let buf = self.state.ensure_buffer(&buffer_name);
                buf.append(msg);
                if !viewing && !is_own {
                    buf.unread = buf.unread.saturating_add(1);
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
                    self.state.toast = Some(format!("Kicked from {channel}"));
                    if self.state.active_channel.as_deref() == Some(channel.as_str()) {
                        self.state.close_chat();
                    }
                }
            }
            Event::ServerNotice { text } => {
                self.state.status_line = text.clone();
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(text));
            }
            Event::Disconnected { reason } => {
                self.state.clear_session();
                if reason != "quit" {
                    self.state.toast = Some(format!("Disconnected: {reason}"));
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
                self.state.toast = Some(format!("Invited to {channel} by {by}"));
            }
            // TAGMSG, batches, history, etc. — ignored for v0.1
            _ => {}
        }
    }

    fn do_connect(&mut self) {
        // Guest path — no SASL web-token.
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

        let auto_join = vec!["#freeq".into()];

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
        self.state.ensure_buffer(&ch);
        self.net.send(NetCmd::Join(ch.clone()));
        self.state.open_chat(&ch);
        self.state.tab = Tab::Chats;
    }

    fn do_send(&mut self, target: String, text: String) {
        // Optimistic local echo for snappy UI
        let msg = ChatMessage {
            id: format!("local-{}", chrono::Utc::now().timestamp_millis()),
            from: self.state.nick.clone(),
            text: text.clone(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: chrono::Local::now(),
            reply_to: None,
            is_signed: false,
        };
        let buf = self.state.ensure_buffer(&target);
        buf.append(msg);
        self.net.send(NetCmd::Privmsg { target, text });
    }
}

impl eframe::App for SleekApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_net(ctx);

        let th = self.theme();
        let p = &th.palette;
        let sp = &th.spacing;
        let screen = ctx.screen_rect();
        let phone = screen.height() >= screen.width() || screen.width() < 640.0;

        reserve_system_chrome(ctx, &th);

        let connected = self.state.connection == ConnectionState::Registered
            || self.state.connection == ConnectionState::Connected
            || self.state.connection == ConnectionState::Connecting;

        // ── Toast ──────────────────────────────────────────────────
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
                    ui.horizontal(|ui| {
                        ui.set_max_width(ui.available_width() - 100.0);
                        body(ui, &th, &msg);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if button(ui, &th, "Dismiss").clicked() {
                                self.state.toast = None;
                            }
                        });
                    });
                });
        }

        // ── Bottom tabs (connected, not in chat detail) ────────────
        let show_tabs = connected
            && matches!(self.state.route, Route::Tabs)
            && self.state.connection == ConnectionState::Registered;

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
                            ui.add_space(sp.xs + 2.0);
                            let blurb = if self.state.connection == ConnectionState::Registered {
                                format!("{} · {}", self.state.nick, self.state.server)
                            } else if self.state.connection == ConnectionState::Connecting {
                                "Connecting…".into()
                            } else if self.state.has_saved_session() {
                                "freeq · saved session".into()
                            } else {
                                "freeq · sign in or guest".into()
                            };
                            dim_label(ui, &th, &blurb);
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !connected {
                                let is_dark = self.mode == Mode::Dark;
                                let label = if is_dark { " Light " } else { " Dark " };
                                if button(ui, &th, label).clicked() {
                                    let next = if is_dark { Mode::Light } else { Mode::Dark };
                                    self.set_mode(ctx, next);
                                }
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
            let panel_h = ui.available_height();
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(panel_h)
                .id_salt("main_scroll")
                .show(ui, |ui| {
                    let col_w = if phone {
                        ui.available_width()
                    } else {
                        ui.available_width().min(480.0)
                    };
                    ui.set_max_width(col_w);
                    ui.set_min_width(col_w);

                    match self.state.route.clone() {
                        Route::Chat(ch) => {
                            // Chat manages its own scroll; don't nest heavily.
                            // Use a non-scrolling parent: disable outer by filling.
                            match ui::chat_screen(ui, &th, &mut self.state, &ch) {
                                ChatAction::None => {}
                                ChatAction::Back => {
                                    self.state.close_chat();
                                }
                                ChatAction::Send { target, text } => {
                                    self.do_send(target, text);
                                }
                                ChatAction::Part(channel) => {
                                    self.net.send(NetCmd::Part(channel.clone()));
                                    self.state.channels.remove(&channel);
                                    self.state.channel_order.retain(|n| n != &channel);
                                    self.state.close_chat();
                                }
                            }
                        }
                        Route::Tabs => {
                            if self.state.connection == ConnectionState::Registered {
                                match self.state.tab {
                                    Tab::Chats => match ui::chats_tab(ui, &th, &mut self.state) {
                                        ChatsAction::None => {}
                                        ChatsAction::Open(name) => {
                                            self.state.open_chat(&name);
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
                                }
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
