//! Relm4 desktop frontend.
//!
//! This intentionally shares Sleek's existing network bridge while the UI is
//! migrated screen by screen. The Android/egui frontend remains available as
//! `sleek-egui` during the transition.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

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
    pending_auth_server: Option<String>,
    channels: Vec<String>,
    topics: HashMap<String, String>,
    messages: HashMap<String, Vec<ChatLine>>,
    members: HashMap<String, Vec<String>>,
    active_channel: Option<String>,
    channel_calls: HashMap<String, ChannelCall>,
    local_call: Option<LocalCall>,
    video: Option<sleek::av::VideoFrameStore>,
    video_generations: HashMap<String, u64>,
    image_previews: HashMap<String, gtk::gdk::Texture>,
    pending_image_previews: HashSet<String>,
    pending_edit: Option<(String, usize, String)>,
    pending_reply: Option<(String, String, String)>,
    history_settle_at: HashMap<String, Instant>,
    mobile: bool,
    /// Whether the message list should auto-follow new content. Shared with
    /// the scroll adjustment's own closures (see `message_overlay` setup),
    /// which flip it as the user scrolls, and read from `render_messages`'
    /// follow-to-bottom pass so scrolling up to read history is not undone
    /// by the next incoming event.
    pinned_to_bottom: Rc<Cell<bool>>,
}

struct ChatLine {
    id: String,
    from: String,
    text: String,
    reply_to: Option<String>,
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
    AtprotoLogin {
        handle: String,
        server: String,
    },
    /// A `freeq://auth` callback delivered by Android as a VIEW intent.
    #[cfg(target_os = "android")]
    DeepLink(String),
    Disconnect,
    ToggleChannels,
    ToggleUsers,
    SelectChannel(String),
    OpenDm(String),
    SendMessage(String),
    ToggleCall,
    ToggleMute,
    ToggleSpeaker,
    ToggleCamera,
    Viewport(i32),
    React {
        target: String,
        message_index: usize,
        msgid: String,
        emoji: String,
    },
    Edit {
        target: String,
        message_index: usize,
        msgid: String,
    },
    Reply {
        target: String,
        msgid: String,
        author: String,
    },
    JumpToMessage(String),
    CancelEdit,
    ImagePreviewLoaded {
        url: String,
        bytes: Result<Vec<u8>, String>,
    },
    Tick,
}

struct Widgets {
    stack: gtk::Stack,
    header: gtk::HeaderBar,
    disconnect: gtk::Button,
    status: gtk::Label,
    topic: gtk::Label,
    login_status: gtk::Label,
    heading: gtk::Label,
    channel_list: gtk::Box,
    channel_scroll: gtk::ScrolledWindow,
    channel_separator: gtk::Separator,
    compact_channel_list: gtk::Box,
    compact_channels: gtk::Revealer,
    compact_channels_button: gtk::Button,
    compact_user_list: gtk::ListBox,
    compact_users: gtk::Revealer,
    compact_users_button: gtk::Button,
    user_list: gtk::ListBox,
    user_scroll: gtk::ScrolledWindow,
    user_separator: gtk::Separator,
    message_list: gtk::ListBox,
    message_scroll: gtk::ScrolledWindow,
    jump_to_present: gtk::Button,
    conversation: gtk::Box,
    video_grid: gtk::FlowBox,
    compose: gtk::Entry,
    edit_cancel_button: gtk::Button,
    call_button: gtk::Button,
    call_bar: gtk::Box,
    call_status: gtk::Label,
    mute_button: gtk::Button,
    speaker_button: gtk::Button,
    camera_button: gtk::Button,
    reaction_bars: HashMap<usize, gtk::Box>,
    message_rows: HashMap<String, gtk::Box>,
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
            .default_width(320)
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
        connect.set_margin_top(18);
        connect.set_margin_bottom(18);
        connect.set_margin_start(18);
        connect.set_margin_end(18);
        let connect_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        connect_card.add_css_class("card");
        connect_card.set_halign(gtk::Align::Center);
        connect_card.set_valign(gtk::Align::Center);
        connect_card.set_width_request(280);
        connect_card.append(&connect);

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
        let saved_prefs = sleek::auth::SavedPrefs::load();
        let handle = gtk::Entry::builder()
            .placeholder_text("Bluesky handle")
            .text(saved_prefs.last_bsky_handle.as_deref().unwrap_or(""))
            .build();
        let atproto_button = gtk::Button::with_label("Sign in with ATProto");
        let login_status = gtk::Label::new(None);
        login_status.add_css_class("dim-label");
        login_status.set_wrap(true);
        let recent_handles = gtk::Box::new(gtk::Orientation::Vertical, 4);
        if !saved_prefs.recent_handles.is_empty() {
            let recent_label = gtk::Label::new(Some("Previous accounts"));
            recent_label.add_css_class("caption");
            recent_label.set_halign(gtk::Align::Start);
            recent_handles.append(&recent_label);
            for saved_handle in saved_prefs.recent_handles.iter().take(5) {
                let button = gtk::Button::with_label(saved_handle);
                button.add_css_class("flat");
                button.set_halign(gtk::Align::Fill);
                button.connect_clicked({
                    let handle = handle.clone();
                    let saved_handle = saved_handle.clone();
                    move |_| handle.set_text(&saved_handle)
                });
                recent_handles.append(&button);
            }
        }

        connect.append(&title);
        connect.append(&subtitle);
        connect.append(&nick);
        connect.append(&server);
        connect.append(&connect_button);
        connect.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        connect.append(&handle);
        connect.append(&recent_handles);
        connect.append(&atproto_button);
        connect.append(&login_status);
        let connect_clamp = adw::Clamp::builder()
            .maximum_size(440)
            .tightening_threshold(360)
            .child(&connect_card)
            .build();
        stack.add_named(&connect_clamp, Some("connect"));

