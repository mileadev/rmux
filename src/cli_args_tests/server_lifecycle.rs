use super::*;

#[test]
fn build_scope_global_produces_global_selector() {
    let scope = super::super::build_scope(true, None);
    assert!(matches!(scope, rmux_proto::ScopeSelector::Global));
}

#[test]
fn build_scope_target_produces_session_selector() {
    let name = rmux_proto::SessionName::new("test").unwrap();
    let scope = super::super::build_scope(false, Some(name.clone()));
    assert!(matches!(scope, rmux_proto::ScopeSelector::Session(n) if n == name));
}

#[test]
fn start_server_parses_without_arguments() {
    let cli = parse_args(&["start-server"]).unwrap();

    assert!(matches!(
        cli.command,
        Some(super::super::Command::StartServer(_))
    ));
}

#[test]
fn start_server_rejects_extra_arguments() {
    let error = parse_args(&["start-server", "extra"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn kill_server_parses_without_arguments() {
    let cli = parse_args(&["kill-server"]).unwrap();

    assert!(matches!(
        cli.command,
        Some(super::super::Command::KillServer)
    ));
}

#[test]
fn kill_server_rejects_extra_arguments() {
    let error = parse_args(&["kill-server", "extra"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn server_access_list_parses_without_user() {
    let cli = parse_args(&["server-access", "-l"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.list);
            assert_eq!(args.user, None);
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_read_only_parses_with_user() {
    let cli = parse_args(&["server-access", "-r", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.read_only);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_rejects_combined_add_and_deny_flags() {
    let error = parse_args(&["server-access", "-a", "-d", "alice"]).unwrap_err();

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    assert!(error
        .to_string()
        .contains("-a and -d cannot be used together"));
}

#[test]
fn server_access_rejects_combined_read_and_write_flags() {
    let error = parse_args(&["server-access", "-r", "-w", "alice"]).unwrap_err();

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    assert!(error
        .to_string()
        .contains("-r and -w cannot be used together"));
}

#[test]
fn server_access_list_accepts_ignored_user_argument() {
    let cli = parse_args(&["server-access", "-l", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.list);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_list_ignores_otherwise_conflicting_flags() {
    let cli = parse_args(&["server-access", "-l", "-a", "-d", "-r", "-w", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.list);
            assert!(args.add);
            assert!(args.deny);
            assert!(args.read_only);
            assert!(args.write);
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_missing_user_is_a_runtime_error() {
    let cli = parse_args(&["server-access", "-r"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.read_only);
            assert_eq!(args.user, None);
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_rejects_tmux_target_flag_like_tmux_runtime() {
    let error = parse_args(&["server-access", "-t", "%0", "-l"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    assert!(error
        .to_string()
        .contains("command server-access: unknown flag -t"));

    let error = parse_args(&["server-access", "-t%0", "root"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    assert!(error
        .to_string()
        .contains("command server-access: unknown flag -t"));

    let error = parse_args(&["server-access", "-xt", "root"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    assert!(error
        .to_string()
        .contains("command server-access: unknown flag -x"));

    let error = parse_args(&["server-access", "--target", "%0", "root"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    assert!(error
        .to_string()
        .contains("command server-access: invalid flag --"));
}

#[test]
fn server_access_help_and_completion_omit_rejected_target_flag() {
    let help =
        parse_args(&["server-access", "--help"]).expect_err("--help renders server-access help");
    assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
    let help = help.to_string();
    assert!(help.lines().any(|line| line.trim_start().starts_with("-a")));
    assert!(help.lines().any(|line| line.trim_start().starts_with("-w")));
    assert!(
        !help.lines().any(|line| line.trim_start().starts_with("-t")),
        "server-access help advertised rejected -t: {help}"
    );

    let completion = super::super::completion_command();
    let server_access = completion
        .get_subcommands()
        .find(|command| command.get_name() == "server-access")
        .expect("server-access completion subcommand");
    let visible_flags = server_access
        .get_arguments()
        .filter(|argument| !argument.is_hide_set())
        .filter_map(|argument| argument.get_short())
        .collect::<Vec<_>>();
    for flag in ['a', 'd', 'l', 'r', 'w'] {
        assert!(
            visible_flags.contains(&flag),
            "missing completion flag -{flag}"
        );
    }
    assert!(
        !visible_flags.contains(&'t'),
        "server-access completion advertised rejected -t"
    );
}

#[test]
fn server_access_rejects_bare_dash() {
    let error = parse_args(&["server-access", "-"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    assert!(error
        .to_string()
        .contains("command server-access: invalid flag -"));
}

#[test]
fn lock_server_parses_without_arguments() {
    let cli = parse_args(&["lock-server"]).unwrap();

    assert!(matches!(
        cli.command,
        Some(super::super::Command::LockServer)
    ));
}

#[test]
fn lock_session_parses_target() {
    let cli = parse_args(&["lock-session", "-t", "alpha"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::LockSession(args) => {
            assert_eq!(args.target.as_ref().expect("target").to_string(), "alpha")
        }
        _ => panic!("expected LockSession command"),
    }
}

#[test]
fn lock_client_parses_client_target() {
    let cli = parse_args(&["lock-client", "-t", "="]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::LockClient(args) => assert_eq!(args.target.as_deref(), Some("=")),
        _ => panic!("expected LockClient command"),
    }
}

#[test]
fn lock_server_rejects_extra_arguments() {
    let error = parse_args(&["lock-server", "extra"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn lock_session_allows_implicit_current_session_target() {
    let cli = parse_args(&["lock-session"]).unwrap();
    match cli.command.expect("parsed command") {
        super::super::Command::LockSession(args) => assert!(args.target.is_none()),
        _ => panic!("expected LockSession command"),
    }
}

#[test]
fn lock_client_defaults_to_current_client() {
    let cli = parse_args(&["lock-client"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::LockClient(args) => assert_eq!(args.target, None),
        _ => panic!("expected LockClient command"),
    }
}

#[test]
fn server_access_add_parses_with_user() {
    let cli = parse_args(&["server-access", "-a", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.add);
            assert!(!args.deny);
            assert!(!args.list);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_deny_parses_with_user() {
    let cli = parse_args(&["server-access", "-d", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.deny);
            assert!(!args.add);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_write_parses_with_user() {
    let cli = parse_args(&["server-access", "-w", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.write);
            assert!(!args.read_only);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_bare_user_parses() {
    let cli = parse_args(&["server-access", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(!args.add);
            assert!(!args.deny);
            assert!(!args.list);
            assert!(!args.read_only);
            assert!(!args.write);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_list_accepts_add_flag() {
    let cli = parse_args(&["server-access", "-l", "-a", "alice"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.list);
            assert!(args.add);
            assert_eq!(args.user.as_deref(), Some("alice"));
        }
        _ => panic!("expected ServerAccess command"),
    }
}

#[test]
fn server_access_list_accepts_deny_flag_without_user() {
    let cli = parse_args(&["server-access", "-l", "-d"]).unwrap();

    match cli.command.expect("parsed command") {
        super::super::Command::ServerAccess(args) => {
            assert!(args.list);
            assert!(args.deny);
            assert_eq!(args.user, None);
        }
        _ => panic!("expected ServerAccess command"),
    }
}
