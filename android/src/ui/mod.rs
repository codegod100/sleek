//! Screen and widget helpers for the Sleek shell.

mod chat;
mod chats;
mod connect;
mod discover;

mod settings;
mod widgets;

pub use chat::{active_call_panel, chat_screen, ChatAction};
pub use chats::{chats_tab, ChatsAction};
pub use connect::{connect_screen, ConnectAction};
pub use discover::{discover_tab, DiscoverAction};

pub use settings::{settings_tab, SettingsAction};
// Widget helpers are used from screens via `widgets::` or direct calls as needed.
#[allow(unused_imports)]
pub use widgets::{avatar_circle, card, conversation_row, message_bubble, section_label};
