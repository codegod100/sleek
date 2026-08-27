//! Relm4 desktop frontend.
//!
//! This intentionally shares Sleek's existing network bridge while the UI is
//! migrated screen by screen. The Android/egui frontend remains available as
//! `sleek-egui` during the transition.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use freeq_sdk::av::{parse_av_state, AvAction};
use freeq_sdk::event::Event;
use libadwaita as adw;
use adw::prelude::*;
use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmApp};
use sleek::net::{NetBridge, NetCmd, NetEvent};

struct App {
    net: NetBridge,
    connected: bool,
    nick: String,
    server: String,
    channels: Vec<String>,
    messages: HashMap<String, Vec<ChatLine>>,
    members: HashMap<String, Vec<String>>,
    active_channel: Option<String>,
    channel_calls: HashMap<String, ChannelCall>,
    local_call: Option<LocalCall>,
    video: Option<sleek::av::VideoFrameStore>,
    video_generations: HashMap<String, u64>,
}

struct ChatLine {
    id: String,
    from: String,
    text: String,
    reactions: HashMap<String, HashSet<String>>,
}

struct ChannelCall {
    session_id: String,
    participants: u32,
}

struct LocalCall {
    channel: String,
    session_id: String,
    instance: String,
    muted: bool,
    speaker_muted: bool,
    camera: bool,
    media_started: bool,
}

#[derive(Debug)]
enum Input {
    Connect {
        nick: String,
        server: String,
    },
    Disconnect,
    SelectChannel(String),
    SendMessage(String),
    ToggleCall,
    ToggleMute,
    ToggleSpeaker,
    ToggleCamera,
    React {
        target: String,
        msgid: String,
        emoji: String,
    },
    Tick,
}

struct Widgets {
    stack: gtk::Stack,
    header: gtk::HeaderBar,
    status: gtk::Label,
    heading: gtk::Label,
    channel_list: gtk::Box,
    user_list: gtk::ListBox,
    message_list: gtk::ListBox,
    message_scroll: gtk::ScrolledWindow,
    video_grid: gtk::FlowBox,
    compose: gtk::Entry,
    call_button: gtk::Button,
    call_bar: gtk::Box,
    call_status: gtk::Label,
    mute_button: gtk::Button,
    speaker_button: gtk::Button,
    camera_button: gtk::Button,
}

impl Component for App {
    type CommandOutput = ();
    type Init = ();
    type Input = Input;
    type Output = ();
    type Root = gtk::ApplicationWindow;
    type Widgets = Widgets;

