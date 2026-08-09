//! Desktop host: `just host` or `cargo run --manifest-path host/Cargo.toml`
//!
//! Builds / loads the Gleam Wasm slash guest (`just gleam-slash` / `--gleam-slash-only`).

mod gleam_slash;
mod gleam_string;

fn main() -> eframe::Result {
    let slash_only = std::env::args().any(|a| a == "--gleam-slash-only");

    match smoke_gleam_slash() {
        Ok(summary) => {
            println!("gleam slash smoke → {summary}");
            if slash_only {
                return Ok(());
            }
        }
        Err(err) => {
            eprintln!("gleam slash smoke failed: {err}");
            if slash_only {
                std::process::exit(1);
            }
        }
    }

    install_gleam_slash_hook();
    sleek::run_desktop()
}

fn install_gleam_slash_hook() {
    sleek::slash::install_parse_hook(Box::new(|input: &str| {
        gleam_slash::parse(input).map(into_sleek_slash)
    }));
}

fn into_sleek_slash(cmd: gleam_slash::SlashCommand) -> sleek::slash::SlashCommand {
    use gleam_slash::SlashCommand as G;
    use sleek::slash::SlashCommand as S;
    match cmd {
        G::Join(c) => S::Join(c),
        G::PartActive => S::PartActive,
        G::Nick(n) => S::Nick(n),
        G::Me(a) => S::Me(a),
        G::Msg { target, text } => S::Msg { target, text },
        G::Topic(t) => S::Topic(t),
        G::Raw(r) => S::Raw(r),
        G::Empty => S::Empty,
    }
}

fn smoke_gleam_slash() -> Result<String, String> {
    let cases: &[(&str, gleam_slash::SlashCommand)] = &[
        ("/join #freeq", gleam_slash::SlashCommand::Join("#freeq".into())),
        ("/join", gleam_slash::SlashCommand::Empty),
        ("/part", gleam_slash::SlashCommand::PartActive),
        ("/leave", gleam_slash::SlashCommand::PartActive),
        (
            "/nick newhandle",
            gleam_slash::SlashCommand::Nick("newhandle".into()),
        ),
        (
            "/me waves hello",
            gleam_slash::SlashCommand::Me("waves hello".into()),
        ),
        (
            "/msg alice hi there",
            gleam_slash::SlashCommand::Msg {
                target: "alice".into(),
                text: "hi there".into(),
            },
        ),
        ("/msg alice", gleam_slash::SlashCommand::Empty),
        (
            "/topic the new topic",
            gleam_slash::SlashCommand::Topic("the new topic".into()),
        ),
        (
            "/invite alice #freeq",
            gleam_slash::SlashCommand::Raw("invite alice #freeq".into()),
        ),
        ("/JOIN #x", gleam_slash::SlashCommand::Join("#x".into())),
        ("/", gleam_slash::SlashCommand::Empty),
        (
            "/me does  the  weird   thing",
            gleam_slash::SlashCommand::Me("does  the  weird   thing".into()),
        ),
    ];

    for &(input, ref expected) in cases {
        let native = sleek::slash::parse_native(input);
        let gleam = gleam_slash::parse(input)?;
        let gleam_as_sleek = into_sleek_slash(gleam.clone());
        if gleam_as_sleek != native {
            return Err(format!(
                "{input:?}: gleam={gleam_as_sleek:?} native={native:?}"
            ));
        }
        if &gleam != expected {
            return Err(format!(
                "{input:?}: gleam got {gleam:?}, expected {expected:?}"
            ));
        }
    }

    Ok(format!("{} cases match native oracle", cases.len()))
}
