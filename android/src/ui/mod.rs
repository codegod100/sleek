//! Screen and widget helpers for the Sleek shell.

mod chat;
mod chats;
mod connect;
mod discover;
mod policy_gate;
mod profile;
mod search;
mod settings;
mod widgets;

pub use chat::{active_call_panel, chat_screen, ChatAction};
pub use chats::{chats_tab, ChatsAction};
pub use connect::{connect_screen, ConnectAction};
pub use discover::{discover_tab, DiscoverAction};
pub use policy_gate::{open_verification_url, policy_gate_overlay, PolicyGateAction};
pub use profile::{open_profile_url, profile_gate_overlay, ProfileGateAction};
pub use search::{message_search_panel, SearchAction};
pub use settings::{settings_tab, SettingsAction};
// Widget helpers are used from screens via `widgets::` or direct calls as needed.
#[allow(unused_imports)]
pub use widgets::{
    avatar_circle, card, conversation_row, date_separator, format_day_separator,
    image_lightbox_overlay, message_bubble, react_picker_overlay, section_label,
    text_edit_clipboard_menu, user_avatar, MessageBubbleAction,
};