    fn init_root() -> Self::Root {
        gtk::ApplicationWindow::builder()
            .title("Sleek")
            .default_width(960)
            .default_height(680)
            .build()
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();

        let connect = gtk::Box::new(gtk::Orientation::Vertical, 12);
        connect.add_css_class("card");
        connect.set_halign(gtk::Align::Center);
        connect.set_valign(gtk::Align::Center);
        connect.set_width_request(360);

        let title = gtk::Label::new(Some("Sleek"));
        title.add_css_class("title-1");
        let subtitle = gtk::Label::new(Some("A freeq chat client"));
        subtitle.add_css_class("dim-label");
        let nick = gtk::Entry::builder()
            .placeholder_text("Nickname")
            .text(default_nick())
            .build();
        let server = gtk::Entry::builder()
            .placeholder_text("Server")
            .text("irc.freeq.at:6697")
            .build();
        let connect_button = gtk::Button::with_label("Continue as guest");
        connect_button.add_css_class("suggested-action");

        connect.append(&title);
        connect.append(&subtitle);
        connect.append(&nick);
        connect.append(&server);
        connect.append(&connect_button);
        let connect_clamp = adw::Clamp::builder()
            .maximum_size(440)
            .tightening_threshold(360)
            .child(&connect)
            .build();
        stack.add_named(&connect_clamp, Some("connect"));

        let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::HeaderBar::new();
        let heading = gtk::Label::new(Some("Chats"));
        heading.add_css_class("title-2");
        header.set_title_widget(Some(&heading));
        let disconnect = gtk::Button::with_label("Disconnect");
        disconnect.add_css_class("flat");
        header.pack_end(&disconnect);
        let call_button = gtk::Button::with_label("Start Call");
        call_button.add_css_class("suggested-action");
        header.pack_start(&call_button);
        let status = gtk::Label::new(Some("Connecting…"));
        status.set_margin_top(6);
        status.set_margin_bottom(6);
        status.add_css_class("dim-label");

        let channel_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        channel_list.set_width_request(220);

        let channel_scroll = gtk::ScrolledWindow::builder()
            .child(&channel_list)
            .vexpand(true)
            .build();

        let message_list = gtk::ListBox::new();
        message_list.set_selection_mode(gtk::SelectionMode::None);
        message_list.add_css_class("boxed-list");
        let message_scroll = gtk::ScrolledWindow::builder()
            .child(&message_list)
            .hexpand(true)
            .vexpand(true)
            .build();
        message_scroll.add_css_class("view");

        let compose = gtk::Entry::builder()
            .placeholder_text("Message")
            .hexpand(true)
            .build();
        let send_button = gtk::Button::with_label("Send");
        send_button.add_css_class("suggested-action");
        let compose_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        compose_row.set_margin_top(12);
        compose_row.set_margin_bottom(12);
        compose_row.set_margin_start(12);
        compose_row.set_margin_end(12);
        compose_row.append(&compose);
        compose_row.append(&send_button);

        let conversation = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let video_grid = gtk::FlowBox::builder()
            .column_spacing(8)
            .row_spacing(8)
            .max_children_per_line(3)
            .selection_mode(gtk::SelectionMode::None)
            .build();
        video_grid.set_margin_top(8);
        video_grid.set_margin_bottom(8);
        video_grid.set_margin_start(12);
        video_grid.set_margin_end(12);
        video_grid.set_visible(false);
        conversation.append(&video_grid);
        conversation.append(&message_scroll);

        let call_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        call_bar.set_margin_top(8);
        call_bar.set_margin_bottom(8);
        call_bar.set_margin_start(12);
        call_bar.set_margin_end(12);
        call_bar.add_css_class("toolbar");
        call_bar.set_visible(false);
        let call_status = gtk::Label::new(Some("Connecting media…"));
        call_status.set_hexpand(true);
        call_status.set_halign(gtk::Align::Start);
        let mute_button = gtk::Button::with_label("Mute");
        let speaker_button = gtk::Button::with_label("Mute Speaker");
        let camera_button = gtk::Button::with_label("Camera On");
        let hangup_button = gtk::Button::with_label("Leave");
        hangup_button.add_css_class("destructive-action");
        call_bar.append(&call_status);
        call_bar.append(&mute_button);
        call_bar.append(&speaker_button);
        call_bar.append(&camera_button);
        call_bar.append(&hangup_button);
        conversation.append(&call_bar);
        conversation.append(&compose_row);

        let user_list = gtk::ListBox::new();
        user_list.set_selection_mode(gtk::SelectionMode::None);
        user_list.add_css_class("navigation-sidebar");
        let user_scroll = gtk::ScrolledWindow::builder()
            .child(&user_list)
            .vexpand(true)
            .width_request(180)
            .build();

        let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        channel_scroll.set_width_request(220);
        layout.append(&channel_scroll);
        layout.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        conversation.set_hexpand(true);
        layout.append(&conversation);
        layout.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        layout.append(&user_scroll);

        shell.append(&status);
        shell.append(&layout);
        stack.add_named(&shell, Some("shell"));

        root.set_child(Some(&stack));

        connect_button.connect_clicked({
            let sender = sender.clone();
            move |_| {
                sender.input(Input::Connect {
                    nick: nick.text().to_string(),
                    server: server.text().to_string(),
                });
            }
        });
        disconnect.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::Disconnect)
        });
        send_button.connect_clicked({
            let sender = sender.clone();
            let compose = compose.clone();
            move |_| {
                sender.input(Input::SendMessage(compose.text().to_string()));
            }
        });
        compose.connect_activate({
            let sender = sender.clone();
            let compose = compose.clone();
            move |_| {
                sender.input(Input::SendMessage(compose.text().to_string()));
            }
        });
        call_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleCall)
        });
        hangup_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleCall)
        });
        mute_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleMute)
        });
        speaker_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleSpeaker)
        });
        camera_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleCamera)
        });

        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            sender.input(Input::Tick);
            gtk::glib::ControlFlow::Continue
        });

        let model = App {
            net: NetBridge::start(),
            connected: false,
            nick: String::new(),
            server: String::new(),
            channels: Vec::new(),
            messages: HashMap::new(),
            members: HashMap::new(),
            active_channel: None,
            channel_calls: HashMap::new(),
            local_call: None,
            video: None,
            video_generations: HashMap::new(),
        };
        let widgets = Widgets {
            stack,
            header,
            status,
            heading,
            channel_list,
            user_list,
            message_list,
            message_scroll,
            video_grid,
            compose,
            call_button,
            call_bar,
            call_status,
            mute_button,
            speaker_button,
            camera_button,
        };
        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Widgets,
        message: Input,
        _sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match message {
            Input::Connect { nick, server } => {
                if nick.trim().is_empty() || server.trim().is_empty() {
                    widgets.status.set_text("Nickname and server are required");
                    return;
                }
                self.connected = true;
                self.nick = nick.clone();
                self.server = server.clone();
                self.channels.clear();
                self.messages.clear();
                self.members.clear();
                self.active_channel = None;
                self.channel_calls.clear();
                self.local_call = None;
                self.video = None;
                self.video_generations.clear();
                root.set_titlebar(Some(&widgets.header));
                widgets.stack.set_visible_child_name("shell");
                widgets.status.set_text("Connecting…");
                self.net.send(NetCmd::Connect {
                    nick,
                    server,
                    tls: true,
                    websocket: false,
                    auto_join: vec!["#general".into(), "#test".into()],
                    web_token: None,
                });
            }
            Input::Disconnect => {
                self.net.send(NetCmd::Quit);
                self.net = NetBridge::start();
                self.connected = false;
                self.channels.clear();
                self.messages.clear();
                self.members.clear();
                self.active_channel = None;
                self.channel_calls.clear();
                self.local_call = None;
                self.video = None;
                self.video_generations.clear();
                clear_flow_box(&widgets.video_grid);
                widgets.video_grid.set_visible(false);
                widgets.call_bar.set_visible(false);
                root.set_titlebar(None::<&gtk::Widget>);
                widgets.stack.set_visible_child_name("connect");
            }
            Input::SelectChannel(channel) => {
                self.active_channel = Some(channel);
                self.render_channels(widgets, &_sender);
                self.render_messages(widgets, &_sender);
                self.render_users(widgets);
                self.render_call_controls(widgets);
                widgets.compose.grab_focus();
            }
            Input::SendMessage(text) => {
                let text = text.trim();
                if text.is_empty() {
                    return;
                }
                let Some(target) = self.active_channel.clone() else {
                    widgets.status.set_text("Select a chat before sending");
                    return;
                };
                self.net.send(NetCmd::Privmsg {
                    target,
                    text: text.to_owned(),
                });
                widgets.compose.set_text("");
            }
            Input::ToggleCall => self.toggle_call(widgets),
            Input::ToggleMute => {
                if let Some(call) = self.local_call.as_mut() {
                    call.muted = !call.muted;
                    self.net.send(NetCmd::AvMute { muted: call.muted });
                    self.render_call_controls(widgets);
                }
            }
            Input::ToggleSpeaker => {
                if let Some(call) = self.local_call.as_mut() {
                    call.speaker_muted = !call.speaker_muted;
                    self.net.send(NetCmd::AvSpeakerMute {
                        muted: call.speaker_muted,
                    });
                    self.render_call_controls(widgets);
                }
            }
            Input::ToggleCamera => {
                if let Some(call) = self.local_call.as_mut() {
                    call.camera = !call.camera;
                    self.net.send(NetCmd::AvCamera {
                        enabled: call.camera,
                    });
                    self.render_call_controls(widgets);
                }
            }
            Input::React {
                target,
                msgid,
                emoji,
            } => {
                let reacted = self
                    .messages
                    .get(&target)
                    .and_then(|messages| messages.iter().find(|message| message.id == msgid))
                    .and_then(|message| message.reactions.get(&emoji))
                    .is_some_and(|nicks| {
                        nicks.iter().any(|nick| nick.eq_ignore_ascii_case(&self.nick))
                    });
                if reacted {
                    self.net.send(NetCmd::Unreact {
                        target: target.clone(),
                        emoji: emoji.clone(),
                        msgid: msgid.clone(),
                    });
                } else {
                    self.net.send(NetCmd::React {
                        target: target.clone(),
                        emoji: emoji.clone(),
                        msgid: msgid.clone(),
                    });
                }
                let nick = self.nick.clone();
                self.apply_reaction(&target, &msgid, &emoji, &nick, !reacted);
                self.render_messages(widgets, &_sender);
            }
            Input::Tick => {
                let mut refresh_chat = false;
                for event in self.net.poll() {
                    match event {
                        NetEvent::Status(message) => widgets.status.set_text(&message),
                        NetEvent::Failed(error) => {
                            widgets.status.set_text(&format!("Connection failed: {error}"))
                        }
                        NetEvent::Sdk(event) if self.connected => {
                            refresh_chat = true;
                            self.handle_sdk_event(event, widgets, &_sender)
                        }
                        NetEvent::AvSignalingSent {
                            channel,
                            session_id,
                            instance,
                            ..
                        } => {
                            if let Some(call) = self.local_call.as_mut() {
                                if call.channel == channel {
                                    call.instance = instance;
                                    if let Some(session_id) = session_id {
                                        call.session_id = session_id;
                                    }
                                }
                            }
                            self.try_start_media(widgets);
                        }
                        NetEvent::AvMediaStatus {
                            status,
                            video,
                            has_camera,
                            has_mic,
                            ..
                        } => {
                            widgets.call_status.set_text(&status.label());
                            widgets.camera_button.set_sensitive(has_camera);
                            widgets.mute_button.set_sensitive(has_mic);
                            if let Some(video) = video {
                                self.video = Some(video);
                            }
                        }
                        _ => {}
                    }
                }
                if refresh_chat {
                    self.render_channels(widgets, &_sender);
                    self.render_messages(widgets, &_sender);
                    self.render_users(widgets);
                    self.render_call_controls(widgets);
                }
                self.render_video(widgets);
            }
        }
    }
}

