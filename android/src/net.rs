//! Async freeq-sdk bridge: background tokio runtime ↔ egui UI thread.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use freeq_sdk::av::new_av_instance;
use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;
use tokio::sync::mpsc;

use crate::auth::{self, AuthTokens};
use crate::bsky::{self, HandleSuggestion};
use crate::state::{prefer_websocket, websocket_url_for};

use crate::av_media::{AvMediaConfig, AvMediaSession};

/// Commands from the UI into the network thread.
#[derive(Debug, Clone)]
pub enum NetCmd {
    Connect {
        nick: String,
        server: String,
        tls: bool,
        websocket: bool,
        auto_join: Vec<String>,
        /// One-shot SASL web-token from auth broker (consumed on first connect).
        web_token: Option<String>,
    },
    /// Open browser OAuth via auth broker loopback capture.
    BlueskyLogin {
        handle: String,
        auth_broker: String,
    },
    /// Mint a fresh web-token from durable broker_token (UI then Connects).
    ReconnectSession {
        broker_token: String,
        auth_broker: String,
    },
    Join(String),
    Part(String),
    Privmsg {
        target: String,
        text: String,
    },
    /// Raw IRC line (slash-command fallthrough, NICK, TOPIC, …).
    Raw(String),
    /// Upload image bytes to freeq `/api/v1/upload`, then PRIVMSG the URL (+ caption).
    UploadAndSend {
        /// Correlates with [`NetEvent::UploadFinished::upload_id`] so a cancelled
        /// compose upload cannot clear a newer attachment.
        upload_id: u64,
        target: String,
        caption: String,
        bytes: Vec<u8>,
        content_type: String,
        did: String,
        /// e.g. `https://irc.freeq.at`
        api_base: String,
    },
    /// CHATHISTORY LATEST — pull backlog when joining / opening a channel.
    HistoryLatest {
        target: String,
        count: usize,
    },
    /// CHATHISTORY TARGETS — DM conversation list (seeds chat list on cold connect).
    HistoryTargets {
        limit: usize,
    },
    /// Download an image for inline chat preview.
    FetchImage { url: String },
    /// Fetch Open Graph metadata for a link card.
    FetchLinkPreview { url: String },
    /// Open a freeq AV call in `channel` (sends `av-start`).
    AvStart { channel: String },
    /// Join an existing call by session id.
    AvJoin {
        channel: String,
        session_id: String,
    },
    /// Leave the call we are in.
    AvLeave {
        channel: String,
        session_id: String,
        instance: String,
    },
    /// Dial the MoQ SFU for media (desktop). Signaling must already be up.
    AvMediaConnect {
        sfu_url: String,
        session_id: String,
        nick: String,
        instance: String,
        /// Initial mic mute from pre-call prefs.
        muted: bool,
        /// Initial speaker / remote-audio mute from pre-call prefs.
        speaker_muted: bool,
        /// Initial camera publish from pre-call prefs.
        camera: bool,
        /// Preferred camera id (`None` = first available).
        camera_id: Option<String>,
        /// Preferred mic name (`None` = system default).
        mic_id: Option<String>,
        /// Preferred speaker name (`None` = system default).
        speaker_id: Option<String>,
    },
    /// Tear down MoQ media (leave call / disconnect).
    AvMediaStop,
    /// Mute / unmute local mic (media plane only).
    AvMute { muted: bool },
    /// Mute / unmute speaker (remote playback volume).
    AvSpeakerMute { muted: bool },
    /// Enable / disable camera publish (media plane only; desktop).
    AvCamera { enabled: bool },
    /// Switch camera device mid-call (desktop).
    AvCameraDevice { id: Option<String> },
    /// Switch microphone mid-call.
    AvMicDevice { id: Option<String> },
    /// Switch speaker / output mid-call.
    AvSpeakerDevice { id: Option<String> },
    /// TAGMSG `+react` + `+reply` — add a reaction on a message.
    React {
        target: String,
        emoji: String,
        msgid: String,
    },
    /// TAGMSG `+freeq.at/unreact` + `+reply` — remove our reaction.
    Unreact {
        target: String,
        emoji: String,
        msgid: String,
    },
    /// PRIVMSG with `+draft/edit` — replace a previously sent message.
    EditMessage {
        target: String,
        msgid: String,
        text: String,
    },
    /// TAGMSG `+draft/delete` — soft-delete a message.
    DeleteMessage { target: String, msgid: String },
    /// Bluesky handle typeahead for the connect-screen login field.
    SearchHandles {
        /// Monotonic id so the UI can ignore stale responses.
        request_id: u64,
        query: String,
    },
    Quit,
}

