//! Relm4 desktop frontend.
//!
//! This intentionally shares Sleek's existing network bridge while the UI is
//! migrated screen by screen. The Android/egui frontend remains available as
//! `sleek-egui` during the transition.

use std::collections::HashMap;
use std::time::Duration;

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
    channels: Vec<String>,
    messages: HashMap<String, Vec<ChatLine>>,
    active_channel: Option<String>,
}

struct ChatLine {
    from: String,
    text: String,
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
    Tick,
}

struct Widgets {
    stack: gtk::Stack,
    header: gtk::HeaderBar,
    status: gtk::Label,
    heading: gtk::Label,
    channel_list: gtk::ListBox,
    message_list: gtk::ListBox,
    compose: gtk::Entry,
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
        let status = gtk::Label::new(Some("Connecting…"));
        status.set_margin_top(6);
        status.set_margin_bottom(6);
        status.add_css_class("dim-label");

        let channel_list = gtk::ListBox::new();
        channel_list.set_selection_mode(gtk::SelectionMode::None);
        channel_list.set_width_request(220);
        channel_list.add_css_class("navigation-sidebar");

        let channel_scroll = gtk::ScrolledWindow::builder()
            .child(&channel_list)
            .vexpand(true)
            .build();
        channel_scroll.add_css_class("sidebar");

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
        conversation.append(&message_scroll);
        conversation.append(&compose_row);

        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_start_child(Some(&channel_scroll));
        split.set_end_child(Some(&conversation));
        split.set_resize_start_child(false);
        split.set_shrink_start_child(false);

        shell.append(&status);
        shell.append(&split);
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

        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            sender.input(Input::Tick);
            gtk::glib::ControlFlow::Continue
        });

        let model = App {
            net: NetBridge::start(),
            connected: false,
            nick: String::new(),
            channels: Vec::new(),
            messages: HashMap::new(),
            active_channel: None,
        };
        let widgets = Widgets {
            stack,
            header,
            status,
            heading,
            channel_list,
            message_list,
            compose,
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
                self.channels.clear();
                self.messages.clear();
                self.active_channel = None;
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
                self.active_channel = None;
                root.set_titlebar(None::<&gtk::Widget>);
                widgets.stack.set_visible_child_name("connect");
            }
            Input::SelectChannel(channel) => {
                self.active_channel = Some(channel);
                self.render_messages(widgets);
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
            Input::Tick => {
                for event in self.net.poll() {
                    match event {
                        NetEvent::Status(message) => widgets.status.set_text(&message),
                        NetEvent::Failed(error) => {
                            widgets.status.set_text(&format!("Connection failed: {error}"))
                        }
                        NetEvent::Sdk(event) if self.connected => {
                            self.handle_sdk_event(event, widgets, &_sender)
                        }
                        _ => {}
                    }
                }
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
                if nick.eq_ignore_ascii_case(&self.nick) {
                    self.ensure_channel(&channel);
                    self.net.send(NetCmd::HistoryLatest {
                        target: channel.clone(),
                        count: 100,
                    });
                    if self.active_channel.is_none() {
                        self.active_channel = Some(channel);
                    }
                    self.render_channels(widgets, sender);
                    self.render_messages(widgets);
                }
            }
            Event::Parted { channel, nick } => {
                if nick.eq_ignore_ascii_case(&self.nick) {
                    self.channels.retain(|item| item != &channel);
                    self.messages.remove(&channel);
                    if self.active_channel.as_deref() == Some(channel.as_str()) {
                        self.active_channel = self.channels.first().cloned();
                    }
                    self.render_channels(widgets, sender);
                    self.render_messages(widgets);
                }
            }
            Event::Message {
                from,
                target,
                text,
                dm_key,
                ..
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
                self.push_message(&channel, from, text);
                self.render_channels(widgets, sender);
                if self.active_channel.as_deref() == Some(channel.as_str()) {
                    self.render_messages(widgets);
                }
            }
            _ => {}
        }
    }

    fn ensure_channel(&mut self, channel: &str) {
        if !self.channels.iter().any(|item| item == channel) {
            self.channels.push(channel.to_owned());
        }
        self.messages.entry(channel.to_owned()).or_default();
    }

    fn push_message(&mut self, channel: &str, from: String, text: String) {
        let messages = self.messages.entry(channel.to_owned()).or_default();
        messages.push(ChatLine { from, text });
        if messages.len() > 500 {
            messages.remove(0);
        }
    }

    fn render_channels(&self, widgets: &Widgets, sender: &ComponentSender<Self>) {
        clear_list(&widgets.channel_list);
        for channel in &self.channels {
            let button = gtk::Button::with_label(channel);
            button.set_has_frame(false);
            button.set_halign(gtk::Align::Fill);
            let selected = self.active_channel.as_deref() == Some(channel.as_str());
            if selected {
                button.add_css_class("accent");
            }
            button.connect_clicked({
                let sender = sender.clone();
                let channel = channel.clone();
                move |_| sender.input(Input::SelectChannel(channel.clone()))
            });
            widgets.channel_list.append(&button);
        }
    }

    fn render_messages(&self, widgets: &Widgets) {
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
                body.set_wrap(true);
                body.set_selectable(true);
                row.append(&author);
                row.append(&body);
                widgets.message_list.append(&row);
            }
        }
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
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