impl App {
    fn handle_sdk_event(
        &mut self,
        event: Event,
        widgets: &mut Widgets,
        sender: &ComponentSender<Self>,
    ) {
        match event {
            Event::Connected => widgets.status.set_text("Connected"),
            Event::Registered { nick } => {
                self.nick = nick.clone();
                widgets.status.set_text(&format!("Online as {nick}"));
            }
            Event::Joined { channel, nick, .. } => {
                self.add_member(&channel, &nick);
                if nick.eq_ignore_ascii_case(&self.nick) {
                    self.ensure_channel(&channel);
                    self.net.send(NetCmd::HistoryLatest {
                        target: channel.clone(),
                        count: 100,
                    });
                    if self.active_channel.is_none() {
                        self.active_channel = Some(channel.clone());
                    }
                    self.render_channels(widgets, sender);
                    self.render_messages(widgets, sender);
                }
                if self.active_channel.as_deref() == Some(channel.as_str()) {
                    self.render_users(widgets);
                }
            }
            Event::Parted { channel, nick } => {
                self.remove_member(&channel, &nick);
                if nick.eq_ignore_ascii_case(&self.nick) {
                    self.channels.retain(|item| item != &channel);
                    self.messages.remove(&channel);
                    self.members.remove(&channel);
                    if self.active_channel.as_deref() == Some(channel.as_str()) {
                        self.active_channel = self.channels.first().cloned();
                    }
                    self.render_channels(widgets, sender);
                    self.render_messages(widgets, sender);
                }
                self.render_users(widgets);
            }
            Event::Names { channel, nicks } => {
                for nick in nicks {
                    self.add_member(&channel, clean_nick(&nick));
                }
            }
            Event::Kicked { channel, nick, .. } => {
                self.remove_member(&channel, &nick);
                self.render_users(widgets);
            }
            Event::NickChanged { old_nick, new_nick } => {
                for members in self.members.values_mut() {
                    if let Some(nick) = members
                        .iter_mut()
                        .find(|nick| nick.eq_ignore_ascii_case(&old_nick))
                    {
                        *nick = new_nick.clone();
                    }
                }
                if self.nick.eq_ignore_ascii_case(&old_nick) {
                    self.nick = new_nick;
                }
                self.render_users(widgets);
            }
            Event::UserQuit { nick, .. } => {
                for members in self.members.values_mut() {
                    members.retain(|member| !member.eq_ignore_ascii_case(&nick));
                }
                self.render_users(widgets);
            }
            Event::Message {
                from,
                target,
                text,
                dm_key,
                tags,
            } => {
                let channel = if let Some(dm_key) = dm_key {
                    dm_key
                } else if target.starts_with('#') || target.starts_with('&') {
                    target
                } else if target.eq_ignore_ascii_case(&self.nick) {
                    from.clone()
                } else {
                    target
                };
                self.ensure_channel(&channel);
                let id = message_id(&tags);
                let reactions = tags
                    .get("+freeq.at/reactions")
                    .map(|value| parse_reactions(value))
                    .unwrap_or_default();
                self.push_message(&channel, id, from, text, reactions);
            }
            Event::TagMsg {
                from, target, tags, ..
            } => {
                if let Some((msgid, emoji, add)) = reaction_update(&tags) {
                    self.apply_reaction(&target, &msgid, &emoji, &from, add);
                }
                if let Some(state) = parse_av_state(&tags) {
                    match state.action {
                        AvAction::Started | AvAction::Joined => {
                            self.channel_calls.insert(
                                target.clone(),
                                ChannelCall {
                                    session_id: state.session_id.clone(),
                                    participants: state.participants.unwrap_or(1),
                                },
                            );
                            if state
                                .actor
                                .as_deref()
                                .is_some_and(|actor| actor.eq_ignore_ascii_case(&self.nick))
                            {
                                if let Some(call) = self.local_call.as_mut() {
                                    if call.channel == target {
                                        call.session_id = state.session_id;
                                    }
                                }
                                self.try_start_media(widgets);
                            }
                        }
                        AvAction::Left => {
                            if let Some(call) = self.channel_calls.get_mut(&target) {
                                call.participants = state.participants.unwrap_or(0);
                            }
                        }
                        AvAction::Ended => {
                            self.channel_calls.remove(&target);
                            if self
                                .local_call
                                .as_ref()
                                .is_some_and(|call| call.channel == target)
                            {
                                self.net.send(NetCmd::AvMediaStop);
                                self.local_call = None;
                            }
                        }
                    }
                    self.render_call_controls(widgets);
                }
            }
            _ => {}
        }
    }

