use std::io;
use std::time::Instant;

use rmux_core::{key_code_lookup_bits, KeyCode};
use rmux_proto::{OptionName, Response};

use super::super::bracketed_paste::{
    decode_bracketed_paste_after_append, take_incomplete_bracketed_paste_segment,
    BracketedPasteDecode,
};
use super::super::retain_partial_attached_control_input;
use super::super::retained::MAX_RETAINED_ATTACHED_CONTROL_INPUT;
use super::{decode_live_attached_key, AttachedKeyDecode};
use crate::handler::attach_support::ActiveAttachIdentity;
use crate::handler::RequestHandler;
use crate::key_table::{
    lookup_attached_key_table_binding, matches_prefix_key, session_option_key, PREFIX_TABLE,
};

struct ReadOnlyDetachSnapshot {
    session_name: rmux_proto::SessionName,
    key_table_name: Option<String>,
    key_table_generation: u64,
    prefix: Option<KeyCode>,
    prefix2: Option<KeyCode>,
    detach_keys: Vec<KeyCode>,
}

impl RequestHandler {
    pub(super) async fn handle_read_only_detach_input(
        &self,
        identity: ActiveAttachIdentity,
        pending_input: &mut Vec<u8>,
        bytes: &[u8],
        backspace: Option<u8>,
        mut key_override: Option<KeyCode>,
    ) -> io::Result<()> {
        let Some(snapshot) = self.read_only_detach_snapshot(identity).await? else {
            pending_input.clear();
            return Ok(());
        };
        let initial_table_name = snapshot.key_table_name.clone();
        let mut prefix_pending = initial_table_name.as_deref() == Some(PREFIX_TABLE);
        let new_input_at = pending_input.len();
        pending_input.extend_from_slice(bytes);
        let mut offset = 0;
        let mut detach = false;

        while offset < pending_input.len() {
            let slice = &pending_input[offset..];
            let slice_new_input_at = new_input_at.saturating_sub(offset);
            match decode_bracketed_paste_after_append(slice, slice_new_input_at) {
                BracketedPasteDecode::Matched { size, .. } => {
                    prefix_pending = false;
                    offset += size;
                    continue;
                }
                BracketedPasteDecode::Partial => {
                    pending_input.drain(..offset);
                    let _ = take_incomplete_bracketed_paste_segment(
                        pending_input,
                        MAX_RETAINED_ATTACHED_CONTROL_INPUT,
                    );
                    retain_partial_attached_control_input(
                        "read-only live bracketed paste",
                        pending_input,
                    )?;
                    self.commit_read_only_prefix_state(
                        identity,
                        &snapshot,
                        &initial_table_name,
                        prefix_pending,
                        false,
                    )
                    .await?;
                    return Ok(());
                }
                BracketedPasteDecode::NotPaste => {}
            }

            let (size, key) = match decode_live_attached_key(slice, backspace) {
                AttachedKeyDecode::Matched { size, key } => (size, key),
                AttachedKeyDecode::Partial => {
                    pending_input.drain(..offset);
                    retain_partial_attached_control_input("read-only live key", pending_input)?;
                    self.commit_read_only_prefix_state(
                        identity,
                        &snapshot,
                        &initial_table_name,
                        prefix_pending,
                        false,
                    )
                    .await?;
                    return Ok(());
                }
                AttachedKeyDecode::Invalid => {
                    pending_input.clear();
                    prefix_pending = false;
                    self.commit_read_only_prefix_state(
                        identity,
                        &snapshot,
                        &initial_table_name,
                        prefix_pending,
                        false,
                    )
                    .await?;
                    return Ok(());
                }
            };
            if size == 0 {
                pending_input.clear();
                prefix_pending = false;
                break;
            }
            offset += size;
            let key = key_override.take().unwrap_or(key);
            let lookup_key = key_code_lookup_bits(key);
            if prefix_pending {
                prefix_pending = false;
                if snapshot.detach_keys.contains(&lookup_key) {
                    detach = true;
                    break;
                }
            } else if matches_prefix_key(lookup_key, snapshot.prefix, snapshot.prefix2) {
                prefix_pending = true;
            }
        }

        pending_input.clear();
        if detach {
            if !self
                .commit_read_only_prefix_state(
                    identity,
                    &snapshot,
                    &initial_table_name,
                    false,
                    true,
                )
                .await?
            {
                return Ok(());
            }
            if !self.current_live_attach_input(identity).await {
                return Ok(());
            }
            if let Response::Error(error) = self.handle_detach_client_for_identity(identity).await {
                return Err(io::Error::other(error.error.to_string()));
            }
            return Ok(());
        }

        let _ = self
            .commit_read_only_prefix_state(
                identity,
                &snapshot,
                &initial_table_name,
                prefix_pending,
                false,
            )
            .await?;
        Ok(())
    }

