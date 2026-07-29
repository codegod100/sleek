//! Async freeq-sdk bridge: background tokio runtime ↔ egui UI thread.

use std::thread;

use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;
use tokio::sync::mpsc;

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
        NetCmd::Connect {
            nick,
            server,
            tls,
            websocket,
            auto_join,
        } => {
            // Tear down any prior session.
            if let Some(h) = handle.take() {
                let _ = h.quit(Some("reconnecting")).await;
            }
            *events = None;
            *pending_joins = auto_join;

            let use_ws = websocket || prefer_websocket(&server);
            let ws_url = if use_ws {
                Some(websocket_url_for(&server))
            } else {
                None
            };

            let config = ConnectConfig {
                server_addr: server.clone(),
                nick: nick.clone(),
                user: nick.clone(),
                realname: "Sleek freeq client".into(),
                tls: if ws_url.is_some() { false } else { tls },
                tls_insecure: false,
                web_token: None,
                websocket_url: ws_url.clone(),
            };

            let via = if let Some(ref u) = ws_url {
                format!("via {u}")
            } else if tls {
                format!("TLS {server}")
            } else {
                format!("TCP {server}")
            };
            let _ = event_tx.send(NetEvent::Status(format!("Connecting to {via} as {nick}…")));

            match client::establish_connection(&config).await {
                Ok(conn) => {
                    let (h, rx) = client::connect_with_stream(conn, config, None);
                    *handle = Some(h);
                    *events = Some(rx);
                    let _ = event_tx.send(NetEvent::Status("Socket up — registering…".into()));
                }
                Err(e) => {
                    // If TLS TCP failed and we didn't try WS, retry WSS once.
                    if ws_url.is_none() && tls {
                        let _ = event_tx.send(NetEvent::Status(format!(
                            "TCP failed ({e}); retrying WebSocket…"
                        )));
                        let cfg = ConnectConfig {
                            server_addr: server.clone(),
                            nick: nick.clone(),
                            user: nick.clone(),
                            realname: "Sleek freeq client".into(),
                            tls: false,
                            tls_insecure: false,
                            web_token: None,
                            websocket_url: Some(websocket_url_for(&server)),
                        };
                        match client::establish_connection(&cfg).await {
                            Ok(conn) => {
                                let (h, rx) = client::connect_with_stream(conn, cfg, None);
                                *handle = Some(h);
                                *events = Some(rx);
                                let _ = event_tx
                                    .send(NetEvent::Status("WebSocket up — registering…".into()));
                            }
                            Err(e2) => {
                                let _ = event_tx
                                    .send(NetEvent::Failed(format!("Connect failed: {e2}")));
                            }
                        }
                    } else {
                        let _ = event_tx.send(NetEvent::Failed(format!("Connect failed: {e}")));
                    }
                }
            }
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
