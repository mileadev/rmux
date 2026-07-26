use rmux_core::command_parser::{CommandArgument, ParsedCommand, ParsedCommands};

/// Tracks the Windows stdin EOF marker separately from a raw transport EOF.
///
/// Once this stream has run `attach-session` and admitted work before the
/// marker, its transcript must stay open until that bounded batch has
/// completed. Other EOF paths retain the global grace period and detached
/// finite drain.
#[derive(Debug, Default)]
pub(super) struct ControlEofCompletion {
    stdin_eof_marker_seen: bool,
    attach_session_seen: bool,
    active_command_attaches_session: bool,
    finish_admitted_attached_batch: bool,
}

impl ControlEofCompletion {
    pub(super) fn command_started(&mut self, commands: &ParsedCommands) {
        self.active_command_attaches_session = commands_attach_session(commands);
        self.attach_session_seen |= self.active_command_attaches_session;
    }

    pub(super) fn command_finished(&mut self) {
        self.active_command_attaches_session = false;
    }

    pub(super) fn observe_stdin_eof_marker(
        &mut self,
        client_attached: bool,
        admitted_work_pending: bool,
    ) {
        self.stdin_eof_marker_seen = true;
        if self.attach_session_seen && client_attached && admitted_work_pending {
            self.finish_admitted_attached_batch = true;
        }
    }

    pub(super) fn observe_attachment(
        &mut self,
        client_attached: bool,
        admitted_work_pending: bool,
    ) {
        if self.stdin_eof_marker_seen
            && self.attach_session_seen
            && client_attached
            && admitted_work_pending
        {
            self.finish_admitted_attached_batch = true;
        }
    }

    pub(super) fn allows_detached_drain_transition(&self) -> bool {
        !(self.finish_admitted_attached_batch
            || (self.stdin_eof_marker_seen && self.active_command_attaches_session))
    }
}

fn commands_attach_session(commands: &ParsedCommands) -> bool {
    commands.commands().iter().any(command_attaches_session)
}

fn command_attaches_session(command: &ParsedCommand) -> bool {
    command.name() == "attach-session"
        || command.arguments().iter().any(|argument| match argument {
            CommandArgument::Commands(nested) => commands_attach_session(nested),
            CommandArgument::String(_) => false,
        })
}

#[cfg(test)]
mod tests {
    use rmux_core::command_parser::CommandParser;

    use super::ControlEofCompletion;

    fn parse(input: &str) -> rmux_core::command_parser::ParsedCommands {
        CommandParser::new().parse(input).expect("command parses")
    }

    #[test]
    fn argv_attach_batch_does_not_leave_a_completion_barrier() {
        let mut completion = ControlEofCompletion::default();
        completion.command_started(&parse("attach-session -t alpha"));
        completion.observe_stdin_eof_marker(false, true);

        assert!(!completion.allows_detached_drain_transition());

        completion.command_finished();
        completion.observe_attachment(true, false);

        assert!(completion.allows_detached_drain_transition());
    }

    #[test]
    fn pipe_eof_protects_follow_on_frames_admitted_behind_attach() {
        let mut completion = ControlEofCompletion::default();
        completion.command_started(&parse("attach-session -t alpha"));
        completion.observe_stdin_eof_marker(false, true);

        assert!(!completion.allows_detached_drain_transition());

        completion.command_finished();
        completion.observe_attachment(true, true);

        assert!(!completion.allows_detached_drain_transition());
    }

    #[test]
    fn open_pipe_does_not_arm_a_completion_barrier() {
        let mut completion = ControlEofCompletion::default();
        completion.command_started(&parse("attach-session -t alpha"));

        assert!(completion.allows_detached_drain_transition());

        completion.command_finished();
        completion.observe_attachment(true, true);

        assert!(completion.allows_detached_drain_transition());
    }

    #[test]
    fn marker_after_attach_protects_the_active_admitted_frame() {
        let mut completion = ControlEofCompletion::default();
        completion.command_started(&parse("attach-session -t alpha"));
        completion.command_finished();
        completion.observe_attachment(true, false);
        completion.command_started(&parse("set-buffer -b proof done"));
        completion.observe_stdin_eof_marker(true, true);

        assert!(!completion.allows_detached_drain_transition());
    }

    #[test]
    fn preattached_non_attach_eof_keeps_the_detached_finite_drain() {
        let mut completion = ControlEofCompletion::default();
        completion.command_started(&parse("run-shell 'sleep 1'"));
        completion.observe_stdin_eof_marker(true, true);

        assert!(completion.allows_detached_drain_transition());
    }

    #[test]
    fn nested_attach_is_classified_from_typed_commands() {
        let mut completion = ControlEofCompletion::default();
        completion.command_started(&parse(
            "if-shell -F 1 { attach-session -t alpha } { display-message no }",
        ));
        completion.observe_stdin_eof_marker(false, true);

        assert!(!completion.allows_detached_drain_transition());
    }
}
