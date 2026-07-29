//! Screen and widget helpers for the Sleek shell.

mod chat;
mod chats;
mod connect;
mod discover;
mod settings;
mod widgets;

pub use chat::chat_screen;
pub use chats::chats_tab;
pub use connect::connect_screen;
pub use discover::discover_tab;
pub use settings::settings_tab;
pub use widgets::{avatar_circle, card, conversation_row, message_bubble, section_label};
