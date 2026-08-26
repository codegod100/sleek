//! Relm4 desktop frontend.
//!
//! This intentionally shares Sleek's existing network bridge while the UI is
//! migrated screen by screen. The Android/egui frontend remains available as
//! `sleek-egui` during the transition.

use std::time::Duration;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmApp};
use sleek::net::{NetBridge, NetCmd, NetEvent};

struct App {
    net: NetBridge,
    connected: bool,
}

#[derive(Debug)]
enum Input {
    Connect {
        nick: String,
        server: String,
    },
    Disconnect,
    Tick,
}

struct Widgets {
    stack: gtk::Stack,
    status: gtk::Label,
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
        stack.add_named(&connect, Some("connect"));

        let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::HeaderBar::new();
        let heading = gtk::Label::new(Some("Chats"));
        heading.add_css_class("title-2");
        header.set_title_widget(Some(&heading));
        let disconnect = gtk::Button::with_label("Disconnect");
        header.pack_end(&disconnect);
        let status = gtk::Label::new(Some("Connecting…"));
        status.set_margin_top(24);
        status.set_margin_bottom(24);
        status.add_css_class("dim-label");
        shell.append(&header);
        shell.append(&status);
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

        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            sender.input(Input::Tick);
            gtk::glib::ControlFlow::Continue
        });

        let model = App {
            net: NetBridge::start(),
            connected: false,
        };
        let widgets = Widgets { stack, status };
        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Widgets,
        message: Input,
        _sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            Input::Connect { nick, server } => {
                if nick.trim().is_empty() || server.trim().is_empty() {
                    widgets.status.set_text("Nickname and server are required");
                    return;
                }
                self.connected = true;
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
                widgets.stack.set_visible_child_name("connect");
            }
            Input::Tick => {
                for event in self.net.poll() {
                    match event {
                        NetEvent::Status(message) => widgets.status.set_text(&message),
                        NetEvent::Failed(error) => {
                            widgets.status.set_text(&format!("Connection failed: {error}"))
                        }
                        NetEvent::Sdk(_) if self.connected => {
                            widgets.status.set_text("Online — chat migration in progress")
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn default_nick() -> String {
    let suffix = std::process::id() % 10_000;
    format!("sleek{suffix:04}")
}

fn main() {
    let app = RelmApp::new("uk.nandi.sleek");
    app.run::<App>(());
}