    fn toggle_call(&mut self, widgets: &Widgets) {
        if let Some(call) = self.local_call.take() {
            self.net.send(NetCmd::AvLeave {
                channel: call.channel,
                session_id: call.session_id,
                instance: call.instance,
            });
            self.net.send(NetCmd::AvMediaStop);
            self.video = None;
            self.video_generations.clear();
            clear_flow_box(&widgets.video_grid);
            widgets.video_grid.set_visible(false);
            widgets.call_status.set_text("Call ended");
            self.render_call_controls(widgets);
            return;
        }

        let Some(channel) = self.active_channel.clone() else {
            widgets.status.set_text("Select a channel before calling");
            return;
        };
        let existing = self.channel_calls.get(&channel);
        let session_id = existing
            .map(|call| call.session_id.clone())
            .unwrap_or_default();
        self.local_call = Some(LocalCall {
            channel: channel.clone(),
            session_id: session_id.clone(),
            instance: String::new(),
            muted: false,
            speaker_muted: false,
            camera: false,
            media_started: false,
        });
        if session_id.is_empty() {
            self.net.send(NetCmd::AvStart { channel });
        } else {
            self.net.send(NetCmd::AvJoin {
                channel,
                session_id,
            });
        }
        widgets.call_status.set_text("Joining call…");
        self.render_call_controls(widgets);
    }

