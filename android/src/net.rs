//! Async freeq-sdk bridge: background tokio runtime ↔ egui UI thread.

use std::thread;
use std::time::Duration;

use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;
use tokio::sync::mpsc;

use crate::auth::{self, AuthTokens};
use crate::state::{prefer_websocket, websocket_url_for};

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
                        &event_tx,
                    )
                    .await;
                }
                None => break,
            }
        }
    }
}

async fn apply_cmd(
    cmd: NetCmd,
    handle: &mut Option<ClientHandle>,
    events: &mut Option<mpsc::Receiver<Event>>,
    pending_joins: &mut Vec<String>,
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
            match auth::bluesky_login_loopback(
                &auth_broker,
                &bsky_handle,
                Duration::from_secs(5 * 60),
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
        NetCmd::Quit => {
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
