//! Sleek eframe app — desktop + Android.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Stroke, Vec2};
use freeq_sdk::event::Event;
use vidya::{apply, body, button, dim_label, reserve_system_chrome, title, Mode, Theme};
#[cfg(target_os = "android")]
use vidya::set_system_chrome;

use crate::auth::{self, AuthTokens};
use crate::av::{self, LocalCall, MediaStatus};
use crate::clipboard;
use crate::net::{NetBridge, NetCmd, NetEvent};
use crate::preview;
use crate::state::{
    api_base_for_server, parse_reactions_tag, AppState, CachedPixels, ChatMessage, ConnectionState,
    LinkMeta, MediaFetch, Route, Tab,
};
use crate::ui::{
    self, image_lightbox_overlay, ChatAction, ChatsAction, ConnectAction, DiscoverAction,
    SettingsAction,
};

/// Phone-shaped default for local desktop; on Codespaces / noVNC fill the
/// X display so the app matches the browser viewport (not a 390×780 island).
/// Override: `SLEEK_FIT_VIEWPORT=1` always fit; `=0` force phone size.
/// Optional exact size: `SLEEK_VIEWPORT=1440x768`.
fn fit_viewport_enabled() -> bool {
    match std::env::var("SLEEK_FIT_VIEWPORT").as_deref() {
        Ok("1") | Ok("true") | Ok("yes") => true,
        Ok("0") | Ok("false") | Ok("no") => false,
        _ => std::env::var_os("SLEEK_CODESPACE").is_some(),
    }
}

/// Best-effort screen size in points (Codespace VNC / X11 when maximize is ignored).
fn detect_screen_size() -> Option<egui::Vec2> {
    if let Ok(s) = std::env::var("SLEEK_VIEWPORT") {
        let lower = s.to_lowercase();
        let mut parts = lower.split('x');
        let w: f32 = parts.next()?.trim().parse().ok()?;
        let h: f32 = parts.next()?.trim().parse().ok()?;
        if w > 0.0 && h > 0.0 {
            return Some(egui::vec2(w, h));
        }
    }
    // desktop-lite / TigerVNC ships xdpyinfo; dimensions: 1440x768 pixels …
    let out = std::process::Command::new("xdpyinfo").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("dimensions:") else {
            continue;
        };
        let dim = rest.trim().split_whitespace().next()?;
        let mut parts = dim.split('x');
        let w: f32 = parts.next()?.parse().ok()?;
        let h: f32 = parts.next()?.parse().ok()?;
        if w > 0.0 && h > 0.0 {
            return Some(egui::vec2(w, h));
        }
    }
    None
}

/// Freedesktop / Wayland app id — must match `StartupWMClass` in the .desktop file.
const APP_ID: &str = "uk.nandi.sleek";

/// Window / taskbar icon (RGBA). Path relative to the `sleek` crate root (`android/`).
fn load_app_icon() -> Option<egui::IconData> {
    // Built into the binary so the dock/titlebar works without icon-theme lookup.
    let bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../assets/icons/uk.nandi.sleek-256.png"
    ));
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

/// Desktop / host entry.
pub fn run_desktop() -> eframe::Result {
    #[cfg(not(target_os = "android"))]
    {
        // Default: app at info; moq_lite at error (see `default_log_filter`).
        // Clipboard miss noise stays at debug (vendored egui-winit).
        // RUST_LOG overrides the whole default when set.
        let _ = env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or(crate::default_log_filter()),
        )
        .try_init();
    }
    let fit_viewport = fit_viewport_enabled();
    // app_id: niri/Noctalia match the window to uk.nandi.sleek.desktop.
    // with_icon: taskbar/titlebar even when the theme lookup fails.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Sleek")
        .with_app_id(APP_ID);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    if fit_viewport {
        // Fluxbox on desktop-lite often ignores `maximized`; size to the X
        // screen (or fullscreen) so noVNC shows a full-viewport app.
        if let Some(size) = detect_screen_size() {
            viewport = viewport
                .with_inner_size(size)
                .with_position(egui::pos2(0.0, 0.0));
        }
        // Maximized (not fullscreen/always-on-top) so OAuth Chromium can
        // appear above Sleek on desktop-lite / Fluxbox VNC.
        viewport = viewport
            .with_maximized(true)
            .with_min_inner_size([320.0, 400.0]);
    } else {
        viewport = viewport
            .with_inner_size([390.0, 780.0])
            .with_min_inner_size([320.0, 560.0]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| Ok(Box::new(SleekApp::new(cc, fit_viewport)))),
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
        // Always fill the real display — portrait *and* landscape. The desktop
        // path uses fill=false for a 480px phone column; that must not apply
        // when the device is rotated wide.
        Box::new(|cc| Ok(Box::new(SleekApp::new(cc, true)))),
    )
}

struct SleekApp {
    mode: Mode,
    state: AppState,
    net: NetBridge,
    /// Full window width (Codespace / SLEEK_FIT_VIEWPORT); skip 480px desktop column.
    fill_viewport: bool,
    /// One-shot: force fullscreen / screen size on Codespace VNC (Fluxbox).
    fit_viewport_pending: bool,
}

impl SleekApp {
    fn new(cc: &eframe::CreationContext<'_>, fit_viewport: bool) -> Self {
        let theme = Theme::dark();
        apply(&cc.egui_ctx, &theme);
        let state = AppState::new();
        let net = NetBridge::start();
        let mut app = Self {
            mode: Mode::Dark,
            state,
            net,
            fill_viewport: fit_viewport,
            fit_viewport_pending: fit_viewport,
        };
        // Cold-start OAuth: freeq://auth may have launched us; prefer that over
        // auto-reconnect / guest so the broker callback is not discarded.
        #[cfg(target_os = "android")]
        if let Some(url) = crate::android_media::take_pending_deep_link() {
            app.apply_freeq_deep_link(&url);
            return app;
        }
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

    fn apply_fit_viewport(&mut self, ctx: &egui::Context) {
        if !self.fit_viewport_pending {
            return;
        }
        self.fit_viewport_pending = false;
        // Prefer live monitor size from winit; fall back to xdpyinfo / env.
        let size = ctx
            .input(|i| i.viewport().monitor_size)
            .filter(|s| s.x > 0.0 && s.y > 0.0)
            .or_else(detect_screen_size);
        if let Some(size) = size {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
        }
        // Avoid borderless fullscreen + always-on-top: it buries the OAuth
        // browser under Sleek with no taskbar way to switch on Fluxbox.
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::viewport::WindowLevel::Normal,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }

    /// Drop always-on-top / fullscreen so the OAuth browser is reachable in VNC.
    fn yield_to_oauth_browser(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::viewport::WindowLevel::Normal,
        ));
        // Slightly shrink so Fluxbox can stack Chromium on top visibly.
        if let Some(size) = detect_screen_size() {
            let w = (size.x * 0.55).max(400.0);
            let h = (size.y * 0.85).max(400.0);
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
        }
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