/// Events from the network thread into the UI.
#[derive(Debug, Clone)]
pub enum NetEvent {
    Sdk(Event),
    Status(String),
    Failed(String),
    /// Browser OAuth completed (or pasted freeq://auth was applied client-side).
    AuthReady(AuthTokens),
    /// Image upload finished; UI should clear compose attachment state.
    /// On success, `sent` is the PRIVMSG body that was just written to the server.
    UploadFinished {
        /// Correlates with [`NetCmd::UploadAndSend::upload_id`].
        upload_id: u64,
        /// `None` on success; error message on failure.
        error: Option<String>,
        /// Set when upload+send succeeded (for optimistic local echo).
        sent: Option<MediaSent>,
    },
    /// Remote image decoded for chat embed.
    ImageFetched {
        url: String,
        width: usize,
        height: usize,
        rgba: std::sync::Arc<[u8]>,
    },
    ImageFetchFailed {
        url: String,
    },
    /// Open Graph metadata for a link card.
    LinkPreviewFetched {
        url: String,
        title: Option<String>,
        description: Option<String>,
        thumb_url: Option<String>,
        site_name: Option<String>,
    },
    LinkPreviewFailed {
        url: String,
    },
    /// Instance id we used for av-start / av-join (UI tracks LocalCall).
    AvSignalingSent {
        channel: String,
        session_id: Option<String>,
        instance: String,
        /// true = start, false = join
        started: bool,
    },
    /// MoQ media plane status update.
    AvMediaStatus {
        status: crate::av::MediaStatus,
        /// Present when status is Live — shared latest remote (and local preview) frames.
        video: Option<crate::av::VideoFrameStore>,
        /// True when a capture device is publishing (desktop).
        has_camera: bool,
        /// Live mic envelope (0..=1) while media is Live.
        mic_level: Option<crate::av::MicLevel>,
        /// True when a real mic is feeding outbound Opus (false = silence/listen-only).
        has_mic: bool,
    },
    /// Bluesky handle typeahead results (or empty on soft failure).
    HandleSuggestions {
        request_id: u64,
        query: String,
        actors: Vec<HandleSuggestion>,
        /// `None` on success; set when the AppView request failed.
        error: Option<String>,
    },
}

/// Payload after a successful image upload + PRIVMSG.
#[derive(Debug, Clone)]
pub struct MediaSent {
    pub target: String,
    pub text: String,
}

/// Sync façade used by the egui app.
pub struct NetBridge {
    cmd_tx: mpsc::UnboundedSender<NetCmd>,
    event_rx: std::sync::mpsc::Receiver<NetEvent>,
}

impl NetBridge {
    pub fn start() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<NetCmd>();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<NetEvent>();

        thread::Builder::new()
            .name("sleek-net".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = event_tx.send(NetEvent::Failed(format!("tokio runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(network_loop(cmd_rx, event_tx));
            })
            .expect("spawn sleek-net thread");

        Self { cmd_tx, event_rx }
    }

    pub fn send(&self, cmd: NetCmd) {
        if let Err(e) = self.cmd_tx.send(cmd) {
            log::error!("net cmd send failed: {e}");
        }
    }

    pub fn poll(&self) -> Vec<NetEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            out.push(ev);
        }
        out
    }
}