    fn try_start_media(&mut self, widgets: &Widgets) {
        let Some(call) = self.local_call.as_mut() else {
            return;
        };
        if call.media_started || call.session_id.is_empty() || call.instance.is_empty() {
            return;
        }
        let sfu_url = match sleek::av::sfu_moq_url(&self.server, None) {
            Ok(url) => url.to_string(),
            Err(error) => {
                widgets.call_status.set_text(&format!("Media error: {error}"));
                return;
            }
        };
        call.media_started = true;
        self.net.send(NetCmd::AvMediaConnect {
            sfu_url,
            session_id: call.session_id.clone(),
            nick: self.nick.clone(),
            instance: call.instance.clone(),
            muted: call.muted,
            speaker_muted: call.speaker_muted,
            camera: call.camera,
            camera_id: None,
            mic_id: None,
            speaker_id: None,
        });
        widgets.call_status.set_text("Connecting media…");
    }

    fn render_call_controls(&self, widgets: &Widgets) {
        let Some(channel) = self.active_channel.as_deref() else {
            widgets.call_button.set_sensitive(false);
            widgets.call_bar.set_visible(false);
            return;
        };
        widgets.call_button.set_sensitive(true);
        if let Some(call) = &self.local_call {
            widgets.call_button.set_label("Leave Call");
            widgets.call_button.remove_css_class("suggested-action");
            widgets.call_button.add_css_class("destructive-action");
            widgets.call_bar.set_visible(true);
            widgets
                .mute_button
                .set_label(if call.muted { "Unmute" } else { "Mute" });
            widgets.speaker_button.set_label(if call.speaker_muted {
                "Unmute Speaker"
            } else {
                "Mute Speaker"
            });
            widgets
                .camera_button
                .set_label(if call.camera { "Camera Off" } else { "Camera On" });
        } else {
            widgets.call_button.remove_css_class("destructive-action");
            widgets.call_button.add_css_class("suggested-action");
            widgets.call_bar.set_visible(false);
            if let Some(call) = self.channel_calls.get(channel) {
                widgets
                    .call_button
                    .set_label(&format!("Join Call ({})", call.participants));
            } else {
                widgets.call_button.set_label("Start Call");
            }
        }
    }