    /// Fire a scheduled auto-reconnect once its backoff deadline elapses.
    fn poll_auto_reconnect(&mut self, ctx: &egui::Context) {
        let Some(at) = self.state.reconnect_at else {
            return;
        };
        // Keep painting so the countdown status updates and the deadline is checked.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        if std::time::Instant::now() < at {
            let secs = at
                .saturating_duration_since(std::time::Instant::now())
                .as_secs()
                .max(1);
            self.state.status_line = format!("Reconnecting in {secs}s…");
            return;
        }
        if self.state.connection != ConnectionState::Disconnected {
            self.state.reconnect_at = None;
            return;
        }
        if self.state.intentional_disconnect || self.state.nick.trim().is_empty() {
            self.state.cancel_auto_reconnect();
            return;
        }
        self.state.reconnect_at = None;
        self.state.error = None;
        if self.state.has_saved_session() {
            self.do_reconnect_session();
        } else {
            self.do_connect();
        }
    }

    /// Debounced Bluesky handle typeahead while the connect form is visible.
    fn poll_handle_typeahead(&mut self, ctx: &egui::Context) {
        if self.state.connection.is_live() || self.state.connect_mode != crate::state::ConnectMode::Bluesky
        {
            return;
        }
        self.state
            .handle_typeahead
            .sync_from_input(&self.state.form_handle);
        if let Some((request_id, query)) = self.state.handle_typeahead.take_ready_fetch() {
            self.net.send(NetCmd::SearchHandles { request_id, query });
        }
        if self.state.handle_typeahead.needs_repaint() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn poll_net(&mut self, ctx: &egui::Context) {
        for ev in self.net.poll() {
            self.handle_net_event(ev);
        }
        // Kick off any image / OG fetches requested while painting chat.
        self.flush_media_fetches();
        self.poll_android_camera_permission();
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

    /// After the user grants CAMERA mid-call, enable publish once (no re-dial).
    #[cfg(target_os = "android")]
    fn poll_android_camera_permission(&mut self) {
        let enable = {
            let Some(lc) = self.state.local_call.as_ref() else {
                return;
            };
            if !lc.camera_awaiting_permission || !lc.camera {
                return;
            }
            if !matches!(lc.media, MediaStatus::Live) {
                return;
            }
            if !crate::android_media::has_camera_permission() {
                return;
            }
            true
        };
        if !enable {
            return;
        }
        if let Some(lc) = self.state.local_call.as_mut() {
            lc.camera_awaiting_permission = false;
            // Optimistic: toggle stays usable while Camera2 opens.
            lc.has_camera = true;
        }
        log::info!("av-media: CAMERA granted — enabling deferred camera publish");
        self.net.send(NetCmd::AvCamera { enabled: true });
        self.state.show_toast("Camera permission granted");
    }

    #[cfg(not(target_os = "android"))]
    fn poll_android_camera_permission(&mut self) {}

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
                        self.state.clear_av_media();
                    }
                }
                self.state.error = Some(s.clone());
                self.state.show_toast(s);
                self.state.awaiting_oauth = false;
                if self.state.connection == ConnectionState::Connecting {
                    self.state.connection = ConnectionState::Disconnected;
                }
                // A failed reconnect attempt (after EOF etc.) should keep backing off.
                if !self.state.intentional_disconnect
                    && self.state.reconnect_attempts > 0
                    && !self.state.nick.trim().is_empty()
                    && self.state.reconnect_at.is_none()
                {
                    self.state.schedule_auto_reconnect();
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
                    self.state.compose_nick_tab.clear();
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
                        muted: self.state.av_pref_muted,
                        speaker_muted: self.state.av_pref_speaker_muted,
                        camera: self.state.av_pref_camera,
                        has_camera: false,
                        has_mic: false,
                        media: MediaStatus::Idle,
                        awaiting_start: started,
                        camera_awaiting_permission: false,
                    });
                }
                if started {
                    self.state.show_toast("Starting call…");
                } else {
                    self.state.show_toast("Joining call…");
                    // Join has a session id; dial only if we already hold a JWT
                    // (or local SFU). Otherwise wait for `+freeq.at/av-token` —
                    // dialing without jwt against freeq's SFU gets the
                    // connection closed immediately and moq-lite spams WARNs.
                    if !sid.is_empty() {
                        let tok = self
                            .state
                            .local_call
                            .as_ref()
                            .and_then(|lc| lc.token.clone());
                        self.try_start_av_media(
                            &channel,
                            &sid,
                            &instance,
                            tok.as_deref(),
                        );
                    }
                }
            }
            NetEvent::AvMediaStatus {
                status,
                video,
                has_camera,
                mic_level,
                has_mic,
            } => {
                let mut sync_camera: Option<bool> = None;
                if let Some(lc) = self.state.local_call.as_mut() {
                    lc.media = status.clone();
                    if matches!(status, MediaStatus::Live) {
                        lc.has_camera = has_camera;
                        lc.has_mic = has_mic;
                        // No capture device → force muted UI so the user sees
                        // listen-only instead of a green mic that isn't sending.
                        if !has_mic {
                            lc.muted = true;
                            self.state.av_pref_muted = true;
                        }
                        // Do NOT wipe camera intent when hardware is missing:
                        // `lc.camera = has_camera && lc.camera` permanently forced
                        // camera off after a failed open, so a later re-dial opened
                        // the device but published no frames (gated off).
                        //
                        // Skip sync while awaiting Android CAMERA grant — enabling
                        // now would fail open and flap Live status updates.
                        if has_camera && !lc.camera_awaiting_permission {
                            // Hardware present — keep join/pre-call intent; re-sync
                            // the media plane in case AtomicBool drifted.
                            sync_camera = Some(lc.camera);
                        }
                    }
                    if matches!(
                        status,
                        MediaStatus::Idle | MediaStatus::Failed(_) | MediaStatus::BrowserOnly
                    ) {
                        // Media plane down — hide camera control; keep mute/camera prefs.
                        lc.has_camera = false;
                        lc.has_mic = false;
                        lc.camera_awaiting_permission = false;
                    }
                }
                if let Some(store) = video {
                    self.state.av_video = Some(store);
                }
                if let Some(level) = mic_level {
                    self.state.av_mic_level = Some(level);
                }
                if matches!(
                    status,
                    MediaStatus::Idle | MediaStatus::Failed(_) | MediaStatus::BrowserOnly
                ) {
                    self.state.clear_av_media();
                }
                if let Some(enabled) = sync_camera {
                    self.net.send(NetCmd::AvCamera { enabled });
                }
                match &status {
                    MediaStatus::Live => {
                        let awaiting = self
                            .state
                            .local_call
                            .as_ref()
                            .is_some_and(|lc| lc.camera_awaiting_permission);
                        let cam = if awaiting {
                            " · awaiting camera permission"
                        } else if has_camera {
                            if self
                                .state
                                .local_call
                                .as_ref()
                                .is_some_and(|lc| lc.camera)
                            {
                                " + camera on"
                            } else {
                                " + camera (off)"
                            }
                        } else if self.state.av_pref_camera {
                            // User wanted video but open failed (busy device,
                            // dead OBS node, etc.) — make it visible in the toast.
                            " · camera unavailable"
                        } else {
                            ""
                        };
                        let mic = if has_mic {
                            ""
                        } else {
                            " · no mic (listen-only)"
                        };
                        self.state
                            .show_toast(format!("Call media connected{cam}{mic}"));
                    }
                    MediaStatus::Failed(e) => {
                        self.state.show_toast(format!("Call media failed: {e}"));
                    }
                    _ => {}
                }
            }
            NetEvent::HandleSuggestions {
                request_id,
                query,
                actors,
                error,
            } => {
                if error.is_some() {
                    self.state.handle_typeahead.apply_failed(request_id);
                } else {
                    self.state
                        .handle_typeahead
                        .apply_results(request_id, query, actors);
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
                    self.state.intentional_disconnect = true;
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
                self.state.cancel_auto_reconnect();
                self.state.intentional_disconnect = false;
                self.state.show_toast(format!("Connected as {nick}"));
                self.state.persist_session();
                // Seed status buffer
                let buf = self.state.ensure_buffer("*status");
                buf.append(ChatMessage::system(format!("Registered as {nick}")));
                // Show remembered rooms in the chat list while auto-join is in
                // flight (net layer JOINs them on Registered).
                for ch in self.state.auto_join_channels() {
                    let buf = self.state.ensure_buffer(&ch);
                    if !buf.is_joined() {
                        buf.join_pending = true;
                    }
                }
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
                // Client-side room memory: remember successful own joins so
                // reconnect re-joins without depending on server state.
                if own {
                    self.state.remember_channel(&channel);
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
                    self.state.forget_channel(&channel);
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
                        // Prefer IRCv3 tag when present; freeq-sdk pin may lack site_name.
                        site_name: tags.get("link-site").cloned(),
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

                let reactions = tags
                    .get("+freeq.at/reactions")
                    .map(|raw| parse_reactions_tag(raw))
                    .unwrap_or_default();
                // Live / history edit: rewrite the original in place when present.
                let edit_of = tags
                    .get("+draft/edit")
                    .or_else(|| tags.get("draft/edit"))
                    .filter(|id| !id.is_empty())
                    .cloned();
                let is_edited = edit_of.is_some()
                    || tags.contains_key("+freeq.at/edited")
                    || tags.get("edited").is_some_and(|v| !v.is_empty());

                let viewing = self
                    .state
                    .active_channel
                    .as_ref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&buffer_name));

                if let Some(original_id) = edit_of.as_deref() {
                    let applied = {
                        let buf = self.state.ensure_buffer(&buffer_name);
                        buf.apply_edit(
                            &from,
                            original_id,
                            Some(msgid.as_str()),
                            &body,
                            embed.clone(),
                            link_meta.clone(),
                        )
                    };
                    if applied {
                        // Rewrote in place — skip appending a second row.
                        return;
                    }
                }

                let msg = ChatMessage {
                    id: msgid,
                    from: from.clone(),
                    text: body,
                    is_system: false,
                    is_action,
                    is_edited,
                    is_deleted: false,
                    timestamp,
                    reply_to,
                    edit_of,
                    is_signed,
                    embed,
                    link_meta,
                    reactions,
                };

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
                    self.state.channels.remove(&channel);
                    self.state.channel_order.retain(|n| n != &channel);
                    self.state.forget_channel(&channel);
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
                let intentional = self.state.intentional_disconnect || reason == "quit";
                self.state.intentional_disconnect = false;

                // Quit after "Guest nick → refresh session" already kicked off
                // Connecting — don't clobber that in-flight restore.
                if intentional {
                    if self.state.connection == ConnectionState::Connecting {
                        return;
                    }
                    self.state.clear_session();
                    return;
                }

                // Unexpected EOF / ping timeout / socket drop — keep buffers and
                // auto-reconnect with backoff (freeq-android parity).
                let nick_empty = self.state.nick.trim().is_empty();
                if crate::reconnect::should_auto_reconnect(false, nick_empty, &reason) {
                    self.state.mark_disconnected_for_reconnect(&reason);
                    self.state.error = Some(format!("Disconnected: {reason}"));
                    self.state.show_toast(format!("Disconnected: {reason}"));
                    self.state.schedule_auto_reconnect();
                } else {
                    self.state.clear_session();
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
                self.handle_reaction_tags(&key, &from, &tags);
                self.handle_delete_tags(&key, &from, &tags);
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
            let err_sid = tags
                .get("+freeq.at/av-id")
                .filter(|s| !s.is_empty())
                .cloned()
                .or_else(|| av::session_id_from_collision_reason(&reason));

            // start-collision: our av-start lost the race (or we started while a
            // session was already live). freeq names the winning session in
            // +freeq.at/av-id — converge by joining it instead of toasting an
            // error and leaving the user stuck. Matches freeq-app client.ts.
            //
            // Note: this TAGMSG is directed at our nick, so `channel` here is a
            // DM buffer key, not the call channel. Prefer local_call.channel.
            if code == "start-collision" {
                let join_channel = self
                    .state
                    .local_call
                    .as_ref()
                    .filter(|lc| lc.awaiting_start || lc.session_id.is_empty())
                    .map(|lc| lc.channel.clone())
                    .or_else(|| av::channel_from_collision_reason(&reason));
                if let (Some(sid), Some(ch)) = (err_sid.as_ref(), join_channel) {
                    // Seed channel roster so the banner shows "Join" if join fails later.
                    let buf = self.state.ensure_buffer(&ch);
                    let needs_seed = buf
                        .call
                        .as_ref()
                        .map(|c| c.session_id != *sid)
                        .unwrap_or(true);
                    if needs_seed {
                        buf.call = Some(av::ChannelCall {
                            session_id: sid.clone(),
                            participants: 1,
                            ..Default::default()
                        });
                    }
                    // Drop optimistic start claim so do_av_join can re-claim the slot.
                    self.state.local_call = None;
                    self.state.clear_av_media();
                    self.state.show_toast("Call already open — joining…");
                    self.do_av_join(ch, sid.clone());
                    return;
                }
            }

            self.state.show_toast(format!("Call error: {reason}"));
            // Tear down optimistic local call on join-failed / failed start.
            // For join-failed, only if av-id matches our session (or is empty).
            if let Some(lc) = &self.state.local_call {
                let about_us = match code.as_str() {
                    "join-failed" => {
                        let ours = lc.session_id.as_str();
                        err_sid
                            .as_deref()
                            .map(|s| s.is_empty() || ours.is_empty() || s == ours)
                            .unwrap_or(true)
                    }
                    _ => lc.awaiting_start || lc.session_id.is_empty(),
                };
                if about_us {
                    self.state.local_call = None;
                    self.state.clear_av_media();
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
                    self.state.clear_av_media();
                }
            }
        }
    }

    /// Dial MoQ media (desktop + Android). Android publishes mic + optional
    /// Camera2 video (NV12 → H.264) when CAMERA permission is granted.
    ///
    /// Skips dial when the SFU needs a JWT we do not have yet, and skips
    /// re-dial while already Connecting/Live for the same session + token
    /// (avoids stop/reconnect churn that floods moq-lite connection-closed WARNs).
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
        if !av::can_dial_sfu(&self.state.server, token) {
            log::info!(
                "av-media: waiting for SFU JWT before dial (session {session_id})"
            );
            return;
        }
        // Android 6+: RECORD_AUDIO / CAMERA are runtime. Batch into one dialog
        // (a second requestPermissions replaces the first). Dial proceeds with
        // camera publish only when CAMERA is already granted; otherwise we
        // defer and enable after [`poll_android_camera_permission`].
        #[cfg(target_os = "android")]
        let android_camera_granted = {
            let want_camera = self
                .state
                .local_call
                .as_ref()
                .map(|lc| lc.camera)
                .unwrap_or(self.state.av_pref_camera);
            let perms = crate::android_media::ensure_av_call_permissions(want_camera);
            if want_camera && !perms.camera {
                if let Some(lc) = self.state.local_call.as_mut() {
                    lc.camera_awaiting_permission = true;
                }
            }
            perms.camera
        };
        #[cfg(not(target_os = "android"))]
        let android_camera_granted = true;
        // Dedup: same session + same token while already up / in flight.
        if let Some(lc) = self.state.local_call.as_ref() {
            let same_session = lc.session_id == session_id;
            let same_token = lc.token.as_deref() == token;
            if same_session
                && same_token
                && matches!(lc.media, MediaStatus::Live | MediaStatus::Connecting)
            {
                log::debug!("av-media: already {:?}, skip re-dial", lc.media);
                return;
            }
        }
        // freeq-app parity: `?inst=` always (media revocation), `?jwt=` when minted.
        let inst = if instance.is_empty() {
            None
        } else {
            Some(instance)
        };
        let sfu = match av::sfu_moq_dial_url(&self.state.server, token, inst) {
            Ok(u) => u.to_string(),
            Err(e) => {
                if let Some(lc) = self.state.local_call.as_mut() {
                    lc.media = MediaStatus::Failed(e.clone());
                }
                self.state.show_toast(format!("SFU URL: {e}"));
                return;
            }
        };
        let muted = self
            .state
            .local_call
            .as_ref()
            .map(|lc| lc.muted)
            .unwrap_or(self.state.av_pref_muted);
        let speaker_muted = self
            .state
            .local_call
            .as_ref()
            .map(|lc| lc.speaker_muted)
            .unwrap_or(self.state.av_pref_speaker_muted);
        // In-call intent if set; otherwise persisted pref.
        let camera_intent = self
            .state
            .local_call
            .as_ref()
            .map(|lc| lc.camera)
            .unwrap_or(self.state.av_pref_camera);
        let camera = av::camera_publish_at_dial(camera_intent, android_camera_granted);
        if let Some(lc) = self.state.local_call.as_mut() {
            lc.media = MediaStatus::Connecting;
            // Keep token in sync so the next try_start can dedup.
            if let Some(t) = token {
                if lc.token.as_deref() != Some(t) {
                    lc.token = Some(t.to_string());
                }
            }
            if camera_intent && !android_camera_granted {
                lc.camera_awaiting_permission = true;
            } else if android_camera_granted {
                lc.camera_awaiting_permission = false;
            }
        }
        log::info!(
            "av-media: dial camera={camera} (intent={camera_intent} perm={android_camera_granted}) \
             cam_id={:?} mic={:?} spk={:?} speaker_muted={speaker_muted}",
            self.state.av_pref_camera_id,
            self.state.av_pref_mic_id,
            self.state.av_pref_speaker_id
        );
        self.net.send(NetCmd::AvMediaConnect {
            sfu_url: sfu,
            session_id: session_id.to_string(),
            nick: self.state.nick.clone(),
            instance: instance.to_string(),
            muted,
            speaker_muted,
            camera,
            camera_id: self.state.av_pref_camera_id.clone(),
            mic_id: self.state.av_pref_mic_id.clone(),
            speaker_id: self.state.av_pref_speaker_id.clone(),
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
            muted: self.state.av_pref_muted,
            speaker_muted: self.state.av_pref_speaker_muted,
            camera: self.state.av_pref_camera,
            has_camera: false,
            has_mic: false,
            media: MediaStatus::Idle,
            awaiting_start: true,
            camera_awaiting_permission: false,
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
            muted: self.state.av_pref_muted,
            speaker_muted: self.state.av_pref_speaker_muted,
            camera: self.state.av_pref_camera,
            has_camera: false,
            has_mic: false,
            media: MediaStatus::Idle,
            awaiting_start: false,
            camera_awaiting_permission: false,
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
        // Remember last in-call choices for the next start/join (and next launch).
        self.state.av_pref_muted = lc.muted;
        self.state.av_pref_speaker_muted = lc.speaker_muted;
        // Only sync camera pref when hardware was present (otherwise Live
        // forced `camera = false` and would wipe a deliberate "camera on").
        if lc.has_camera {
            self.state.av_pref_camera = lc.camera;
        }
        self.state.persist_av_prefs();
        self.state.clear_av_media();
        self.net.send(NetCmd::AvLeave {
            channel: lc.channel,
            session_id: lc.session_id,
            instance: lc.instance,
        });
        self.net.send(NetCmd::AvMediaStop);
    }

    fn do_av_toggle_mute(&mut self) {
        let muted = {
            let Some(lc) = self.state.local_call.as_mut() else {
                return;
            };
            lc.muted = !lc.muted;
            lc.muted
        };
        self.state.av_pref_muted = muted;
        self.state.persist_av_prefs();
        self.net.send(NetCmd::AvMute { muted });
    }

    fn do_av_toggle_speaker_mute(&mut self) {
        let muted = {
            let Some(lc) = self.state.local_call.as_mut() else {
                return;
            };
            lc.speaker_muted = !lc.speaker_muted;
            lc.speaker_muted
        };
        self.state.av_pref_speaker_muted = muted;
        self.state.persist_av_prefs();
        self.net.send(NetCmd::AvSpeakerMute { muted });
    }

    fn do_av_toggle_camera(&mut self) {
        let camera = {
            let Some(lc) = self.state.local_call.as_mut() else {
                return;
            };
            if !lc.has_camera {
                #[cfg(target_os = "android")]
                {
                    // Permission denied / not yet granted: prompt and defer enable.
                    if !crate::android_media::has_camera_permission() {
                        let _ = crate::android_media::ensure_camera_permission();
                        lc.camera = true;
                        lc.camera_awaiting_permission = true;
                        self.state.av_pref_camera = true;
                        self.state.persist_av_prefs();
                        self.state
                            .show_toast("Grant camera permission to enable video");
                        return;
                    }
                }
                self.state.show_toast("No camera available");
                return;
            }
            lc.camera = !lc.camera;
            lc.camera
        };
        self.state.av_pref_camera = camera;
        self.state.persist_av_prefs();
        #[cfg(target_os = "android")]
        if camera && !crate::android_media::has_camera_permission() {
            let _ = crate::android_media::ensure_camera_permission();
            if let Some(lc) = self.state.local_call.as_mut() {
                lc.camera_awaiting_permission = true;
            }
            self.state
                .show_toast("Grant camera permission to enable video");
            return;
        }
        if let Some(lc) = self.state.local_call.as_mut() {
            lc.camera_awaiting_permission = false;
        }
        self.net.send(NetCmd::AvCamera {
            enabled: camera,
        });
    }

    fn do_av_select_camera(&mut self, id: Option<String>) {
        self.state.av_pref_camera_id = id.clone();
        self.state.persist_av_prefs();
        if self.state.local_call.is_some() {
            self.net.send(NetCmd::AvCameraDevice { id });
        }
    }

    fn do_av_select_mic(&mut self, id: Option<String>) {
        self.state.av_pref_mic_id = id.clone();
        self.state.persist_av_prefs();
        if self.state.local_call.is_some() {
            self.net.send(NetCmd::AvMicDevice { id });
        }
    }

    fn do_av_select_speaker(&mut self, id: Option<String>) {
        self.state.av_pref_speaker_id = id.clone();
        self.state.persist_av_prefs();
        if self.state.local_call.is_some() {
            self.net.send(NetCmd::AvSpeakerDevice { id });
        }
    }

    /// Dispatch in-call chrome actions from the global call panel.
    fn handle_av_call_action(&mut self, _ctx: &egui::Context, act: ChatAction) {
        match act {
            ChatAction::AvLeave => self.do_av_leave(),
            ChatAction::AvToggleMute => self.do_av_toggle_mute(),
            ChatAction::AvToggleSpeakerMute => self.do_av_toggle_speaker_mute(),
            ChatAction::AvToggleCamera => self.do_av_toggle_camera(),
            ChatAction::AvSelectCamera(id) => self.do_av_select_camera(id),
            ChatAction::AvSelectMic(id) => self.do_av_select_mic(id),
            ChatAction::AvSelectSpeaker(id) => self.do_av_select_speaker(id),
            ChatAction::OpenCallChannel(ch) => {
                self.state.open_chat(&ch);
            }
            _ => {}
        }
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
        self.state.intentional_disconnect = false;
        self.state.reconnect_at = None;
        self.state.connection = ConnectionState::Connecting;
        self.state.awaiting_oauth = false;
        self.state.nick = nick.clone();
        self.state.server = server.clone();
        self.state.use_tls = self.state.form_tls;
        self.state.use_websocket = self.state.form_websocket;
        self.state.status_line = "Connecting…".into();

        // Client-side room memory (prefs) — freeq does not reliably restore
        // channel membership across reconnects. Defaults: #general, #test.
        let auto_join = self.state.auto_join_channels();

        self.net.send(NetCmd::Connect {
            nick,
            server,
            tls: self.state.form_tls,
            websocket: self.state.form_websocket,
            auto_join,
            web_token,
        });
    }

    /// User-initiated disconnect: suppress auto-reconnect for the Quit event.
    fn do_intentional_disconnect(&mut self) {
        self.state.intentional_disconnect = true;
        self.state.cancel_auto_reconnect();
        self.net.send(NetCmd::Quit);
        self.state.clear_session();
    }

    fn do_bluesky_login(&mut self, ctx: &egui::Context) {
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
        self.state.handle_typeahead.dismiss();
        self.state.awaiting_oauth = true;
        self.state.connection = ConnectionState::Connecting;
        #[cfg(target_os = "android")]
        {
            let _ = ctx;
            self.state.status_line =
                "Browser opened — finish sign-in; Sleek reopens via freeq://".into();
        }
        #[cfg(not(target_os = "android"))]
        {
            self.state.status_line = "Browser opened for sign-in — look for the Chromium window (Alt+Tab if covered).".into();
            // Fullscreen Sleek sits above everything on Fluxbox/VNC; yield first.
            self.yield_to_oauth_browser(ctx);
        }
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

    /// Consume a `freeq://auth?…` deep link (Android intent-filter / paste parity).
    fn apply_freeq_deep_link(&mut self, raw: &str) {
        let raw = raw.trim();
        if !raw.starts_with("freeq://") {
            return;
        }
        // freeq://chat/… could be handled later; OAuth is freeq://auth?…
        if raw.starts_with("freeq://auth") {
            match auth::parse_freeq_auth_url(raw) {
                Ok(tokens) => {
                    log::info!("oauth deep link applied (nick={})", tokens.nick);
                    self.state.awaiting_oauth = false;
                    self.state.status_line = "Sign-in complete".into();
                    self.apply_auth_tokens(tokens, /*connect=*/ true);
                }
                Err(e) => {
                    log::warn!("oauth deep link invalid: {e}");
                    self.state.error = Some(format!("Invalid freeq://auth callback: {e}"));
                    self.state.awaiting_oauth = false;
                    if self.state.connection == ConnectionState::Connecting {
                        self.state.connection = ConnectionState::Disconnected;
                    }
                }
            }
        } else {
            log::debug!("ignored freeq deep link: {raw}");
        }
    }

    /// Poll Android for a pending freeq:// OAuth deep link (SleekActivity).
    ///
    /// Warm path: browser redirects to freeq://auth while we are still running
    /// (`awaiting_oauth`); `onNewIntent` stashes the URI for this poll.
    #[cfg(target_os = "android")]
    fn poll_oauth_deep_link(&mut self, ctx: &egui::Context) {
        if !self.state.awaiting_oauth {
            return;
        }
        if let Some(url) = crate::android_media::take_pending_deep_link() {
            self.apply_freeq_deep_link(&url);
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    #[cfg(not(target_os = "android"))]
    fn poll_oauth_deep_link(&mut self, _ctx: &egui::Context) {}


    fn do_reconnect_session(&mut self) {
        let Some(broker_token) = self.state.broker_token.clone() else {
            self.state.error = Some("No saved session".into());
            return;
        };
        self.state.error = None;
        self.state.intentional_disconnect = false;
        self.state.reconnect_at = None;
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
        // Slash commands don't apply when attaching media (caption is plain text).
        if self.state.compose_image.is_some() {
            self.do_send_with_image(target, text);
            return;
        }

        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.state.compose.clear();
        self.state.compose_nick_tab.clear();

        if text.starts_with('/') {
            self.handle_slash_command(&target, &text);
            return;
        }

        self.do_send_local_echo(&target, text.clone());
        self.net.send(NetCmd::Privmsg { target, text });
    }

    /// Dispatch `/join`, `/me`, `/msg`, … — mirrors freeq-android ComposeBar.
    fn handle_slash_command(&mut self, active_target: &str, input: &str) {
        match crate::slash::parse(input) {
            crate::slash::SlashCommand::Join(channel) => {
                self.do_join(channel);
            }
            crate::slash::SlashCommand::PartActive => {
                self.do_part(active_target.to_string());
            }
            crate::slash::SlashCommand::Nick(new_nick) => {
                self.net.send(NetCmd::Raw(format!("NICK {new_nick}")));
            }
            crate::slash::SlashCommand::Me(action) => {
                let wire = format!("\u{1}ACTION {action}\u{1}");
                self.do_send_local_echo_action(active_target, action);
                self.net.send(NetCmd::Privmsg {
                    target: active_target.to_string(),
                    text: wire,
                });
            }
            crate::slash::SlashCommand::Msg { target, text } => {
                self.do_send_local_echo(&target, text.clone());
                self.net.send(NetCmd::Privmsg { target, text });
            }
            crate::slash::SlashCommand::Topic(topic) => {
                self.net
                    .send(NetCmd::Raw(format!("TOPIC {active_target} :{topic}")));
            }
            crate::slash::SlashCommand::Raw(line) => {
                self.net.send(NetCmd::Raw(line));
            }
            crate::slash::SlashCommand::Empty => {}
        }
    }

    fn do_part(&mut self, channel: String) {
        // Only PART the server if we actually joined; denied joins are local-only.
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
        self.state.forget_channel(&channel);
        if self.state.active_channel.as_deref() == Some(channel.as_str()) {
            self.state.close_chat();
        }
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
        self.do_send_local_echo_inner(target, text, false);
    }

    fn do_send_local_echo_action(&mut self, target: &str, text: String) {
        self.do_send_local_echo_inner(target, text, true);
    }

    fn do_send_local_echo_inner(&mut self, target: &str, text: String, is_action: bool) {
        // Optimistic local echo for snappy UI. When the server echoes the
        // PRIVMSG back (echo-message), Buffer::append drops this local-* row
        // and keeps the real msgid / signed copy — so the user never sees
        // the line twice.
        //
        // File under the canonical DM key (DID when known) so the echo lands
        // in the same thread as the server echo and peer replies.
        let buffer_key = self.state.dm_buffer_key(target);
        let embed = if is_action {
            None
        } else {
            preview::embed_from_text(&text)
        };
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
            is_action,
            is_edited: false,
            is_deleted: false,
            timestamp: chrono::Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed,
            link_meta: None,
            reactions: Default::default(),
        };
        let buf = self.state.ensure_buffer(&buffer_key);
        buf.append(msg);
    }

    /// Apply live reaction TAGMSG (`+react` / `+freeq.at/unreact` + `+reply`).
    fn handle_reaction_tags(
        &mut self,
        buffer: &str,
        from: &str,
        tags: &std::collections::HashMap<String, String>,
    ) {
        let reply = tags
            .get("+reply")
            .or_else(|| tags.get("reply"))
            .or_else(|| tags.get("+draft/reply"))
            .or_else(|| tags.get("draft/reply"))
            .cloned();
        let Some(msg_id) = reply.filter(|id| !id.is_empty()) else {
            return;
        };

        if let Some(emoji) = tags
            .get("+freeq.at/unreact")
            .or_else(|| tags.get("freeq.at/unreact"))
            .filter(|e| !e.trim().is_empty())
            .cloned()
        {
            if let Some(buf) = self.state.channels.get_mut(buffer) {
                buf.remove_reaction(&msg_id, &emoji, from);
            }
            return;
        }

        if let Some(emoji) = tags
            .get("+react")
            .or_else(|| tags.get("react"))
            .or_else(|| tags.get("+draft/react"))
            .or_else(|| tags.get("draft/react"))
            .filter(|e| !e.trim().is_empty())
            .cloned()
        {
            if let Some(buf) = self.state.channels.get_mut(buffer) {
                buf.add_reaction(&msg_id, &emoji, from);
            }
        }
    }

    /// Toggle our reaction on a message (optimistic + wire).
    fn do_toggle_react(&mut self, target: String, msgid: String, emoji: String) {
        if msgid.is_empty() || emoji.trim().is_empty() {
            return;
        }
        if msgid.starts_with("local-") {
            self.state
                .show_toast("Wait for the message to send before reacting");
            return;
        }
        let nick = self.state.nick.clone();
        let buffer_key = self.state.dm_buffer_key(&target);
        let already = self
            .state
            .channels
            .get(&buffer_key)
            .and_then(|b| b.messages.iter().find(|m| m.id == msgid))
            .is_some_and(|m| m.has_reaction_from(&emoji, &nick));

        if already {
            if let Some(buf) = self.state.channels.get_mut(&buffer_key) {
                buf.remove_reaction(&msgid, &emoji, &nick);
            }
            self.net.send(NetCmd::Unreact {
                target,
                emoji,
                msgid,
            });
        } else {
            if let Some(buf) = self.state.channels.get_mut(&buffer_key) {
                buf.add_reaction(&msgid, &emoji, &nick);
            }
            self.net.send(NetCmd::React {
                target,
                emoji,
                msgid,
            });
        }
    }

    /// Apply inbound `+draft/delete` TAGMSG (soft-delete tombstone).
    fn handle_delete_tags(
        &mut self,
        buffer: &str,
        from: &str,
        tags: &std::collections::HashMap<String, String>,
    ) {
        let delete_id = tags
            .get("+draft/delete")
            .or_else(|| tags.get("draft/delete"))
            .filter(|id| !id.is_empty())
            .cloned();
        let Some(msgid) = delete_id else {
            return;
        };
        if let Some(buf) = self.state.channels.get_mut(buffer) {
            let _ = buf.apply_delete(from, &msgid);
        }
    }

    /// Send a `+draft/edit` and clear compose edit mode.
    fn do_edit(&mut self, target: String, msgid: String, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() || msgid.is_empty() || msgid.starts_with("local-") {
            return;
        }
        // Optimistic local rewrite — echo-message will re-apply with the new msgid.
        let nick = self.state.nick.clone();
        let buffer_key = self.state.dm_buffer_key(&target);
        let embed = preview::embed_from_text(&text);
        if let Some(preview::Embed::Image { ref url }) = embed {
            self.state.media.touch_image(url);
        } else if let Some(preview::Embed::Link { ref url }) = embed {
            self.state.media.touch_link(url);
        }
        if let Some(buf) = self.state.channels.get_mut(&buffer_key) {
            let _ = buf.apply_edit(&nick, &msgid, None, &text, embed, None);
        }
        self.state.compose.clear();
        self.state.compose_nick_tab.clear();
        self.state.editing_msgid = None;
        self.net.send(NetCmd::EditMessage {
            target,
            msgid,
            text,
        });
    }

    /// Soft-delete a message. Optimistic tombstone — TAGMSG is not echoed to us.
    fn do_delete(&mut self, target: String, msgid: String) {
        if msgid.is_empty() || msgid.starts_with("local-") {
            return;
        }
        let nick = self.state.nick.clone();
        let buffer_key = self.state.dm_buffer_key(&target);
        if let Some(buf) = self.state.channels.get_mut(&buffer_key) {
            let _ = buf.apply_delete(&nick, &msgid);
        }
        if self.state.editing_msgid.as_deref() == Some(msgid.as_str()) {
            self.state.cancel_edit();
        }
        self.net.send(NetCmd::DeleteMessage { target, msgid });
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

impl eframe::App for SleekApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_fit_viewport(ctx);
        self.poll_auto_reconnect(ctx);
        self.poll_handle_typeahead(ctx);
        self.poll_net(ctx);
        self.poll_file_pick(ctx);
        self.poll_oauth_deep_link(ctx);

        let th = self.theme();
        let p = &th.palette;
        let sp = &th.spacing;
        let screen = ctx.screen_rect();
        let landscape = screen.width() > screen.height();
        // Short viewport (phone landscape, small windows): tighten chrome so
        // chat/compose keep usable height under tabs + call bar.
        let short = screen.height() < 520.0;
        // Fill full width on:
        // - Android (always — landscape used to fall through to the desktop
        //   480px column because width ≥ 640)
        // - Codespace / SLEEK_FIT_VIEWPORT
        // - Portrait-ish or narrow windows (phone shell on desktop)
        let fill = cfg!(target_os = "android")
            || self.fill_viewport
            || screen.height() >= screen.width()
            || screen.width() < 640.0;

        // System status / nav safe areas (edge-to-edge). Measured via
        // WindowInsets JNI + content_rect; top is floored so the status bar
        // is always respected (content_rect alone often reports top=0).
        // Bottom refined for 3-button vs gesture (android_media::navigation_mode).
        // IME clearance still lives in vidya::system_chrome when focused.
        #[cfg(target_os = "android")]
        if let Some(chrome) = crate::android_media::measured_system_chrome(ctx) {
            set_system_chrome(ctx, chrome);
        }
        reserve_system_chrome(ctx, &th);

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
                    // Cap width on ultrawide desktop so a single tile doesn't become a
                    // billboard; keep enough room for long channel titles + video.
                    // Height stays content-sized (TopBottomPanel sizes to children).
                    // On Codespace fill-viewport, use the full width.
                    let avail = ui.available_width();
                    let max_w = if fill { avail } else { avail.min(720.0) };
                    ui.set_max_width(max_w);
                    ui.set_min_width(max_w.min(avail));
                    if let Some(act) = ui::active_call_panel(ui, &th, &mut self.state) {
                        self.handle_av_call_action(ctx, act);
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
            let tab_vpad = if short { sp.xs } else { sp.sm };
            let tab_h = if short {
                (sp.control_height * 0.85).max(36.0)
            } else {
                sp.control_height
            };
            egui::TopBottomPanel::bottom("nav_bottom")
                .frame(
                    egui::Frame::new()
                        .fill(p.headerbar_bg)
                        .inner_margin(egui::Margin::symmetric(sp.md as i8, tab_vpad as i8))
                        .stroke(Stroke::new(1.0_f32, p.border_soft)),
                )
                .show_separator_line(false)
                .show(ctx, |ui| {
                    // Landscape: stretch tabs across the full width for easy taps.
                    let n_tabs = Tab::ALL.len().max(1) as f32;
                    let gap = sp.sm;
                    let row_w = ui.available_width();
                    let tab_w = if landscape && fill {
                        ((row_w - gap * (n_tabs - 1.0)) / n_tabs).max(56.0)
                    } else {
                        72.0
                    };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        let total_unread = self.state.total_unread();
                        for tab in Tab::ALL {
                            let selected = self.state.tab == tab;
                            let tab_fill = if selected { p.accent } else { p.button_bg };
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
                                .fill(tab_fill)
                                .stroke(if selected {
                                    Stroke::NONE
                                } else {
                                    Stroke::new(1.0_f32, p.border_soft)
                                })
                                .corner_radius(sp.radius_md)
                                .min_size(Vec2::new(tab_w, tab_h));
                            if ui.add(btn).clicked() {
                                self.state.tab = tab;
                            }
                        }
                    });
                });
        }

        // ── Header (compact when in chat) ─
        if !matches!(self.state.route, Route::Chat(_)) {
            // Landscape / short: one row (title · status) instead of stacked block.
            let (head_top, head_bot) = if short {
                (sp.xs + 2.0, sp.xs)
            } else {
                (sp.md + 4.0, sp.md + 2.0)
            };
            let header_frame = th.header_frame().inner_margin(egui::Margin {
                left: sp.page as i8,
                right: sp.page as i8,
                top: head_top as i8,
                bottom: head_bot as i8,
            });
            egui::TopBottomPanel::top("header")
                .frame(header_frame)
                .show_separator_line(false)
                .show(ctx, |ui| {
                    if landscape && short {
                        ui.horizontal(|ui| {
                            title(ui, &th, "Sleek");
                            if connected
                                || self.state.connection == ConnectionState::Connecting
                                || self.state.reconnect_at.is_some()
                            {
                                ui.add_space(sp.sm);
                                let blurb = if self.state.connection == ConnectionState::Registered
                                {
                                    format!("{} · {}", self.state.nick, self.state.server)
                                } else {
                                    "Connecting…".into()
                                };
                                dim_label(ui, &th, &blurb);
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                title(ui, &th, "Sleek");
                                // Subtitle only when connected or mid-connect — idle connect
                                // already has its own form; avoid stacking the same chrome.
                                if connected
                                    || self.state.connection == ConnectionState::Connecting
                                    || self.state.reconnect_at.is_some()
                                {
                                    ui.add_space(sp.xs + 2.0);
                                    let blurb =
                                        if self.state.connection == ConnectionState::Registered {
                                            format!("{} · {}", self.state.nick, self.state.server)
                                        } else {
                                            "Connecting…".into()
                                        };
                                    dim_label(ui, &th, &blurb);
                                }
                            });
                        });
                    }
                });
        }

        // ── Main content ───────────────────────────────────────────
        // Android / fill: tight side padding so content uses the full width;
        // vertical pad is minimal (system chrome + header/tabs already own
        // the safe edges — large page padding looked letterboxed).
        let page = if fill {
            let h_pad = if cfg!(target_os = "android") {
                if short { 6_i8 } else { 8_i8 }
            } else if short {
                8_i8
            } else {
                10_i8
            };
            let v_pad = if cfg!(target_os = "android") {
                if short { sp.xs as i8 } else { sp.sm as i8 }
            } else if short {
                sp.sm as i8
            } else {
                sp.page as i8
            };
            egui::Frame::new()
                .fill(p.window_bg)
                .inner_margin(egui::Margin::symmetric(h_pad, v_pad))
        } else {
            th.page_frame()
        };

        egui::CentralPanel::default().frame(page).show(ctx, |ui| {
            // Local desktop: narrow phone-like column. Codespace: full window.
            let col_w = if fill {
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
                    ChatAction::React {
                        target,
                        msgid,
                        emoji,
                    } => {
                        self.do_toggle_react(target, msgid, emoji);
                    }
                    ChatAction::Edit {
                        target,
                        msgid,
                        text,
                    } => {
                        self.do_edit(target, msgid, text);
                    }
                    ChatAction::Delete { target, msgid } => {
                        self.do_delete(target, msgid);
                    }
                    ChatAction::Part(channel) => {
                        self.do_part(channel);
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
                    ChatAction::AvToggleSpeakerMute => {
                        self.do_av_toggle_speaker_mute();
                    }
                    ChatAction::AvToggleCamera => {
                        self.do_av_toggle_camera();
                    }
                    ChatAction::AvSelectCamera(id) => {
                        self.do_av_select_camera(id);
                    }
                    ChatAction::AvSelectMic(id) => {
                        self.do_av_select_mic(id);
                    }
                    ChatAction::AvSelectSpeaker(id) => {
                        self.do_av_select_speaker(id);
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
                                        self.do_intentional_disconnect();
                                    }
                                    SettingsAction::Logout => {
                                        // Full clear: IRC + disk session → real guest next time.
                                        self.state.intentional_disconnect = true;
                                        self.state.cancel_auto_reconnect();
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
                        || self.state.reconnect_at.is_some()
                    {
                        // Waiting for registration / auto-reconnect backoff
                        ui.vertical_centered(|ui| {
                            ui.add_space(sp.xl * 2.0);
                            title(ui, &th, "Connecting");
                            ui.add_space(sp.md);
                            dim_label(ui, &th, &self.state.status_line);
                            ui.add_space(sp.lg);
                            if button(ui, &th, "Cancel").clicked() {
                                self.do_intentional_disconnect();
                            }
                        });
                    } else {
                        match ui::connect_screen(ui, &th, &mut self.state) {
                            ConnectAction::None => {}
                            ConnectAction::ConnectGuest => self.do_connect(),
                            ConnectAction::BlueskyLogin => self.do_bluesky_login(ctx),
                            ConnectAction::ApplyCallback => self.do_apply_callback(),
                            ConnectAction::ReconnectSession => self.do_reconnect_session(),
                            ConnectAction::ClearAccount => {
                                self.state.intentional_disconnect = true;
                                self.state.cancel_auto_reconnect();
                                self.net.send(NetCmd::Quit);
                                self.state.logout();
                            }
                        }
                    }

                    if fill {
                        ui.add_space(sp.xl + sp.lg);
                    }
                });
        });

        // Full-screen image lightbox (above chat / tabs).
        image_lightbox_overlay(ctx, &th, &mut self.state);
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