        let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::HeaderBar::new();
        let heading = gtk::Label::new(Some("Chats"));
        heading.add_css_class("title-2");
        let title = gtk::Box::new(gtk::Orientation::Vertical, 0);
        title.append(&heading);
        let topic = gtk::Label::new(None);
        topic.add_css_class("caption");
        topic.set_ellipsize(gtk::pango::EllipsizeMode::End);
        topic.set_visible(false);
        title.append(&topic);
        header.set_title_widget(Some(&title));
        let compact_channel_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        compact_channel_list.set_margin_top(6);
        compact_channel_list.set_margin_bottom(6);
        compact_channel_list.set_margin_start(6);
        compact_channel_list.set_margin_end(6);
        let compact_channel_scroll = gtk::ScrolledWindow::builder()
            .child(&compact_channel_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(220)
            .vexpand(true)
            .build();
        let compact_channels = gtk::Revealer::builder()
            .child(&compact_channel_scroll)
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .hexpand(true)
            .build();
        compact_channels.set_reveal_child(false);
        compact_channels.set_visible(false);
        let compact_channels_button = gtk::Button::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Open channel list")
            .build();
        compact_channels_button.set_visible(false);
        header.pack_start(&compact_channels_button);
        let compact_user_list = gtk::ListBox::new();
        compact_user_list.set_selection_mode(gtk::SelectionMode::None);
        compact_user_list.add_css_class("navigation-sidebar");
        let compact_user_scroll = gtk::ScrolledWindow::builder()
            .child(&compact_user_list)
            .min_content_width(180)
            .vexpand(true)
            .build();
        let compact_users = gtk::Revealer::builder()
            .child(&compact_user_scroll)
            .transition_type(gtk::RevealerTransitionType::SlideLeft)
            .hexpand(true)
            .build();
        compact_users.set_visible(false);
        let compact_users_button = gtk::Button::builder()
            .icon_name("system-users-symbolic")
            .tooltip_text("Open user list")
            .build();
        compact_users_button.set_visible(false);
        header.pack_end(&compact_users_button);
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
        channel_list.set_margin_top(8);
        channel_list.set_margin_bottom(8);
        channel_list.set_margin_start(8);
        channel_list.set_margin_end(8);

        let channel_scroll = gtk::ScrolledWindow::builder()
            .child(&channel_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_width(220)
            .vexpand(true)
            .build();

        let message_list = gtk::ListBox::new();
        message_list.set_selection_mode(gtk::SelectionMode::None);
        message_list.add_css_class("boxed-list");
        message_list.set_valign(gtk::Align::Start);
        let message_scroll = gtk::ScrolledWindow::builder()
            .child(&message_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .build();
        message_scroll.add_css_class("view");

        let pinned_to_bottom = Rc::new(Cell::new(true));

        let jump_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        jump_content.append(&gtk::Image::from_icon_name("go-down-symbolic"));
        jump_content.append(&gtk::Label::new(Some("Jump to Present")));
        let jump_to_present = gtk::Button::builder().child(&jump_content).build();
        jump_to_present.add_css_class("osd");
        jump_to_present.add_css_class("pill");
        jump_to_present.add_css_class("suggested-action");
        jump_to_present.set_halign(gtk::Align::Center);
        jump_to_present.set_valign(gtk::Align::End);
        jump_to_present.set_margin_bottom(12);
        jump_to_present.set_visible(false);
        jump_to_present.set_tooltip_text(Some("Jump to the latest messages"));

        let message_overlay = gtk::Overlay::new();
        message_overlay.set_hexpand(true);
        message_overlay.set_vexpand(true);
        message_overlay.set_child(Some(&message_scroll));
        message_overlay.add_overlay(&jump_to_present);

        let adjustment = message_scroll.vadjustment();
        // GTK updates the adjustment only after wrapped rows have been
        // measured. Follow that authoritative size change so live messages
        // reach the bottom even when allocation takes multiple frames — but
        // only while the user is not reading back through history, or every
        // incoming message/reaction would yank the view out from under them.
        adjustment.connect_upper_notify({
            let pinned_to_bottom = pinned_to_bottom.clone();
            move |adjustment| {
                if pinned_to_bottom.get() {
                    adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
                }
            }
        });
        // Distance-from-bottom is how we notice the user scrolled away (or
        // back), whether by drag, wheel, or our own programmatic jumps.
        adjustment.connect_value_changed({
            let pinned_to_bottom = pinned_to_bottom.clone();
            let jump_to_present = jump_to_present.clone();
            move |adjustment| {
                let distance =
                    (adjustment.upper() - adjustment.page_size()) - adjustment.value();
                let near_bottom = distance <= 48.0;
                pinned_to_bottom.set(near_bottom);
                jump_to_present.set_visible(!near_bottom);
            }
        });
        jump_to_present.connect_clicked({
            let pinned_to_bottom = pinned_to_bottom.clone();
            let message_scroll = message_scroll.clone();
            move |button| {
                pinned_to_bottom.set(true);
                let adjustment = message_scroll.vadjustment();
                adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
                button.set_visible(false);
            }
        });

        let compose = gtk::Entry::builder()
            .placeholder_text("Message")
            .hexpand(true)
            .build();
        let edit_cancel_button = gtk::Button::with_label("Cancel");
        edit_cancel_button.add_css_class("flat");
        edit_cancel_button.set_tooltip_text(Some("Cancel message edit"));
        edit_cancel_button.set_visible(false);
        let send_button = gtk::Button::with_label("Send");
        send_button.add_css_class("suggested-action");
        let compose_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        compose_row.set_margin_top(12);
        compose_row.set_margin_bottom(12);
        compose_row.set_margin_start(12);
        compose_row.set_margin_end(12);
        compose_row.append(&compose);
        compose_row.append(&edit_cancel_button);
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
        conversation.append(&message_overlay);

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
        user_list.set_margin_top(8);
        user_list.set_margin_bottom(8);
        user_list.set_margin_start(8);
        user_list.set_margin_end(8);
        let user_scroll = gtk::ScrolledWindow::builder()
            .child(&user_list)
            .vexpand(true)
            .max_content_width(180)
            .build();

        let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        layout.set_margin_start(8);
        layout.set_margin_end(8);
        layout.append(&compact_channels);
        layout.append(&channel_scroll);
        let channel_separator = gtk::Separator::new(gtk::Orientation::Vertical);
        layout.append(&channel_separator);
        conversation.set_hexpand(true);
        layout.append(&conversation);
        let user_separator = gtk::Separator::new(gtk::Orientation::Vertical);
        layout.append(&user_separator);
        layout.append(&user_scroll);
        layout.append(&compact_users);

        shell.append(&status);
        shell.append(&layout);
        stack.add_named(&shell, Some("shell"));

        root.set_child(Some(&stack));

        connect_button.connect_clicked({
            let sender = sender.clone();
            let nick = nick.clone();
            let server = server.clone();
            move |_| {
                sender.input(Input::Connect {
                    nick: nick.text().to_string(),
                    server: server.text().to_string(),
                });
            }
        });
        atproto_button.connect_clicked({
            let sender = sender.clone();
            let handle = handle.clone();
            let server = server.clone();
            move |_| {
                sender.input(Input::AtprotoLogin {
                    handle: handle.text().to_string(),
                    server: server.text().to_string(),
                });
            }
        });
        disconnect.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::Disconnect)
        });
        compact_channels_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleChannels)
        });
        compact_users_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::ToggleUsers)
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
        edit_cancel_button.connect_clicked({
            let sender = sender.clone();
            move |_| sender.input(Input::CancelEdit)
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
        root.connect_notify_local(Some("width"), {
            let sender = sender.clone();
            move |root, _| sender.input(Input::Viewport(root.width()))
        });
        root.connect_map({
            let sender = sender.clone();
            move |root| sender.input(Input::Viewport(root.width()))
        });

        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            sender.input(Input::Tick);
            gtk::glib::ControlFlow::Continue
        });

        let net = NetBridge::start();
        let saved_session =
            sleek::auth::SavedSession::load().filter(sleek::auth::SavedSession::has_session);
        let pending_auth_server = saved_session.as_ref().map(|session| {
            if session.server.trim().is_empty() {
                sleek::auth::DEFAULT_IRC_SERVER.into()
            } else {
                session.server.clone()
            }
        });
        if let Some(session) = &saved_session {
            login_status.set_text("Restoring your ATProto session…");
            net.send(NetCmd::ReconnectSession {
                broker_token: session.broker_token.clone(),
                auth_broker: sleek::auth::DEFAULT_AUTH_BROKER.into(),
            });
        }
        let model = App {
            net,
            connected: false,
            nick: String::new(),
            server: String::new(),
            pending_auth_server,
            channels: Vec::new(),
            topics: HashMap::new(),
            messages: HashMap::new(),
            members: HashMap::new(),
            active_channel: None,
            channel_calls: HashMap::new(),
            local_call: None,
            video: None,
            video_generations: HashMap::new(),
            image_previews: HashMap::new(),
            pending_image_previews: HashSet::new(),
            pending_edit: None,
            pending_reply: None,
            history_settle_at: HashMap::new(),
            mobile: false,
            pinned_to_bottom: pinned_to_bottom.clone(),
        };
        let widgets = Widgets {
            stack,
            header,
            disconnect,
            status,
            topic,
            login_status,
            heading,
            channel_list,
            channel_scroll,
            channel_separator,
            compact_channel_list,
            compact_channels,
            compact_channels_button,
            compact_user_list,
            compact_users,
            compact_users_button,
            user_list,
            user_scroll,
            user_separator,
            message_list,
            message_scroll,
            jump_to_present,
            conversation,
            video_grid,
            compose,
            edit_cancel_button,
            call_button,
            call_bar,
            call_status,
            mute_button,
            speaker_button,
            camera_button,
            reaction_bars: HashMap::new(),
            message_rows: HashMap::new(),
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
                    // `status` lives on the shell page, which is not on screen
                    // yet — validation has to speak through the login page's
                    // own label or it is silently swallowed.
                    widgets
                        .login_status
                        .set_text("Nickname and server are required");
                    return;
                }
                self.connected = true;
                self.nick = nick.clone();
                self.server = server.clone();
                self.channels.clear();
                self.topics.clear();
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
            #[cfg(target_os = "android")]
            Input::DeepLink(url) => {
                match sleek::auth::parse_freeq_auth_url(&url) {
                    Ok(tokens) => self.apply_auth_tokens(tokens, root, widgets),
                    Err(error) => {
                        let message = format!("Invalid freeq://auth callback: {error}");
                        relm4::gtk::glib::g_warning!("sleek", "{message}");
                        widgets.status.set_text(&message);
                        if !self.connected {
                            widgets.login_status.set_text(&message);
                        }
                    }
                }
                // GTK has no onNewIntent hook, so the callback always spawns a
                // fresh ToplevelActivity with no toplevel to bind to — the user
                // is left on a blank activity and has to switch back by hand.
                // This at least guarantees the real window exists and is mapped
                // (it matters on a cold start); it cannot re-front the activity.
                root.present();
            }
            Input::AtprotoLogin { handle, server } => {
                let handle = sleek::bsky::normalize_handle_query(&handle);
                if handle.is_empty() || server.trim().is_empty() {
                    widgets
                        .login_status
                        .set_text("Bluesky handle and server are required");
                    return;
                }
                self.pending_auth_server = Some(server.trim().to_owned());
                widgets
                    .login_status
                    .set_text("Opening ATProto sign-in in your browser…");
                self.net.send(NetCmd::BlueskyLogin {
                    handle,
                    auth_broker: sleek::auth::DEFAULT_AUTH_BROKER.into(),
                });
            }
            Input::Disconnect => {
                self.net.send(NetCmd::Quit);
                self.net = NetBridge::start();
                self.connected = false;
                self.pending_edit = None;
                self.pending_reply = None;
                widgets.edit_cancel_button.set_visible(false);
                self.channels.clear();
                self.topics.clear();
                self.messages.clear();
                self.members.clear();
                self.active_channel = None;
                self.channel_calls.clear();
                self.local_call = None;
                self.video = None;
                self.video_generations.clear();
                self.history_settle_at.clear();
                clear_flow_box(&widgets.video_grid);
                widgets.video_grid.set_visible(false);
                widgets.call_bar.set_visible(false);
                root.set_titlebar(None::<&gtk::Widget>);
                widgets.stack.set_visible_child_name("connect");
            }
            Input::ToggleChannels => {
                let open = !widgets.compact_channels.is_visible();
                widgets.compact_users.set_reveal_child(false);
                widgets.compact_users.set_visible(false);
                widgets.compact_channels.set_visible(open);
                widgets.compact_channels.set_reveal_child(open);
                if self.mobile {
                    widgets.conversation.set_visible(!open);
                }
            }
            Input::ToggleUsers => {
                let open = !widgets.compact_users.is_visible();
                widgets.compact_channels.set_reveal_child(false);
                widgets.compact_channels.set_visible(false);
                widgets.compact_users.set_visible(open);
                widgets.compact_users.set_reveal_child(open);
                if self.mobile {
                    widgets.conversation.set_visible(!open);
                }
            }
            Input::SelectChannel(channel) => {
                self.pinned_to_bottom.set(true);
                widgets.jump_to_present.set_visible(false);
                self.active_channel = Some(channel);
                self.pending_edit = None;
                self.pending_reply = None;
                widgets.compose.set_placeholder_text(Some("Message"));
                widgets.compose.set_text("");
                widgets.edit_cancel_button.set_visible(false);
                widgets.compact_channels.set_reveal_child(false);
                widgets.compact_channels.set_visible(false);
                widgets.conversation.set_visible(true);
                self.render_channels(widgets, &_sender);
                self.render_topic(widgets);
                if self.history_settle_at.contains_key(
                    self.active_channel.as_deref().unwrap_or_default(),
                ) {
                    clear_list(&widgets.message_list);
                } else {
                    self.render_messages(widgets, &_sender);
                }
                self.render_users(widgets, &_sender);
                self.render_call_controls(widgets);
                widgets.compose.grab_focus();
            }
            Input::OpenDm(nick) => {
                self.pinned_to_bottom.set(true);
                widgets.jump_to_present.set_visible(false);
                let is_new = !self.channels.iter().any(|target| target == &nick);
                self.ensure_channel(&nick);
                self.active_channel = Some(nick.clone());
                self.pending_edit = None;
                self.pending_reply = None;
                widgets.compose.set_placeholder_text(Some("Message"));
                widgets.compose.set_text("");
                widgets.edit_cancel_button.set_visible(false);
                widgets.compact_users.set_reveal_child(false);
                widgets.compact_users.set_visible(false);
                widgets.conversation.set_visible(true);
                if is_new {
                    self.history_settle_at
                        .insert(nick.clone(), Instant::now() + Duration::from_millis(350));
                    self.net.send(NetCmd::HistoryLatest {
                        target: nick,
                        count: 100,
                    });
                }
                self.render_channels(widgets, &_sender);
                self.render_topic(widgets);
                if self.history_settle_at.contains_key(
                    self.active_channel.as_deref().unwrap_or_default(),
                ) {
                    clear_list(&widgets.message_list);
                } else {
                    self.render_messages(widgets, &_sender);
                }
                self.render_users(widgets, &_sender);
                self.render_call_controls(widgets);
                widgets.compose.grab_focus();
            }
            Input::SendMessage(text) => {
                let text = text.trim();
                if text.is_empty() {
                    return;
                }
                if let Some((target, message_index, msgid)) = self.pending_edit.take() {
                    self.net.send(NetCmd::EditMessage {
                        target: target.clone(),
                        msgid,
                        text: text.to_owned(),
                    });
                    if let Some(message) = self
                        .messages
                        .get_mut(&target)
                        .and_then(|messages| messages.get_mut(message_index))
                    {
                        message.text = text.to_owned();
                    }
                    widgets.compose.set_placeholder_text(Some("Message"));
                    widgets.compose.set_text("");
                    widgets.edit_cancel_button.set_visible(false);
                    self.render_messages(widgets, &_sender);
                    return;
                }
                if let Some((target, msgid, _)) = self.pending_reply.take() {
                    self.pinned_to_bottom.set(true);
                    widgets.jump_to_present.set_visible(false);
                    self.net.send(NetCmd::Reply {
                        target: target.clone(),
                        msgid: msgid.clone(),
                        text: text.to_owned(),
                    });
                    let nick = self.nick.clone();
                    self.push_message(
                        &target,
                        String::new(),
                        nick,
                        text.to_owned(),
                        Some(msgid),
                        HashMap::new(),
                    );
                    widgets.compose.set_placeholder_text(Some("Message"));
                    widgets.compose.set_text("");
                    widgets.edit_cancel_button.set_visible(false);
                    self.render_messages(widgets, &_sender);
                    return;
                }
                let Some(target) = self.active_channel.clone() else {
                    widgets.status.set_text("Select a chat before sending");
                    return;
                };
                self.pinned_to_bottom.set(true);
                widgets.jump_to_present.set_visible(false);
                self.net.send(NetCmd::Privmsg {
                    target: target.clone(),
                    text: text.to_owned(),
                });
                let nick = self.nick.clone();
                self.push_message(
                    &target,
                    String::new(),
                    nick,
                    text.to_owned(),
                    None,
                    HashMap::new(),
                );
                widgets.compose.set_text("");
                self.render_messages(widgets, &_sender);
            }
            Input::Edit {
                target,
                message_index,
                msgid,
            } => {
                let Some(message) = self
                    .messages
                    .get(&target)
                    .and_then(|messages| messages.get(message_index))
                else {
                    return;
                };
                self.pending_edit = Some((target, message_index, msgid));
                self.pending_reply = None;
                widgets.compose.set_placeholder_text(Some("Edit message"));
                widgets.compose.set_text(&message.text);
                widgets.edit_cancel_button.set_visible(true);
                widgets.compose.grab_focus();
                widgets.compose.set_position(-1);
            }
            Input::Reply {
                target,
                msgid,
                author,
            } => {
                self.pending_edit = None;
                self.pending_reply = Some((target, msgid, author.clone()));
                widgets
                    .compose
                    .set_placeholder_text(Some(&format!("Reply to {author}")));
                widgets.compose.set_text("");
                widgets.edit_cancel_button.set_visible(true);
                widgets.compose.grab_focus();
            }
            Input::JumpToMessage(msgid) => {
                if let Some(row) = widgets.message_rows.get(&msgid) {
                    row.grab_focus();
                }
            }
            Input::CancelEdit => {
                self.pending_edit = None;
                self.pending_reply = None;
                widgets.compose.set_placeholder_text(Some("Message"));
                widgets.compose.set_text("");
                widgets.edit_cancel_button.set_visible(false);
                widgets.compose.grab_focus();
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
            Input::Viewport(width) => {
                // Collapse one secondary column at a time so the conversation
                // retains useful space instead of abruptly switching layouts.
                let compact = width < 980;
                let mobile = width < 700;
                self.mobile = mobile;
                widgets.channel_scroll.set_visible(!mobile);
                widgets.channel_separator.set_visible(!mobile);
                widgets.user_scroll.set_visible(!compact);
                widgets.user_separator.set_visible(!compact);
                widgets.compact_channels_button.set_visible(mobile);
                widgets.compact_users_button.set_visible(compact);
                if mobile {
                    widgets.disconnect.set_icon_name("application-exit-symbolic");
                } else {
                    widgets.disconnect.set_label("Disconnect");
                }
                widgets.disconnect.set_tooltip_text(Some("Disconnect"));
                if !mobile {
                    widgets.compact_channels.set_reveal_child(false);
                    widgets.compact_channels.set_visible(false);
                    widgets.conversation.set_visible(true);
                }
                if !compact {
                    widgets.compact_users.set_reveal_child(false);
                    widgets.compact_users.set_visible(false);
                }
                self.render_call_controls(widgets);
            }
            Input::React {
                target,
                message_index,
                msgid,
                emoji,
            } => {
                let reacted = self
                    .messages
                    .get(&target)
                    .and_then(|messages| messages.get(message_index))
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
                self.apply_reaction_at(&target, message_index, &emoji, &nick, !reacted);
                if let (Some(bar), Some(message)) = (
                    widgets.reaction_bars.get(&message_index),
                    self.messages
                        .get(&target)
                        .and_then(|messages| messages.get(message_index)),
                ) {
                    self.render_reaction_bar(
                        bar,
                        &target,
                        message_index,
                        message,
                        &_sender,
                    );
                }
            }
            Input::ImagePreviewLoaded { url, bytes } => {
                self.pending_image_previews.remove(&url);
                match bytes.and_then(|bytes| {
                    let bytes = gtk::glib::Bytes::from_owned(bytes);
                    gtk::gdk::Texture::from_bytes(&bytes).map_err(|error| error.to_string())
                }) {
                    Ok(texture) => {
                        self.image_previews.insert(url, texture);
                        self.render_messages(widgets, &_sender);
                    }
                    Err(error) => eprintln!("image preview failed for {url}: {error}"),
                }
            }
            Input::Tick => {
                #[cfg(target_os = "android")]
                if let Some(url) = take_pending_deep_link() {
                    _sender.input(Input::DeepLink(url));
                }
                let mut refresh_chat = false;
                for event in self.net.poll() {
                    match event {
                        NetEvent::Status(message) => {
                            widgets.status.set_text(&message);
                            if !self.connected {
                                widgets.login_status.set_text(&message);
                            }
                        }
                        NetEvent::Failed(error) => {
                            let message = format!("Connection failed: {error}");
                            widgets.status.set_text(&message);
                            if !self.connected {
                                widgets.login_status.set_text(&message);
                            }
                        }
                        NetEvent::AuthReady(tokens) => {
                            self.apply_auth_tokens(tokens, root, widgets);
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
                let history_ready = self
                    .active_channel
                    .as_ref()
                    .is_some_and(|channel| {
                        self.history_settle_at
                            .get(channel)
                            .is_some_and(|deadline| Instant::now() >= *deadline)
                    });
                if history_ready {
                    if let Some(channel) = &self.active_channel {
                        self.history_settle_at.remove(channel);
                    }
                }
                if refresh_chat {
                    self.render_channels(widgets, &_sender);
                    let active_is_loading = self.active_channel.as_ref().is_some_and(|channel| {
                        self.history_settle_at.contains_key(channel)
                    });
                    if !active_is_loading {
                        self.render_messages(widgets, &_sender);
                    }
                    self.render_users(widgets, &_sender);
                    self.render_call_controls(widgets);
                } else if history_ready {
                    self.render_messages(widgets, &_sender);
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
                    self.history_settle_at.insert(
                        channel.clone(),
                        Instant::now() + Duration::from_millis(350),
                    );
                    self.net.send(NetCmd::HistoryLatest {
                        target: channel.clone(),
                        count: 100,
                    });
                    if self.active_channel.is_none() {
                        self.active_channel = Some(channel.clone());
                    }
                    self.pinned_to_bottom.set(true);
                    widgets.jump_to_present.set_visible(false);
                    self.render_channels(widgets, sender);
                    self.render_messages(widgets, sender);
                }
                if self.active_channel.as_deref() == Some(channel.as_str()) {
                    self.render_users(widgets, sender);
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
                self.render_users(widgets, sender);
            }
            Event::Names { channel, nicks } => {
                for nick in nicks {
                    self.add_member(&channel, clean_nick(&nick));
                }
            }
            Event::TopicChanged { channel, topic, .. } => {
                if topic.trim().is_empty() {
                    self.topics.remove(&channel);
                } else {
                    self.topics.insert(channel.clone(), topic);
                }
                if self.active_channel.as_deref() == Some(channel.as_str()) {
                    self.render_topic(widgets);
                }
            }
            Event::Kicked { channel, nick, .. } => {
                self.remove_member(&channel, &nick);
                self.render_users(widgets, sender);
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
                self.render_users(widgets, sender);
            }
            Event::UserQuit { nick, .. } => {
                for members in self.members.values_mut() {
                    members.retain(|member| !member.eq_ignore_ascii_case(&nick));
                }
                self.render_users(widgets, sender);
            }
            Event::Message {
                from,
                target,
                text,
                dm_key: _,
                tags,
            } => {
                let channel = if target.starts_with('#') || target.starts_with('&') {
                    target
                } else if from.eq_ignore_ascii_case(&self.nick) {
                    // Own DM echoes name the recipient in `target`. Keep the
                    // UI thread keyed by that visible nick rather than the
                    // optional canonical DID in `dm_key`.
                    target
                } else {
                    // Any non-channel message from another user belongs to
                    // that visible peer, even when an authenticated server
                    // addresses our DID rather than our current IRC nick.
                    from.clone()
                };
                self.ensure_channel(&channel);
                let id = message_id(&tags);
                let reactions = tags
                    .get("+freeq.at/reactions")
                    .map(|value| parse_reactions(value))
                    .unwrap_or_default();
                let reply_to = tags
                    .get("+draft/reply")
                    .or_else(|| tags.get("draft/reply"))
                    .or_else(|| tags.get("+reply"))
                    .or_else(|| tags.get("reply"))
                    .cloned();
                let pending_echo = from.eq_ignore_ascii_case(&self.nick)
                    && self.messages.get_mut(&channel).is_some_and(|messages| {
                        messages.iter_mut().rev().take(8).any(|message| {
                            if message.id.is_empty() && message.text == text {
                                message.id = id.clone();
                                message.reply_to = reply_to.clone();
                                message.reactions = reactions.clone();
                                true
                            } else {
                                false
                            }
                        })
                    });
                if !pending_echo {
                    self.push_message(&channel, id, from, text, reply_to, reactions);
                }
                if let Some(deadline) = self.history_settle_at.get_mut(&channel) {
                    *deadline = Instant::now() + Duration::from_millis(150);
                }
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
            if self.mobile {
                widgets.call_button.set_icon_name("call-stop-symbolic");
            } else {
                widgets.call_button.set_label("Leave Call");
            }
            widgets.call_button.set_tooltip_text(Some("Leave call"));
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
            if self.mobile {
                widgets.call_button.set_icon_name("call-start-symbolic");
            } else if let Some(call) = self.channel_calls.get(channel) {
                widgets
                    .call_button
                    .set_label(&format!("Join Call ({})", call.participants));
            } else {
                widgets.call_button.set_label("Start Call");
            }
            widgets.call_button.set_tooltip_text(Some("Start or join call"));
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
        reply_to: Option<String>,
        reactions: HashMap<String, HashSet<String>>,
    ) {
        let messages = self.messages.entry(channel.to_owned()).or_default();
        messages.push(ChatLine {
            id,
            from,
            text,
            reply_to,
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

    fn apply_reaction_at(
        &mut self,
        channel: &str,
        message_index: usize,
        emoji: &str,
        nick: &str,
        add: bool,
    ) {
        let Some(message) = self
            .messages
            .get_mut(channel)
            .and_then(|messages| messages.get_mut(message_index))
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
        clear_box(&widgets.compact_channel_list);
        for channel in &self.channels {
            let selected = self.active_channel.as_deref() == Some(channel.as_str());
            for list in [&widgets.channel_list, &widgets.compact_channel_list] {
                let label = gtk::Label::new(Some(channel));
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_width_chars(1);
                label.set_max_width_chars(24);
                let button = gtk::ToggleButton::builder().child(&label).build();
                button.set_has_frame(false);
                button.set_halign(gtk::Align::Fill);
                button.set_hexpand(true);
                button.set_active(selected);
                button.connect_clicked({
                    let sender = sender.clone();
                    let channel = channel.clone();
                    move |_| sender.input(Input::SelectChannel(channel.clone()))
                });
                list.append(&button);
            }
        }
    }

    fn render_users(&self, widgets: &Widgets, sender: &ComponentSender<Self>) {
        clear_list(&widgets.user_list);
        clear_list(&widgets.compact_user_list);
        let Some(channel) = self.active_channel.as_deref() else {
            return;
        };
        for nick in self.members.get(channel).into_iter().flatten() {
            for list in [&widgets.user_list, &widgets.compact_user_list] {
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
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_max_width_chars(18);
                row.append(&presence);
                row.append(&label);
                let button = gtk::Button::builder()
                    .child(&row)
                    .tooltip_text(format!("Message {nick}"))
                    .build();
                button.add_css_class("flat");
                button.set_halign(gtk::Align::Fill);
                button.set_hexpand(true);
                button.connect_clicked({
                    let sender = sender.clone();
                    let nick = nick.clone();
                    move |_| sender.input(Input::OpenDm(nick.clone()))
                });
                let list_row = gtk::ListBoxRow::new();
                list_row.set_activatable(false);
                list_row.set_selectable(false);
                list_row.set_child(Some(&button));
                list.append(&list_row);
            }
        }
    }

    fn render_topic(&self, widgets: &Widgets) {
        let topic = self
            .active_channel
            .as_deref()
            .and_then(|channel| self.topics.get(channel))
            .map(String::as_str)
            .unwrap_or("");
        widgets.topic.set_text(topic);
        widgets.topic.set_visible(!topic.is_empty());
        widgets.topic.set_tooltip_text((!topic.is_empty()).then_some(topic));
    }

    fn render_messages(&mut self, widgets: &mut Widgets, sender: &ComponentSender<Self>) {
        clear_list(&widgets.message_list);
        widgets.reaction_bars.clear();
        widgets.message_rows.clear();
        let Some(channel) = self.active_channel.as_deref() else {
            widgets.heading.set_text("Chats");
            widgets.compose.set_sensitive(false);
            return;
        };
        widgets.heading.set_text(channel);
        widgets.compose.set_sensitive(true);
        if let Some(messages) = self.messages.get(channel) {
            for (message_index, message) in messages.iter().enumerate() {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                row.set_margin_top(6);
                row.set_margin_bottom(6);
                row.set_margin_start(12);
                // Keep trailing actions clear of the overlay scrollbar.
                row.set_margin_end(24);
                let message_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
                content.set_hexpand(true);
                if let Some(reply_to) = &message.reply_to {
                    let original = messages
                        .iter()
                        .find(|candidate| candidate.id == *reply_to)
                        .map(|candidate| {
                            let mut excerpt: String = candidate.text.chars().take(72).collect();
                            if candidate.text.chars().count() > 72 {
                                excerpt.push('…');
                            }
                            format!("↪ {}: {excerpt}", candidate.from)
                        })
                        .unwrap_or_else(|| "↪ Original message".into());
                    // A Button's own label neither wraps nor ellipsizes, so the
                    // excerpt would set a minimum width wider than the screen and
                    // push the whole conversation out of view. Use a label child
                    // that is allowed to shrink.
                    let reply_label = gtk::Label::new(Some(&original));
                    reply_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    reply_label.set_width_chars(1);
                    reply_label.set_max_width_chars(32);
                    reply_label.set_xalign(0.0);
                    let reply_context = gtk::Button::builder().child(&reply_label).build();
                    reply_context.add_css_class("flat");
                    reply_context.add_css_class("pill");
                    reply_context.set_halign(gtk::Align::Fill);
                    reply_context.set_tooltip_text(Some("Go to original message"));
                    reply_context.connect_clicked({
                        let sender = sender.clone();
                        let reply_to = reply_to.clone();
                        move |_| sender.input(Input::JumpToMessage(reply_to.clone()))
                    });
                    content.append(&reply_context);
                }
                let author = gtk::Label::new(Some(&message.from));
                author.set_halign(gtk::Align::Start);
                author.set_ellipsize(gtk::pango::EllipsizeMode::End);
                author.set_width_chars(1);
                author.set_max_width_chars(28);
                author.add_css_class("heading");
                let body = gtk::Label::new(Some(&message.text));
                body.set_markup(&linkify_message(&message.text));
                body.set_halign(gtk::Align::Start);
                body.set_xalign(0.0);
                body.set_hexpand(true);
                body.set_wrap(true);
                body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                body.set_width_chars(1);
                body.set_max_width_chars(80);
                body.set_selectable(true);
                content.append(&author);
                content.append(&body);

                if let Some(sleek::preview::Embed::Image { url }) =
                    sleek::preview::embed_from_text(&message.text)
                {
                    if let Some(texture) = self.image_previews.get(&url) {
                        let picture = gtk::Picture::for_paintable(texture);
                        picture.set_halign(gtk::Align::Start);
                        picture.set_size_request(240, 180);
                        picture.set_can_shrink(true);
                        picture.set_tooltip_text(Some(&url));
                        content.append(&picture);
                    } else {
                        let loading = gtk::Label::new(Some("Loading image preview…"));
                        loading.add_css_class("dim-label");
                        loading.set_halign(gtk::Align::Start);
                        content.append(&loading);
                        if self.pending_image_previews.insert(url.clone()) {
                            let sender = sender.clone();
                            gtk::glib::spawn_future_local(async move {
                                let bytes = sleek::preview::fetch_image_preview(&url)
                                    .await
                                    .map_err(|error| error.to_string());
                                sender.input(Input::ImagePreviewLoaded { url, bytes });
                            });
                        }
                    }
                }

                let reactions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                self.render_reaction_bar(&reactions, channel, message_index, message, sender);
                content.append(&reactions);
                let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                actions.set_valign(gtk::Align::Start);
                self.render_message_actions(&actions, channel, message_index, message, sender);
                message_line.append(&content);
                message_line.append(&actions);
                row.append(&message_line);
                widgets.reaction_bars.insert(message_index, reactions);
                if !message.id.is_empty() {
                    row.set_focusable(true);
                    widgets.message_rows.insert(message.id.clone(), row.clone());
                }
                widgets.message_list.append(&row);
            }
        }
        let adjustment = widgets.message_scroll.vadjustment();
        let pinned_to_bottom = self.pinned_to_bottom.clone();
        // ListBox geometry is not final during this render pass. Waiting for
        // the next frame ensures `upper` includes newly appended/wrapped rows
        // before moving the viewport, which matters most for live DMs. Only
        // follow if the user has not scrolled away to read history — callers
        // that must always land at the bottom (switching chats, sending)
        // force `pinned_to_bottom` before calling this.
        gtk::glib::timeout_add_local_once(Duration::from_millis(32), move || {
            if pinned_to_bottom.get() {
                adjustment.set_value((adjustment.upper() - adjustment.page_size()).max(0.0));
            }
        });
    }

    /// Persist an ATProto session and connect with the issued web token.
    ///
    /// Shared by the broker callback (`NetEvent::AuthReady`) and the Android
    /// `freeq://auth` deep link, which reach us by different routes.
    fn apply_auth_tokens(
        &mut self,
        tokens: sleek::auth::AuthTokens,
        root: &gtk::ApplicationWindow,
        widgets: &mut Widgets,
    ) {
        let server = self
            .pending_auth_server
            .take()
            .unwrap_or_else(|| sleek::auth::DEFAULT_IRC_SERVER.into());
        let session = sleek::auth::SavedSession {
            broker_token: tokens.broker_token.clone(),
            did: tokens.did.clone(),
            handle: tokens.handle.clone(),
            nick: tokens.nick.clone(),
            server: server.clone(),
            last_login_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default(),
            guest: false,
            use_tls: true,
            use_websocket: false,
        };
        if let Err(error) = session.save() {
            widgets
                .status
                .set_text(&format!("Could not save ATProto session: {error}"));
        }
        let mut prefs = sleek::auth::SavedPrefs::load();
        let handle = tokens.handle.trim().trim_start_matches('@');
        if !handle.is_empty() {
            prefs
                .recent_handles
                .retain(|saved| !saved.eq_ignore_ascii_case(handle));
            prefs.recent_handles.insert(0, handle.to_owned());
            prefs.recent_handles.truncate(8);
            prefs.last_bsky_handle = Some(handle.to_owned());
            if let Err(error) = prefs.save() {
                eprintln!("could not save recent ATProto handle: {error}");
            }
        }
        self.connected = true;
        self.nick = tokens.nick.clone();
        self.server = server.clone();
        root.set_titlebar(Some(&widgets.header));
        widgets.stack.set_visible_child_name("shell");
        widgets.status.set_text("Signing in…");
        self.net.send(NetCmd::Connect {
            nick: tokens.nick,
            server,
            tls: true,
            websocket: false,
            auto_join: vec!["#general".into(), "#test".into()],
            web_token: Some(tokens.token),
        });
    }

    fn render_reaction_bar(
        &self,
        reactions: &gtk::Box,
        channel: &str,
        message_index: usize,
        message: &ChatLine,
        sender: &ComponentSender<Self>,
    ) {
        clear_box(reactions);
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
                        message_index,
                        msgid: msgid.clone(),
                        emoji: emoji.clone(),
                    })
                }
            });
            reactions.append(&button);
        }
    }

    fn render_message_actions(
        &self,
        actions: &gtk::Box,
        channel: &str,
        message_index: usize,
        message: &ChatLine,
        sender: &ComponentSender<Self>,
    ) {
        if !message.id.is_empty() {
            let reply_button = gtk::Button::builder()
                .icon_name("mail-reply-sender-symbolic")
                .tooltip_text("Reply to message")
                .build();
            reply_button.add_css_class("flat");
            reply_button.add_css_class("circular");
            reply_button.connect_clicked({
                let sender = sender.clone();
                let target = channel.to_owned();
                let msgid = message.id.clone();
                let author = message.from.clone();
                move |_| {
                    sender.input(Input::Reply {
                        target: target.clone(),
                        msgid: msgid.clone(),
                        author: author.clone(),
                    });
                }
            });
            actions.append(&reply_button);
            if message.from.eq_ignore_ascii_case(&self.nick) {
                let edit_button = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .tooltip_text("Edit message")
                    .build();
                edit_button.add_css_class("flat");
                edit_button.add_css_class("circular");
                edit_button.connect_clicked({
                    let sender = sender.clone();
                    let target = channel.to_owned();
                    let msgid = message.id.clone();
                    move |_| {
                        sender.input(Input::Edit {
                            target: target.clone(),
                            message_index,
                            msgid: msgid.clone(),
                        });
                    }
                });
                actions.append(&edit_button);
            }
            let picker_button = gtk::Button::builder()
                .icon_name("face-smile-symbolic")
                .tooltip_text("Add reaction")
                .build();
            picker_button.add_css_class("flat");
            picker_button.add_css_class("circular");
            picker_button.connect_clicked({
                let sender = sender.clone();
                let target = channel.to_owned();
                let msgid = message.id.clone();
                move |button| {
                    #[cfg(target_os = "android")]
                    {
                        // GtkEmojiChooser focuses its search entry as it maps,
                        // which raises the soft keyboard. The resulting inset
                        // change resizes the popup surface and GDK's Android
                        // backend destroys it, so the chooser vanishes the
                        // instant it opens. A plain grid needs no text entry.
                        let popover = gtk::Popover::new();
                        // Plain boxes, not a GtkFlowBox: the flow box's own
                        // click gesture swallows presses before they reach the
                        // buttons, so picking an emoji did nothing.
                        let grid = gtk::Box::new(gtk::Orientation::Vertical, 4);
                        let mut row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                        for (index, emoji) in REACTION_EMOJI.into_iter().enumerate() {
                            if index > 0 && index % 6 == 0 {
                                grid.append(&row);
                                row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                            }
                            let choice = gtk::Button::builder().label(emoji).build();
                            choice.add_css_class("flat");
                            choice.connect_clicked({
                                let sender = sender.clone();
                                let target = target.clone();
                                let msgid = msgid.clone();
                                let popover = popover.downgrade();
                                move |_| {
                                    sender.input(Input::React {
                                        target: target.clone(),
                                        message_index,
                                        msgid: msgid.clone(),
                                        emoji: emoji.to_owned(),
                                    });
                                    if let Some(popover) = popover.upgrade() {
                                        popover.popdown();
                                    }
                                }
                            });
                            row.append(&choice);
                        }
                        grid.append(&row);
                        popover.set_child(Some(&grid));
                        popover.connect_closed(|popover| popover.unparent());
                        popover.set_parent(button);
                        popover.popup();
                    }
                    #[cfg(not(target_os = "android"))]
                    {
                        let picker = gtk::EmojiChooser::new();
                        picker.connect_emoji_picked({
                            let sender = sender.clone();
                            let target = target.clone();
                            let msgid = msgid.clone();
                            move |_, emoji| {
                                sender.input(Input::React {
                                    target: target.clone(),
                                    message_index,
                                    msgid: msgid.clone(),
                                    emoji: emoji.to_owned(),
                                });
                            }
                        });
                        picker.connect_closed(|picker| picker.unparent());
                        picker.set_parent(button);
                        picker.popup();
                    }
                }
            });
            actions.append(&picker_button);
        }
    }
}

/// Reaction choices for the Android popover, which cannot use GtkEmojiChooser.
#[cfg(target_os = "android")]
const REACTION_EMOJI: [&str; 12] = [
    "\u{1f44d}", "\u{2764}\u{fe0f}", "\u{1f602}", "\u{1f389}", "\u{1f525}", "\u{1f62e}",
    "\u{1f622}", "\u{1f64f}", "\u{1f440}", "\u{1f4af}", "\u{1f600}", "\u{1f629}",
];

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

fn linkify_message(text: &str) -> String {
    let spans = sleek::preview::extract_url_spans(text);
    let mut markup = String::new();
    let mut cursor = 0;
    for span in spans {
        markup.push_str(&gtk::glib::markup_escape_text(&text[cursor..span.start]));
        let url = gtk::glib::markup_escape_text(&span.url);
        markup.push_str(&format!("<a href=\"{url}\">{url}</a>"));
        cursor = span.end;
    }
    markup.push_str(&gtk::glib::markup_escape_text(&text[cursor..]));
    markup
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

pub fn run() {
    adw::init().expect("failed to initialize libadwaita");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
    #[cfg(target_os = "android")]
    install_browser_opener();
    #[cfg(target_os = "android")]
    let app = {
        use relm4::gtk::gio;
        // HANDLES_OPEN has to be set before the application registers, or GDK
        // falls back to activate() and drops the intent's URI on the floor.
        let app = adw::Application::builder()
            .application_id("uk.nandi.sleek")
            .flags(gio::ApplicationFlags::HANDLES_OPEN)
            .build();
        install_deep_link_handler(&app);
        RelmApp::from_app(app)
    };
    #[cfg(not(target_os = "android"))]
    let app = RelmApp::new("uk.nandi.sleek");
    app.run::<App>(());
}

/// Route auth's "open the browser" through GTK.
///
/// The default Android opener needs android-activity's `AndroidApp`, which only
/// the egui frontend stores; here it always fails and sign-in stalls with just
/// the URL printed. GtkFileLauncher's Android path turns the file's URI into the
/// `ACTION_VIEW` intent we want. It needs the GTK thread and a parent window,
/// and auth calls us from the network thread, so hop back to the main context.
#[cfg(target_os = "android")]
fn install_browser_opener() {
    use relm4::gtk::gio;
    sleek::auth::set_browser_opener(|url| {
        let url = url.to_owned();
        relm4::gtk::glib::idle_add_once(move || {
            let Some(window) = relm4::main_application().active_window() else {
                // The Android launch path dereferences the parent's surface.
                eprintln!("sleek: no active window; cannot open {url}");
                return;
            };
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_uri(&url)));
            launcher.launch(Some(&window), gio::Cancellable::NONE, move |result| {
                if let Err(error) = result {
                    eprintln!("sleek: failed to open browser: {error}");
                }
            });
        });
        Ok(())
    });
}

/// `freeq://auth` callbacks arrive as Android VIEW intents.
///
/// GDK hands an intent carrying data to `g_application_open` when the
/// application claims `HANDLES_OPEN`, so no JNI is needed — but the flag has to
/// be set before the app is registered. The URL is parked here rather than
/// pushed straight into the component, because `open` can fire before the
/// component exists; the tick loop drains it.
#[cfg(target_os = "android")]
static PENDING_DEEP_LINK: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(target_os = "android")]
fn take_pending_deep_link() -> Option<String> {
    PENDING_DEEP_LINK.lock().ok()?.take()
}

#[cfg(target_os = "android")]
fn install_deep_link_handler(app: &adw::Application) {
    app.connect_open(|app, files, _hint| {
        for file in files {
            let uri = file.uri();
            relm4::gtk::glib::g_message!("sleek", "deep link: {uri}");
            if uri.starts_with("freeq://") {
                if let Ok(mut pending) = PENDING_DEEP_LINK.lock() {
                    *pending = Some(uri.to_string());
                }
            }
        }
        // A cold start opens instead of activating, so without this the window
        // would never be built and the tick loop would never drain the link.
        app.activate();
    });
}

fn main() {
    run();
}
