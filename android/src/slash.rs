//! Compose-box slash commands — mirrors freeq-android `SlashCommandParser`.
//!
//! Desktop host may install a Gleam Wasm implementation via [`install_parse_hook`];
//! Android and unit tests keep the native Rust parser.

use std::sync::OnceLock;

/// A parsed slash-command from the compose box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Join(String),
    PartActive,
    Nick(String),
    Me(String),
    Msg { target: String, text: String },
    Topic(String),
    /// Unknown command: forwarded to the server as a raw IRC line.
    Raw(String),
    /// Recognized command but missing/malformed args — silent no-op.
    Empty,
}

type ParseHook = Box<dyn Fn(&str) -> Result<SlashCommand, String> + Send + Sync>;

static PARSE_HOOK: OnceLock<ParseHook> = OnceLock::new();

/// Install a Gleam (or other) parser used by [`parse`] when present.
pub fn install_parse_hook(hook: ParseHook) {
    let _ = PARSE_HOOK.set(hook);
}

/// Parse a compose-box string that starts with `/`.
///
/// Prefers the installed hook (desktop Gleam guest); falls back to [`parse_native`].
pub fn parse(input: &str) -> SlashCommand {
    if let Some(hook) = PARSE_HOOK.get() {
        match hook(input) {
            Ok(cmd) => return cmd,
            Err(err) => {
                log::warn!("slash parse hook failed ({err}); using native parser");
            }
        }
    }
    parse_native(input)
}

/// Native Rust parser (oracle for tests / Android / hook fallback).
pub fn parse_native(input: &str) -> SlashCommand {
    let without_slash = input.strip_prefix('/').unwrap_or(input);
    let (cmd, arg) = match without_slash.split_once(' ') {
        Some((c, rest)) => (c, Some(rest).filter(|s| !s.is_empty())),
        None => (without_slash, None),
    };
    let cmd = cmd.to_ascii_lowercase();
    if cmd.is_empty() {
        return SlashCommand::Empty;
    }
    match cmd.as_str() {
        "join" => arg
            .map(|a| SlashCommand::Join(a.to_string()))
            .unwrap_or(SlashCommand::Empty),
        "part" | "leave" => SlashCommand::PartActive,
        "nick" => arg
            .map(|a| SlashCommand::Nick(a.to_string()))
            .unwrap_or(SlashCommand::Empty),
        "me" => arg
            .map(|a| SlashCommand::Me(a.to_string()))
            .unwrap_or(SlashCommand::Empty),
        "msg" => {
            let Some(arg) = arg else {
                return SlashCommand::Empty;
            };
            match arg.split_once(' ') {
                Some((target, text)) if !target.is_empty() && !text.is_empty() => {
                    SlashCommand::Msg {
                        target: target.to_string(),
                        text: text.to_string(),
                    }
                }
                _ => SlashCommand::Empty,
            }
        }
        "topic" => arg
            .map(|a| SlashCommand::Topic(a.to_string()))
            .unwrap_or(SlashCommand::Empty),
        _ => SlashCommand::Raw(without_slash.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_join_with_channel() {
        assert_eq!(
            parse_native("/join #freeq"),
            SlashCommand::Join("#freeq".into())
        );
    }

    #[test]
    fn join_with_no_arg_is_empty() {
        assert_eq!(parse_native("/join"), SlashCommand::Empty);
        assert_eq!(parse_native("/join "), SlashCommand::Empty);
    }

    #[test]
    fn part_and_leave_both_resolve_to_part_active() {
        assert_eq!(parse_native("/part"), SlashCommand::PartActive);
        assert_eq!(parse_native("/leave"), SlashCommand::PartActive);
        // Arg is ignored — dispatch uses the active channel.
        assert_eq!(parse_native("/part #ignored"), SlashCommand::PartActive);
    }

    #[test]
    fn parses_nick_change() {
        assert_eq!(
            parse_native("/nick newhandle"),
            SlashCommand::Nick("newhandle".into())
        );
    }

    #[test]
    fn nick_with_no_arg_is_empty() {
        assert_eq!(parse_native("/nick"), SlashCommand::Empty);
    }

    #[test]
    fn parses_me_action() {
        assert_eq!(
            parse_native("/me waves hello"),
            SlashCommand::Me("waves hello".into())
        );
    }

    #[test]
    fn parses_msg_with_target_and_text() {
        assert_eq!(
            parse_native("/msg alice hi there"),
            SlashCommand::Msg {
                target: "alice".into(),
                text: "hi there".into(),
            }
        );
    }

    #[test]
    fn msg_without_text_is_empty() {
        assert_eq!(parse_native("/msg alice"), SlashCommand::Empty);
        assert_eq!(parse_native("/msg"), SlashCommand::Empty);
    }

    #[test]
    fn parses_topic() {
        assert_eq!(
            parse_native("/topic the new topic"),
            SlashCommand::Topic("the new topic".into())
        );
    }

    #[test]
    fn unknown_command_falls_through_to_raw() {
        assert_eq!(
            parse_native("/invite alice #freeq"),
            SlashCommand::Raw("invite alice #freeq".into())
        );
        assert_eq!(
            parse_native("/whois alice"),
            SlashCommand::Raw("whois alice".into())
        );
    }

    #[test]
    fn command_lookup_is_case_insensitive() {
        assert_eq!(parse_native("/JOIN #x"), SlashCommand::Join("#x".into()));
        assert_eq!(parse_native("/Join #x"), SlashCommand::Join("#x".into()));
    }

    #[test]
    fn empty_or_lone_slash_input_is_empty() {
        assert_eq!(parse_native("/"), SlashCommand::Empty);
    }

    #[test]
    fn preserves_trailing_arguments_with_spaces() {
        assert_eq!(
            parse_native("/me does  the  weird   thing"),
            SlashCommand::Me("does  the  weird   thing".into())
        );
    }
}