    async fn read_only_detach_snapshot(
        &self,
        identity: ActiveAttachIdentity,
    ) -> io::Result<Option<ReadOnlyDetachSnapshot>> {
        let state = self.state.lock().await;
        let active_attach = self.active_attach.lock().await;
        let active = active_attach
            .by_pid
            .get(&identity.attach_pid())
            .filter(|active| {
                identity.matches_active(active)
                    && !active.closing.load(std::sync::atomic::Ordering::SeqCst)
            })
            .ok_or_else(|| io::Error::other("attached client disappeared"))?;
        let Some(session) = state.sessions.session(&active.session_name) else {
            return Ok(None);
        };
        if session.id() != active.session_id {
            return Ok(None);
        }

        let bound_keys = state
            .key_bindings
            .table(PREFIX_TABLE)
            .into_iter()
            .flat_map(|table| table.active().keys())
            .copied()
            .collect::<Vec<_>>();
        let detach_keys = bound_keys
            .into_iter()
            .filter_map(|key| {
                lookup_attached_key_table_binding(&state, PREFIX_TABLE, key_code_lookup_bits(key))
                    .is_some_and(|binding| {
                        let commands = binding.commands().commands();
                        commands.len() == 1
                            && commands[0].name() == "detach-client"
                            && commands[0].arguments().is_empty()
                    })
                    .then_some(key_code_lookup_bits(key))
            })
            .collect();

        Ok(Some(ReadOnlyDetachSnapshot {
            session_name: active.session_name.clone(),
            key_table_name: active.key_table_name.clone(),
            key_table_generation: active.key_table_generation,
            prefix: session_option_key(&state, &active.session_name, OptionName::Prefix),
            prefix2: session_option_key(&state, &active.session_name, OptionName::Prefix2),
            detach_keys,
        }))
    }

    async fn commit_read_only_prefix_state(
        &self,
        identity: ActiveAttachIdentity,
        snapshot: &ReadOnlyDetachSnapshot,
        initial_table_name: &Option<String>,
        prefix_pending: bool,
        force_generation_check: bool,
    ) -> io::Result<bool> {
        let next_table_name = prefix_pending.then(|| PREFIX_TABLE.to_owned());
        if !force_generation_check && initial_table_name == &next_table_name {
            return Ok(true);
        }
        let set_at = prefix_pending.then(Instant::now);
        let commit = self
            .set_attached_key_table_for_read_only_input(
                identity,
                &snapshot.session_name,
                snapshot.key_table_generation,
                next_table_name,
                set_at,
            )
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        let Some(commit) = commit else {
            return Ok(false);
        };
        if let Some(set_at) = set_at {
            if commit.prefix_timeout_ms != 0 {
                self.schedule_attached_prefix_timeout_for_identity(
                    commit.identity,
                    set_at,
                    commit.key_table_generation,
                    commit.prefix_timeout_ms,
                );
            }
        }
        Ok(true)
    }

    pub(super) async fn dispatch_immediate_prefix_detach(
        &self,
        identity: ActiveAttachIdentity,
        target: &rmux_proto::PaneTarget,
        bytes: &[u8],
        backspace: Option<u8>,
    ) -> io::Result<bool> {
        let AttachedKeyDecode::Matched {
            size: prefix_size,
            key: prefix_key,
        } = decode_live_attached_key(bytes, backspace)
        else {
            return Ok(false);
        };
        if prefix_size == 0 || prefix_size >= bytes.len() {
            return Ok(false);
        }

        let AttachedKeyDecode::Matched {
            size: command_size,
            key: command_key,
        } = decode_live_attached_key(&bytes[prefix_size..], backspace)
        else {
            return Ok(false);
        };
        if prefix_size.saturating_add(command_size) != bytes.len() {
            return Ok(false);
        }

        let is_bare_detach_binding = {
            let state = self.state.lock().await;
            let prefix = session_option_key(
                &state,
                target.session_name(),
                rmux_proto::OptionName::Prefix,
            );
            let prefix2 = session_option_key(
                &state,
                target.session_name(),
                rmux_proto::OptionName::Prefix2,
            );
            if !matches_prefix_key(prefix_key, prefix, prefix2) {
                return Ok(false);
            }
            lookup_attached_key_table_binding(
                &state,
                PREFIX_TABLE,
                key_code_lookup_bits(command_key),
            )
            .is_some_and(|binding| {
                let commands = binding.commands().commands();
                commands.len() == 1
                    && commands[0].name() == "detach-client"
                    && commands[0].arguments().is_empty()
            })
        };
        if !is_bare_detach_binding {
            return Ok(false);
        }

        if !self.current_live_attach_input(identity).await {
            return Ok(false);
        }
        match self.handle_detach_client_for_identity(identity).await {
            Response::Error(error) => Err(io::Error::other(error.error.to_string())),
            _ => Ok(true),
        }
    }
}