async fn network_loop(
    mut cmd_rx: mpsc::UnboundedReceiver<NetCmd>,
    event_tx: std::sync::mpsc::Sender<NetEvent>,
) {
    let mut handle: Option<ClientHandle> = None;
    let mut events: Option<mpsc::Receiver<Event>> = None;
    let mut pending_joins: Vec<String> = Vec::new();
    let mut media: NetMedia = NetMedia::default();

    loop {
        // Prefer draining IRC events when connected; also wait for UI commands.
        if let Some(ref mut ev_rx) = events {
            tokio::select! {
                biased;
                maybe_ev = ev_rx.recv() => {
                    match maybe_ev {
                        Some(ev) => {
                            if let Event::Registered { ref nick } = ev {
                                let joins = std::mem::take(&mut pending_joins);
                                if let Some(h) = handle.clone() {
                                    for ch in joins {
                                        let _ = h.join(&ch).await;
                                    }
                                }
                                let _ = event_tx.send(NetEvent::Status(format!("Registered as {nick}")));
                            }
                            if matches!(ev, Event::Disconnected { .. }) {
                                handle = None;
                                media.stop().await;
                                // fall through: clear events after send
                                let _ = event_tx.send(NetEvent::Sdk(ev));
                                events = None;
                                continue;
                            }
                            let _ = event_tx.send(NetEvent::Sdk(ev));
                        }
                        None => {
                            handle = None;
                            events = None;
                            media.stop().await;
                            let _ = event_tx.send(NetEvent::Sdk(Event::Disconnected {
                                reason: "event channel closed".into(),
                            }));
                        }
                    }
                }
                maybe_cmd = cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(cmd) => {
                            apply_cmd(
                                cmd,
                                &mut handle,
                                &mut events,
                                &mut pending_joins,
                                &mut media,
                                &event_tx,
                            )
                            .await;
                        }
                        None => break,
                    }
                }
            }
        } else {
            match cmd_rx.recv().await {
                Some(cmd) => {
                    apply_cmd(
                        cmd,
                        &mut handle,
                        &mut events,
                        &mut pending_joins,
                        &mut media,
                        &event_tx,
                    )
                    .await;
                }
                None => break,
            }
        }
    }
}

/// MoQ media session owned by the net thread (desktop + Android).
#[derive(Default)]
struct NetMedia {
    session: Option<AvMediaSession>,
    muted: Arc<AtomicBool>,
    speaker_muted: Arc<AtomicBool>,
}

impl NetMedia {
    /// Clean teardown: soft-stop MoQ so we unpublish (avoid SFU ghost nicks
    /// that peers subscribe to and hear silence from).
    async fn stop(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.stop_and_wait(std::time::Duration::from_millis(1500)).await;
        }
    }

    fn set_muted(&mut self, muted: bool) {
        if let Some(s) = self.session.as_ref() {
            s.set_muted(muted);
        }
        self.muted = Arc::new(AtomicBool::new(muted));
    }

    fn set_speaker_muted(&mut self, muted: bool) {
        if let Some(s) = self.session.as_ref() {
            s.set_speaker_muted(muted);
        }
        self.speaker_muted = Arc::new(AtomicBool::new(muted));
    }

    fn set_camera(&mut self, enabled: bool) {
        if let Some(s) = self.session.as_ref() {
            s.set_camera_enabled(enabled);
        }
    }

    fn set_camera_device(&mut self, id: Option<String>) {
        if let Some(s) = self.session.as_ref() {
            s.set_camera_device(id);
        }
    }

    fn set_mic_device(&mut self, id: Option<String>) {
        if let Some(s) = self.session.as_ref() {
            s.set_mic_device(id);
        }
    }

    fn set_speaker_device(&mut self, id: Option<String>) {
        if let Some(s) = self.session.as_ref() {
            s.set_speaker_device(id);
        }
    }
}

