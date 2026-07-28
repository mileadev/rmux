use rmux_proto::PaneTarget;
use tokio::time::{sleep, Duration, Instant};

use super::RequestHandler;

impl RequestHandler {
    pub(crate) async fn wait_for_pane_terminal_for_test(&self, target: &PaneTarget) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let active = {
                let mut state = self.state.lock().await;
                state
                    .clone_pane_master_if_alive(
                        target.session_name(),
                        target.window_index(),
                        target.pane_index(),
                    )
                    .is_ok()
            };
            if active {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for pane {target} terminal to become active"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }

    pub(crate) async fn wait_for_pane_startup_to_finish_for_test(&self, target: &PaneTarget) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let ready = {
                let mut state = self.state.lock().await;
                let terminal_active = state
                    .clone_pane_master_if_alive(
                        target.session_name(),
                        target.window_index(),
                        target.pane_index(),
                    )
                    .is_ok();
                let still_starting = {
                    #[cfg(windows)]
                    {
                        state.pane_is_starting_in_window(
                            target.session_name(),
                            target.window_index(),
                            target.pane_index(),
                        )
                    }
                    #[cfg(not(windows))]
                    {
                        false
                    }
                };
                terminal_active && !still_starting
            };
            if ready {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for pane {target} startup marker to finish"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }

    pub(crate) async fn wait_for_initial_pane_output_to_quiesce_for_test(
        &self,
        target: &PaneTarget,
    ) {
        const QUIET_PERIOD: Duration = Duration::from_secs(1);

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_sequence = None;
        let mut unchanged_since = Instant::now();
        loop {
            let transcript = {
                let state = self.state.lock().await;
                state
                    .transcript_handle(target)
                    .expect("pane transcript must exist while waiting for initial output")
            };
            let sequence = transcript
                .lock()
                .expect("pane transcript mutex must not be poisoned")
                .output_sequence();
            if last_sequence != Some(sequence) {
                last_sequence = Some(sequence);
                unchanged_since = Instant::now();
            }
            if sequence > 0 && unchanged_since.elapsed() >= QUIET_PERIOD {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for pane {target} initial output to quiesce"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }
}
