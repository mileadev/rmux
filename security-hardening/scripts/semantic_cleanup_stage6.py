#!/usr/bin/env python3
"""Remove residual WebShare call sites; restore and rename generic local identity guard code."""
from __future__ import annotations
import os,re,subprocess,sys
from pathlib import Path
BASE='dfd68c774ca0f4212139a21d37d09c90f75f8bd7'

def atomic(p: Path,text: str) -> None:
    p.parent.mkdir(parents=True,exist_ok=True)
    tmp=p.with_name(p.name+'.rmux-stage6-tmp'); tmp.write_text(text,encoding='utf-8'); os.replace(tmp,p)
def edit(p: Path,fn) -> None:
    if not p.exists(): return
    old=p.read_text(encoding='utf-8'); new=fn(old)
    if new!=old: atomic(p,new); print(f'edited {p}')
def baseline(root: Path,rel: str) -> str:
    return subprocess.check_output(['git','show',f'{BASE}:{rel}'],cwd=root,text=True)

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()

    # This module was badly named upstream: it is a generic local request/attach identity guard,
    # not a WebShare transport. Restore it under a neutral name because local attach depends on it.
    identity=root/'crates/rmux-server/src/handler/request_identity.rs'
    if not identity.exists():
        text=baseline(root,'crates/rmux-server/src/handler/web_request_identity.rs')
        text=text.replace('web_request_identity/test_support.rs','request_identity/test_support.rs')
        atomic(identity,text); print('restored generic local request identity guard under neutral name')
    test_support=root/'crates/rmux-server/src/handler/request_identity/test_support.rs'
    if not test_support.exists():
        atomic(test_support,baseline(root,'crates/rmux-server/src/handler/web_request_identity/test_support.rs'))
        print('restored request identity tests under neutral path')

    def handler(t: str) -> str:
        if 'mod request_identity;' not in t:
            anchor='mod request_identity;\n'
            # place close to other handler support modules; exact position is semantically irrelevant.
            pos=t.find('mod request_identity;')
            if pos < 0:
                marker='mod request_identity;'
                # Insert before first `use` after module declarations when possible.
                idx=t.find('\nuse ')
                if idx >= 0: t=t[:idx]+'\nmod request_identity;\n'+t[idx:]
        t=t.replace('web_request_identity::{','request_identity::{')
        return t
    edit(root/'crates/rmux-server/src/handler.rs',handler)

    edit(root/'src/cli_args/queue.rs',lambda t: t.replace('        "web-share" => super::web::parse_web_share_args(arguments).map(Command::WebShare),\n',''))
    edit(root/'src/cli_args/completion.rs',lambda t: t.replace('        "web-share" => completion_typed_subcommand::<WebShareArgs>(entry.name),\n',''))
    edit(root/'crates/rmux-sdk/src/lib.rs',lambda t: re.sub(r'\n#\[cfg\(feature = "web"\)\]\npub use rmux_proto::\{WebTerminalPalette, WebTerminalTheme\};','',re.sub(r'\n#\[cfg\(feature = "web"\)\]\npub use web_share::\{.*?\n\};','',t,flags=re.S)))

    def upgrade(t: str) -> str:
        t=t.replace('use rmux_proto::{Response, CAPABILITY_WEB_SHARE};\n','')
        t=t.replace('        upgrade::DaemonFreshness::Current => {\n            ensure_required_web_capability_or_restart(connection, socket_path, binary_path, config)\n        }','        upgrade::DaemonFreshness::Current => {\n            Ok(ReadyServerConnection::new(connection, socket_path))\n        }')
        t=re.sub(r'\n#\[cfg\(windows\)\]\npub\(super\) fn ensure_daemon_fresh_or_restart_after_windows_readiness\([\s\S]*?\n\}\n\nfn ensure_required_web_capability_or_restart\([\s\S]*?\n\}\n\nfn restart_hidden_daemon', '\nfn restart_hidden_daemon',t)
        return t
    edit(root/'crates/rmux-client/src/auto_start/upgrade_restart.rs',upgrade)

    edit(root/'crates/rmux-server/src/handler_daemon.rs',lambda t: t.replace('        let shutdown =\n            session_count == 0 && client_count == 0 && !self.has_persistent_web_listener();','        let shutdown = session_count == 0 && client_count == 0;'))

    def dispatch(t: str) -> str:
        t=t.replace('            Request::WebShare(request) => {\n                HandleOutcome::response(self.handle_web_share(*request).await)\n            }','            Request::ReservedRemoteAccessRemoved => {\n                HandleOutcome::response(Response::Error(ErrorResponse {\n                    error: RmuxError::Server("reserved remote-access request is permanently unsupported".to_owned()),\n                }))\n            }')
        t=t.replace('capabilities_for_features(cfg!(all(any(unix, windows), feature = "web")))','capabilities_for_features(false)')
        return t
    edit(root/'crates/rmux-server/src/handler_dispatch.rs',dispatch)

    edit(root/'crates/rmux-server/src/listener.rs',lambda t: re.sub(r'\n        Request::WebShare\(web_share\)\n            if matches!\(web_share\.as_ref\(\), rmux_proto::WebShareRequest::Create\(_\)\) =>\n        \{\n            RequestQuiesceBehavior::CancelSafe\n        \}','',t))

    # Remove registry-maintenance callbacks that only serviced the deleted browser-share registry.
    for rel in [
        'crates/rmux-server/src/handler_pane/by_id.rs',
        'crates/rmux-server/src/handler_pane/lifecycle.rs',
        'crates/rmux-server/src/handler_pane/management.rs',
        'crates/rmux-server/src/handler_pane/transfer.rs',
        'crates/rmux-server/src/handler_session.rs',
        'crates/rmux-server/src/handler_window/move_window_effects.rs',
        'crates/rmux-server/src/handler_pane_state.rs',
    ]:
        def prune(t: str) -> str:
            t=re.sub(r'\n\s*self\.prune_web_panes\([^;]*\);','',t)
            t=re.sub(r'\n\s*self\.prune_web_session\(Some\(\([\s\S]*?\)\)\);','',t)
            t=re.sub(r'\n\s*self\.rekey_web_session\([^;]*\);','',t)
            # loops whose body was solely prune_web_session
            t=re.sub(r'\n\s*for \([^\n]+\) in [^\{]+\{\n\s*self\.prune_web_session\(Some\(\([\s\S]*?\)\)\);\n\s*\}','',t)
            return t
        edit(root/rel,prune)

    print('stage6 residual call-site cleanup complete'); return 0
if __name__=='__main__': raise SystemExit(main())