async fn apply_cmd(
    cmd: NetCmd,
    handle: &mut Option<ClientHandle>,
    events: &mut Option<mpsc::Receiver<Event>>,
    pending_joins: &mut Vec<String>,
    media: &mut NetMedia,
    event_tx: &std::sync::mpsc::Sender<NetEvent>,
) {
    match cmd {
        NetCmd::BlueskyLogin {
            handle: bsky_handle,
            auth_broker,
        } => {
            let _ = event_tx.send(NetEvent::Status(
                "Opening browser for Bluesky sign-in…".into(),
            ));
            #[cfg(target_os = "android")]
            {
                // freeq:// deep link (SleekActivity intent-filter) — UI polls
                // take_pending_deep_link and applies tokens; no loopback server.
                if let Err(e) = auth::bluesky_login_mobile(&auth_broker, &bsky_handle, |msg| {
                    let _ = event_tx.send(NetEvent::Status(msg));
                })
                .await
                {
                    let _ = event_tx.send(NetEvent::Failed(format!("Sign-in failed: {e}")));
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                match auth::bluesky_login_loopback(
                    &auth_broker,
                    &bsky_handle,
                    Duration::from_secs(5 * 60),
                    |msg| {
                        let _ = event_tx.send(NetEvent::Status(msg));
                    },
                )
                .await
                {
                    Ok(tokens) => {
                        let _ = event_tx.send(NetEvent::Status("Sign-in complete".into()));
                        let _ = event_tx.send(NetEvent::AuthReady(tokens));
                    }
                    Err(e) => {
                        let _ = event_tx.send(NetEvent::Failed(format!("Sign-in failed: {e}")));
                    }
                }
            }
        }
        NetCmd::ReconnectSession {
            broker_token,
            auth_broker,
        } => {
            // Mint a fresh web-token; UI applies AuthReady and issues Connect.
            let _ = event_tx.send(NetEvent::Status("Refreshing session…".into()));
            match auth::fetch_broker_session(&auth_broker, &broker_token).await {
                Ok(tokens) => {
                    let _ = event_tx.send(NetEvent::AuthReady(tokens));
                }
                Err(e) => {
                    let _ = event_tx.send(NetEvent::Failed(format!("Session refresh failed: {e}")));
                }
            }
        }
        NetCmd::Connect {
            nick,
            server,
            tls,
            websocket,
            auto_join,
            web_token,
        } => {
            do_connect(
                handle,
                events,
                pending_joins,
                event_tx,
                ConnectArgs {
                    nick,
                    server,
                    tls,
                    websocket,
                    auto_join,
                    web_token,
                },
            )
            .await;
        }
        NetCmd::Join(channel) => {
            if let Some(h) = handle {
                if let Err(e) = h.join(&channel).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Join {channel}: {e}")));
                }
            } else {
                pending_joins.push(channel);
            }
        }
        NetCmd::Part(channel) => {
            if let Some(h) = handle {
                let line = format!("PART {channel}");
                if let Err(e) = h.raw(&line).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Part {channel}: {e}")));
                }
            }
        }
        NetCmd::Privmsg { target, text } => {
            if let Some(h) = handle {
                if let Err(e) = h.privmsg(&target, &text).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Send failed: {e}")));
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
            }
        }
        NetCmd::Raw(line) => {
            if let Some(h) = handle {
                if let Err(e) = h.raw(&line).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Raw send failed: {e}")));
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
            }
        }
        NetCmd::React {
            target,
            emoji,
            msgid,
        } => {
            if let Some(h) = handle {
                if let Err(e) = h.react(&target, &emoji, &msgid).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("React failed: {e}")));
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
            }
        }
        NetCmd::Unreact {
            target,
            emoji,
            msgid,
        } => {
            if let Some(h) = handle {
                let mut tags = std::collections::HashMap::new();
                tags.insert("+freeq.at/unreact".to_string(), emoji);
                tags.insert("+reply".to_string(), msgid);
                if let Err(e) = h.send_tagmsg(&target, tags).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Unreact failed: {e}")));
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
            }
        }
        NetCmd::EditMessage {
            target,
            msgid,
            text,
        } => {
            if let Some(h) = handle {
                if let Err(e) = h.edit_message(&target, &msgid, &text).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Edit failed: {e}")));
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
            }
        }
        NetCmd::DeleteMessage { target, msgid } => {
            if let Some(h) = handle {
                if let Err(e) = h.delete_message(&target, &msgid).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("Delete failed: {e}")));
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
            }
        }
        NetCmd::UploadAndSend {
            upload_id,
            target,
            caption,
            bytes,
            content_type,
            did,
            api_base,
        } => {
            if handle.is_none() {
                let _ = event_tx.send(NetEvent::UploadFinished {
                    upload_id,
                    error: Some("Not connected".into()),
                    sent: None,
                });
                return;
            }
            match upload_media(&api_base, &did, &target, &caption, &content_type, bytes).await {
                Ok(url) => {
                    let text = if caption.trim().is_empty() {
                        url
                    } else {
                        format!("{url} {}", caption.trim())
                    };
                    if let Some(h) = handle {
                        if let Err(e) = h.privmsg(&target, &text).await {
                            let _ = event_tx.send(NetEvent::UploadFinished {
                                upload_id,
                                error: Some(format!("Upload ok, send failed: {e}")),
                                sent: None,
                            });
                            return;
                        }
                    }
                    let _ = event_tx.send(NetEvent::UploadFinished {
                        upload_id,
                        error: None,
                        sent: Some(MediaSent { target, text }),
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(NetEvent::UploadFinished {
                        upload_id,
                        error: Some(e),
                        sent: None,
                    });
                }
            }
        }
        NetCmd::HistoryLatest { target, count } => {
            if let Some(h) = handle {
                if let Err(e) = h.history_latest(&target, count).await {
                    log::warn!("history_latest {target}: {e}");
                }
            }
        }
        NetCmd::HistoryTargets { limit } => {
            if let Some(h) = handle {
                if let Err(e) = h.chathistory_targets(limit).await {
                    log::warn!("chathistory_targets: {e}");
                }
            }
        }
        NetCmd::FetchImage { url } => {
            // Fire-and-forget so IRC event loop stays responsive.
            let tx = event_tx.clone();
            tokio::spawn(async move {
                match fetch_image_bytes(&url).await {
                    Ok((width, height, rgba)) => {
                        let _ = tx.send(NetEvent::ImageFetched {
                            url,
                            width,
                            height,
                            rgba,
                        });
                    }
                    Err(e) => {
                        log::debug!("image fetch {url}: {e}");
                        let _ = tx.send(NetEvent::ImageFetchFailed { url });
                    }
                }
            });
        }
        NetCmd::FetchLinkPreview { url } => {
            let tx = event_tx.clone();
            tokio::spawn(async move {
                match freeq_sdk::media::fetch_link_preview(&url).await {
                    Ok(preview) => {
                        // freeq-sdk flake pin may omit `site_name` on LinkPreview;
                        // host label is optional UI chrome only.
                        let _ = tx.send(NetEvent::LinkPreviewFetched {
                            url,
                            title: preview.title,
                            description: preview.description,
                            thumb_url: preview.thumb_url,
                            site_name: None,
                        });
                    }
                    Err(e) => {
                        log::debug!("link preview {url}: {e}");
                        let _ = tx.send(NetEvent::LinkPreviewFailed { url });
                    }
                }
            });
        }
        NetCmd::AvStart { channel } => {
            let Some(h) = handle else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
                return;
            };
            let instance = new_av_instance();
            match h.av_start(&channel, &instance, None).await {
                Ok(()) => {
                    let _ = event_tx.send(NetEvent::AvSignalingSent {
                        channel,
                        session_id: None,
                        instance,
                        started: true,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(NetEvent::Failed(format!("av-start: {e}")));
                }
            }
        }
        NetCmd::AvJoin {
            channel,
            session_id,
        } => {
            let Some(h) = handle else {
                let _ = event_tx.send(NetEvent::Failed("Not connected".into()));
                return;
            };
            let instance = new_av_instance();
            match h.av_join(&channel, &session_id, &instance).await {
                Ok(()) => {
                    let _ = event_tx.send(NetEvent::AvSignalingSent {
                        channel,
                        session_id: Some(session_id),
                        instance,
                        started: false,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(NetEvent::Failed(format!("av-join: {e}")));
                }
            }
        }
        NetCmd::AvLeave {
            channel,
            session_id,
            instance,
        } => {
            media.stop().await;
            let _ = event_tx.send(NetEvent::AvMediaStatus {
                status: crate::av::MediaStatus::Idle,
                video: None,
                has_camera: false,
                mic_level: None,
                has_mic: false,
            });
            if let Some(h) = handle {
                if let Err(e) = h.av_leave(&channel, &session_id, &instance).await {
                    let _ = event_tx.send(NetEvent::Failed(format!("av-leave: {e}")));
                }
            }
        }
        NetCmd::AvMediaConnect {
            sfu_url,
            session_id,
            nick,
            instance,
            muted,
            speaker_muted,
            camera,
            camera_id,
            mic_id,
            speaker_id,
        } => {
            media.stop().await;
            let url: url::Url = match sfu_url.parse() {
                Ok(u) => u,
                Err(e) => {
                    let _ = event_tx.send(NetEvent::AvMediaStatus {
                        status: crate::av::MediaStatus::Failed(format!("bad SFU URL: {e}")),
                        video: None,
                        has_camera: false,
                        mic_level: None,
                        has_mic: false,
                    });
                    return;
                }
            };
            media.muted = Arc::new(AtomicBool::new(muted));
            media.speaker_muted = Arc::new(AtomicBool::new(speaker_muted));
            let tx = event_tx.clone();
            let session = AvMediaSession::start(
                AvMediaConfig {
                    sfu_url: url,
                    session_id,
                    nick,
                    instance,
                    muted,
                    speaker_muted,
                    camera_enabled: camera,
                    camera_id,
                    mic_id,
                    speaker_id,
                },
                move |update| {
                    use crate::av_media::AvMediaUpdate;
                    let (status, video, has_camera, mic_level, has_mic) = match update {
                        AvMediaUpdate::Live {
                            video,
                            has_camera,
                            mic_level,
                            has_mic,
                        } => (
                            crate::av::MediaStatus::Live,
                            Some(video),
                            has_camera,
                            Some(mic_level),
                            has_mic,
                        ),
                        AvMediaUpdate::Ended => {
                            (crate::av::MediaStatus::Idle, None, false, None, false)
                        }
                        AvMediaUpdate::Failed(e) => {
                            (crate::av::MediaStatus::Failed(e), None, false, None, false)
                        }
                    };
                    let _ = tx.send(NetEvent::AvMediaStatus {
                        status,
                        video,
                        has_camera,
                        mic_level,
                        has_mic,
                    });
                },
            );
            // Attach the live frame store immediately while still Connecting so
            // local preview frames can paint before MoQ Live (camera opens
            // before the SFU handshake finishes).
            let _ = event_tx.send(NetEvent::AvMediaStatus {
                status: crate::av::MediaStatus::Connecting,
                video: Some(session.video.clone()),
                has_camera: false,
                mic_level: Some(session.mic_level.clone()),
                has_mic: false,
            });
            media.session = Some(session);
        }
        NetCmd::AvMediaStop => {
            media.stop().await;
            let _ = event_tx.send(NetEvent::AvMediaStatus {
                status: crate::av::MediaStatus::Idle,
                video: None,
                has_camera: false,
                mic_level: None,
                has_mic: false,
            });
        }
        NetCmd::AvMute { muted } => {
            media.set_muted(muted);
        }
        NetCmd::AvSpeakerMute { muted } => {
            media.set_speaker_muted(muted);
        }
        NetCmd::AvCamera { enabled } => {
            media.set_camera(enabled);
        }
        NetCmd::AvCameraDevice { id } => {
            media.set_camera_device(id);
        }
        NetCmd::AvMicDevice { id } => {
            media.set_mic_device(id);
        }
        NetCmd::AvSpeakerDevice { id } => {
            media.set_speaker_device(id);
        }
        NetCmd::SearchHandles { request_id, query } => {
            let tx = event_tx.clone();
            tokio::spawn(async move {
                match bsky::search_actors_typeahead(&query, bsky::TYPEAHEAD_LIMIT).await {
                    Ok(actors) => {
                        let _ = tx.send(NetEvent::HandleSuggestions {
                            request_id,
                            query,
                            actors,
                            error: None,
                        });
                    }
                    Err(e) => {
                        log::debug!("handle typeahead {query}: {e}");
                        let _ = tx.send(NetEvent::HandleSuggestions {
                            request_id,
                            query,
                            actors: Vec::new(),
                            error: Some(e.to_string()),
                        });
                    }
                }
            });
        }
        NetCmd::Quit => {
            media.stop().await;
            if let Some(h) = handle.take() {
                let _ = h.quit(Some("Sleek quit")).await;
            }
            *events = None;
            pending_joins.clear();
            let _ = event_tx.send(NetEvent::Sdk(Event::Disconnected {
                reason: "quit".into(),
            }));
        }
    }
}

struct ConnectArgs {
    nick: String,
    server: String,
    tls: bool,
    websocket: bool,
    auto_join: Vec<String>,
    web_token: Option<String>,
}

async fn do_connect(
    handle: &mut Option<ClientHandle>,
    events: &mut Option<mpsc::Receiver<Event>>,
    pending_joins: &mut Vec<String>,
    event_tx: &std::sync::mpsc::Sender<NetEvent>,
    args: ConnectArgs,
) {
    // Tear down any prior session.
    if let Some(h) = handle.take() {
        let _ = h.quit(Some("reconnecting")).await;
    }
    *events = None;
    *pending_joins = args.auto_join;

    let use_ws = args.websocket || prefer_websocket(&args.server);
    let ws_url = if use_ws {
        Some(websocket_url_for(&args.server))
    } else {
        None
    };

    let config = ConnectConfig {
        server_addr: args.server.clone(),
        nick: args.nick.clone(),
        user: args.nick.clone(),
        realname: "Sleek freeq client".into(),
        tls: if ws_url.is_some() { false } else { args.tls },
        tls_insecure: false,
        web_token: args.web_token.clone(),
        websocket_url: ws_url.clone(),
    };

    let via = if let Some(ref u) = ws_url {
        format!("via {u}")
    } else if args.tls {
        format!("TLS {}", args.server)
    } else {
        format!("TCP {}", args.server)
    };
    let auth_note = if args.web_token.is_some() {
        " (SASL)"
    } else {
        " (guest)"
    };
    let _ = event_tx.send(NetEvent::Status(format!(
        "Connecting to {via} as {}{auth_note}…",
        args.nick
    )));

    match client::establish_connection(&config).await {
        Ok(conn) => {
            let (h, rx) = client::connect_with_stream(conn, config, None);
            *handle = Some(h);
            *events = Some(rx);
            let _ = event_tx.send(NetEvent::Status("Socket up — registering…".into()));
        }
        Err(e) => {
            // If TLS TCP failed and we didn't try WS, retry WSS once.
            if ws_url.is_none() && args.tls {
                let _ = event_tx.send(NetEvent::Status(format!(
                    "TCP failed ({e}); retrying WebSocket…"
                )));
                let cfg = ConnectConfig {
                    server_addr: args.server.clone(),
                    nick: args.nick.clone(),
                    user: args.nick.clone(),
                    realname: "Sleek freeq client".into(),
                    tls: false,
                    tls_insecure: false,
                    web_token: args.web_token,
                    websocket_url: Some(websocket_url_for(&args.server)),
                };
                match client::establish_connection(&cfg).await {
                    Ok(conn) => {
                        let (h, rx) = client::connect_with_stream(conn, cfg, None);
                        *handle = Some(h);
                        *events = Some(rx);
                        let _ =
                            event_tx.send(NetEvent::Status("WebSocket up — registering…".into()));
                    }
                    Err(e2) => {
                        let _ = event_tx.send(NetEvent::Failed(format!("Connect failed: {e2}")));
                    }
                }
            } else {
                let _ = event_tx.send(NetEvent::Failed(format!("Connect failed: {e}")));
            }
        }
    }
}

/// POST multipart image to freeq `/api/v1/upload` (private media; needs active DID session).
async fn upload_media(
    api_base: &str,
    did: &str,
    channel: &str,
    alt: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("Empty image".into());
    }
    if bytes.len() > 10 * 1024 * 1024 {
        return Err("Image is too large (max 10MB)".into());
    }

    let base = api_base.trim_end_matches('/');
    let url = format!("{base}/api/v1/upload");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let file_name = match content_type {
        "image/jpeg" => "paste.jpg",
        "image/gif" => "paste.gif",
        "image/webp" => "paste.webp",
        _ => "paste.png",
    };

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(file_name.to_string())
        .mime_str(content_type)
        .map_err(|e| format!("multipart: {e}"))?;

    let mut form = reqwest::multipart::Form::new()
        .text("did", did.to_string())
        .text("channel", channel.to_string())
        .part("file", part);
    if !alt.trim().is_empty() {
        form = form.text("alt", alt.trim().to_string());
    }

    let resp = client
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Upload request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let short = body.chars().take(120).collect::<String>();
        let msg = match status.as_u16() {
            401 => "Not authorized — stay signed in and connected, then try again".into(),
            413 => "Image is too large".into(),
            _ => format!("Upload failed ({status}) {short}").trim().to_string(),
        };
        return Err(msg);
    }

    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Upload response: {e}"))?;
    v.get("url")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Upload response missing url".into())
}

/// Fetch and decode a remote image for chat inline preview (SSRF-safe).
async fn fetch_image_bytes(url: &str) -> Result<(usize, usize, std::sync::Arc<[u8]>), String> {
    use crate::preview::{MAX_IMAGE_BYTES, MAX_IMAGE_DIM};

    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only http(s) image URLs".into());
    }

    let parsed = url::Url::parse(url).map_err(|e| format!("Bad URL: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_string();
    let port = parsed
        .port()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });

    let addrs = freeq_sdk::ssrf::resolve_and_check(&host, port)
        .await
        .map_err(|e| format!("SSRF check: {e}"))?;

    // DNS-pin + limited redirects (CDN image hosts often 302).
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(5));
    for addr in &addrs {
        builder = builder.resolve(&host, *addr);
    }
    let client = builder.build().map_err(|e| format!("HTTP client: {e}"))?;

    let resp = client
        .get(url)
        .header("Accept", "image/*,*/*;q=0.8")
        .header("User-Agent", "Sleek/0.1 (chat image preview)")
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("HTTP error: {e}"))?;

    if let Some(len) = resp.content_length() {
        if len > MAX_IMAGE_BYTES as u64 {
            return Err("Image too large".into());
        }
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Body: {e}"))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("Image too large".into());
    }

    let dyn_img = image::load_from_memory(&bytes).map_err(|e| format!("Decode: {e}"))?;
    let dyn_img = if dyn_img.width() > MAX_IMAGE_DIM || dyn_img.height() > MAX_IMAGE_DIM {
        dyn_img.thumbnail(MAX_IMAGE_DIM, MAX_IMAGE_DIM)
    } else {
        dyn_img
    };
    let rgba = dyn_img.to_rgba8();
    let width = rgba.width() as usize;
    let height = rgba.height() as usize;
    Ok((width, height, std::sync::Arc::from(rgba.into_raw())))
}