    fn render_video(&mut self, widgets: &Widgets) {
        let frames = self
            .video
            .as_ref()
            .map(sleek::av::VideoFrameStore::snapshot)
            .unwrap_or_default();
        let generations: HashMap<_, _> = frames
            .iter()
            .map(|(nick, frame)| (nick.clone(), frame.gen))
            .collect();
        if generations == self.video_generations {
            return;
        }
        self.video_generations = generations;
        clear_flow_box(&widgets.video_grid);
        widgets.video_grid.set_visible(!frames.is_empty());

        for (nick, frame) in frames {
            let bytes = gtk::glib::Bytes::from_owned(frame.rgba.to_vec());
            let texture = gtk::gdk::MemoryTexture::new(
                frame.width as i32,
                frame.height as i32,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                frame.width as usize * 4,
            );
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_size_request(320, 180);
            picture.set_can_shrink(true);

            let tile = gtk::Box::new(gtk::Orientation::Vertical, 4);
            tile.add_css_class("card");
            tile.set_margin_top(4);
            tile.set_margin_bottom(4);
            tile.set_margin_start(4);
            tile.set_margin_end(4);
            tile.append(&picture);
            let label = gtk::Label::new(Some(&nick));
            label.add_css_class("caption");
            label.set_margin_bottom(6);
            tile.append(&label);
            widgets.video_grid.insert(&tile, -1);
        }
    }

