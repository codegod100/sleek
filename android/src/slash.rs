//! Compose-box slash commands — mirrors freeq-android `SlashCommandParser`.

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

/// Parse a compose-box string that starts with `/`.
pub fn parse(input: &str) -> SlashCommand {
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
                Some((target, text))
                    if !target.is_empty() && !text.is_empty() =>
                {
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
        assert_eq!(parse("/join #freeq"), SlashCommand::Join("#freeq".into()));
    }

    #[test]
    fn join_with_no_arg_is_empty() {
        assert_eq!(parse("/join"), SlashCommand::Empty);
        assert_eq!(parse("/join "), SlashCommand::Empty);
    }

    #[test]
    fn part_and_leave_both_resolve_to_part_active() {
        assert_eq!(parse("/part"), SlashCommand::PartActive);
        assert_eq!(parse("/leave"), SlashCommand::PartActive);
        // Arg is ignored — dispatch uses the active channel.
        assert_eq!(parse("/part #ignored"), SlashCommand::PartActive);
    }

    #[test]
    fn parses_nick_change() {
        assert_eq!(
            parse("/nick newhandle"),
            SlashCommand::Nick("newhandle".into())
        );
    }

    #[test]
    fn nick_with_no_arg_is_empty() {
        assert_eq!(parse("/nick"), SlashCommand::Empty);
    }

    #[test]
    fn parses_me_action() {
        assert_eq!(
            parse("/me waves hello"),
            SlashCommand::Me("waves hello".into())
        );
    }

    #[test]
    fn parses_msg_with_target_and_text() {
        assert_eq!(
            parse("/msg alice hi there"),
            SlashCommand::Msg {
                target: "alice".into(),
                text: "hi there".into(),
            }
        );
    }

    #[test]
    fn msg_without_text_is_empty() {
        assert_eq!(parse("/msg alice"), SlashCommand::Empty);
        assert_eq!(parse("/msg"), SlashCommand::Empty);
    }

    #[test]
    fn parses_topic() {
        assert_eq!(
            parse("/topic the new topic"),
            SlashCommand::Topic("the new topic".into())
        );
    }

    #[test]
    fn unknown_command_falls_through_to_raw() {
        assert_eq!(
            parse("/invite alice #freeq"),
            SlashCommand::Raw("invite alice #freeq".into())
        );
        assert_eq!(
            parse("/whois alice"),
            SlashCommand::Raw("whois alice".into())
        );
    }

    #[test]
    fn command_lookup_is_case_insensitive() {
        assert_eq!(parse("/JOIN #x"), SlashCommand::Join("#x".into()));
        assert_eq!(parse("/Join #x"), SlashCommand::Join("#x".into()));
    }

    #[test]
    fn empty_or_lone_slash_input_is_empty() {
        assert_eq!(parse("/"), SlashCommand::Empty);
    }

    #[test]
    fn preserves_trailing_arguments_with_spaces() {
        assert_eq!(
            parse("/me does  the  weird   thing"),
            SlashCommand::Me("does  the  weird   thing".into())
        );
    }
}
