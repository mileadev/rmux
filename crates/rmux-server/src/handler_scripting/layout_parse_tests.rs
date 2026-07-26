use super::*;

fn token(value: &str) -> String {
    value.to_owned()
}

fn fixture() -> (SessionStore, TargetFindContext) {
    let session_name = rmux_proto::SessionName::new("alpha").expect("valid session name");
    let mut sessions = SessionStore::new();
    sessions
        .create_session(session_name.clone(), TerminalSize { cols: 80, rows: 24 })
        .expect("session creation succeeds");
    let find_context = TargetFindContext::from_target(rmux_proto::Target::Window(
        WindowTarget::with_window(session_name, 0),
    ));
    (sessions, find_context)
}

fn parse(arguments: &[&str]) -> Result<ParsedSelectLayout, RmuxError> {
    let (sessions, find_context) = fixture();
    parse_select_layout(
        CommandTokens::new(arguments.iter().map(|argument| token(argument)).collect()),
        &sessions,
        &find_context,
    )
}

#[test]
fn select_layout_rejects_unknown_option_shapes_before_layout() {
    for (arguments, unknown) in [
        (&["-x"][..], "-x"),
        (&["--bogus"][..], "--bogus"),
        (&["-nx"][..], "-nx"),
    ] {
        assert_eq!(
            parse(arguments).expect_err("unknown option must fail before layout parsing"),
            RmuxError::Server(format!("command select-layout: unknown flag {unknown}"))
        );
    }
}

#[test]
fn select_layout_preserves_legitimate_flags_and_separator() {
    for arguments in [
        &["-E", "-t", "alpha:0"][..],
        &["-n", "-t", "alpha:0"][..],
        &["-o", "-t", "alpha:0"][..],
        &["-p", "-t", "alpha:0"][..],
        &["-t", "alpha:0", "tiled"][..],
        &["-ntalpha:0"][..],
        &["--", "tiled"][..],
    ] {
        parse(arguments).unwrap_or_else(|error| {
            panic!("select-layout rejected legitimate arguments {arguments:?}: {error}")
        });
    }
}

#[test]
fn select_layout_preserves_missing_target_error() {
    assert_eq!(
        parse(&["-t"]).expect_err("-t without a target must fail"),
        RmuxError::Server("missing -t target".to_owned())
    );
}