    fn ensure_channel(&mut self, channel: &str) {
        if !self.channels.iter().any(|item| item == channel) {
            self.channels.push(channel.to_owned());
        }
        self.messages.entry(channel.to_owned()).or_default();
        self.members.entry(channel.to_owned()).or_default();
    }

    fn add_member(&mut self, channel: &str, nick: &str) {
        let members = self.members.entry(channel.to_owned()).or_default();
        if !members
            .iter()
            .any(|member| member.eq_ignore_ascii_case(nick))
        {
            members.push(nick.to_owned());
            members.sort_by_key(|member| member.to_ascii_lowercase());
        }
    }

    fn remove_member(&mut self, channel: &str, nick: &str) {
        if let Some(members) = self.members.get_mut(channel) {
            members.retain(|member| !member.eq_ignore_ascii_case(nick));
        }
    }

    fn push_message(
        &mut self,
        channel: &str,
        id: String,
        from: String,
        text: String,
        reactions: HashMap<String, HashSet<String>>,
    ) {
        let messages = self.messages.entry(channel.to_owned()).or_default();
        messages.push(ChatLine {
            id,
            from,
            text,
            reactions,
        });
        if messages.len() > 500 {
            messages.remove(0);
        }
    }

    fn apply_reaction(
        &mut self,
        channel: &str,
        msgid: &str,
        emoji: &str,
        nick: &str,
        add: bool,
    ) {
        let Some(message) = self
            .messages
            .get_mut(channel)
            .and_then(|messages| messages.iter_mut().find(|message| message.id == msgid))
        else {
            return;
        };
        if add {
            message
                .reactions
                .entry(emoji.to_owned())
                .or_default()
                .insert(nick.to_owned());
        } else if let Some(nicks) = message.reactions.get_mut(emoji) {
            nicks.retain(|reactor| !reactor.eq_ignore_ascii_case(nick));
            if nicks.is_empty() {
                message.reactions.remove(emoji);
            }
        }
    }

    fn render_channels(&self, widgets: &Widgets, sender: &ComponentSender<Self>) {
        clear_box(&widgets.channel_list);
        for channel in &self.channels {
            let button = gtk::ToggleButton::with_label(channel);
            button.set_has_frame(false);
            button.set_halign(gtk::Align::Fill);
            button.set_hexpand(true);
            let selected = self.active_channel.as_deref() == Some(channel.as_str());
            button.set_active(selected);
            button.connect_clicked({
                let sender = sender.clone();
                let channel = channel.clone();
                move |_| sender.input(Input::SelectChannel(channel.clone()))
            });
            widgets.channel_list.append(&button);
        }
    }

    fn render_users(&self, widgets: &Widgets) {
        clear_list(&widgets.user_list);
        let Some(channel) = self.active_channel.as_deref() else {
            return;
        };
        for nick in self.members.get(channel).into_iter().flatten() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_top(6);
            row.set_margin_bottom(6);
            row.set_margin_start(12);
            row.set_margin_end(12);
            let presence = gtk::Label::new(Some("●"));
            presence.add_css_class("success");
            let label = gtk::Label::new(Some(nick));
            label.set_halign(gtk::Align::Start);
            label.set_hexpand(true);
            row.append(&presence);
            row.append(&label);
            widgets.user_list.append(&row);
        }
    }

    fn render_messages(&self, widgets: &Widgets, sender: &ComponentSender<Self>) {
        clear_list(&widgets.message_list);
        let Some(channel) = self.active_channel.as_deref() else {
            widgets.heading.set_text("Chats");
            widgets.compose.set_sensitive(false);
            return;
        };
        widgets.heading.set_text(channel);
        widgets.compose.set_sensitive(true);
        if let Some(messages) = self.messages.get(channel) {
            for message in messages {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                row.set_margin_top(6);
                row.set_margin_bottom(6);
                row.set_margin_start(12);
                row.set_margin_end(12);
                let author = gtk::Label::new(Some(&message.from));
                author.set_halign(gtk::Align::Start);
                author.add_css_class("heading");
                let body = gtk::Label::new(Some(&message.text));
                body.set_halign(gtk::Align::Start);
                body.set_xalign(0.0);
                body.set_hexpand(true);
                body.set_wrap(true);
                body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                body.set_width_chars(1);
                body.set_max_width_chars(80);
                body.set_selectable(true);
                row.append(&author);
                row.append(&body);

                let reactions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                for (emoji, nicks) in &message.reactions {
                    let button = gtk::Button::with_label(&format!("{emoji} {}", nicks.len()));
                    button.add_css_class("flat");
                    button.connect_clicked({
                        let sender = sender.clone();
                        let target = channel.to_owned();
                        let msgid = message.id.clone();
                        let emoji = emoji.clone();
                        move |_| {
                            sender.input(Input::React {
                                target: target.clone(),
                                msgid: msgid.clone(),
                                emoji: emoji.clone(),
                            })
                        }
                    });
                    reactions.append(&button);
                }
                if !message.id.is_empty() {
                    let picker_button = gtk::MenuButton::builder()
                        .icon_name("face-smile-symbolic")
                        .tooltip_text("Add reaction")
                        .build();
                    picker_button.add_css_class("flat");
                    picker_button.add_css_class("circular");
                    let picker = gtk::EmojiChooser::new();
                    picker.connect_emoji_picked({
                        let sender = sender.clone();
                        let target = channel.to_owned();
                        let msgid = message.id.clone();
                        move |_, emoji| {
                            sender.input(Input::React {
                                target: target.clone(),
                                msgid: msgid.clone(),
                                emoji: emoji.to_owned(),
                            });
                        }
                    });
                    picker_button.set_popover(Some(&picker));
                    reactions.append(&picker_button);
                }
                row.append(&reactions);
                widgets.message_list.append(&row);
            }
        }
        let adjustment = widgets.message_scroll.vadjustment();
        gtk::glib::idle_add_local_once(move || {
            adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
        });
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn clear_flow_box(flow_box: &gtk::FlowBox) {
    while let Some(child) = flow_box.first_child() {
        if let Ok(child) = child.downcast::<gtk::FlowBoxChild>() {
            flow_box.remove(&child);
        } else {
            break;
        }
    }
}

fn clean_nick(nick: &str) -> &str {
    nick.trim_start_matches(['~', '&', '@', '%', '+'])
}

fn message_id(tags: &HashMap<String, String>) -> String {
    tags.get("msgid")
        .or_else(|| tags.get("+msgid"))
        .or_else(|| tags.get("+draft/msgid"))
        .or_else(|| tags.get("draft/msgid"))
        .cloned()
        .unwrap_or_default()
}

fn parse_reactions(raw: &str) -> HashMap<String, HashSet<String>> {
    raw.split(';')
        .filter_map(|entry| {
            let (emoji, nicks) = entry.split_once(':')?;
            if emoji.is_empty() {
                return None;
            }
            Some((
                emoji.to_owned(),
                nicks
                    .split(',')
                    .filter(|nick| !nick.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ))
        })
        .collect()
}

fn reaction_update(tags: &HashMap<String, String>) -> Option<(String, String, bool)> {
    let msgid = tags
        .get("+reply")
        .or_else(|| tags.get("reply"))
        .or_else(|| tags.get("+draft/reply"))
        .or_else(|| tags.get("draft/reply"))?
        .clone();
    if let Some(emoji) = tags
        .get("+freeq.at/unreact")
        .or_else(|| tags.get("freeq.at/unreact"))
    {
        return Some((msgid, emoji.clone(), false));
    }
    let emoji = tags
        .get("+react")
        .or_else(|| tags.get("react"))
        .or_else(|| tags.get("+draft/react"))
        .or_else(|| tags.get("draft/react"))?
        .clone();
    Some((msgid, emoji, true))
}

fn default_nick() -> String {
    let suffix = std::process::id() % 10_000;
    format!("sleek{suffix:04}")
}

fn main() {
    adw::init().expect("failed to initialize libadwaita");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
    let app = RelmApp::new("uk.nandi.sleek");
    app.run::<App>(());
}
