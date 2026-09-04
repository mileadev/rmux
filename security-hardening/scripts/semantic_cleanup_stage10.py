#!/usr/bin/env python3
"""Delete obsolete remote-access tests and dead helpers exposed by the local-only reduction."""
from __future__ import annotations
import os,re,sys
from pathlib import Path

def atomic(p: Path,text: str) -> None:
    tmp=p.with_name(p.name+'.rmux-stage10-tmp'); tmp.write_text(text,encoding='utf-8'); os.replace(tmp,p)
def edit(p: Path,fn) -> None:
    if not p.exists(): return
    old=p.read_text(encoding='utf-8'); new=fn(old)
    if new!=old: atomic(p,new); print(f'edited {p}')

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()

    def caps(t: str) -> str:
        t=t.replace('        CAPABILITY_SDK_WAITS_ARMED, CAPABILITY_WEB_SHARE,\n','        CAPABILITY_SDK_WAITS_ARMED,\n')
        t=re.sub(r'\n    #\[test\]\n    fn optional_web_capability_follows_the_compiled_feature\(\) \{[\s\S]*?\n    \}\n','\n',t)
        return t
    edit(root/'crates/rmux-proto/src/capabilities.rs',caps)

    def request(t: str) -> str:
        t=t.replace('        SplitDirection, WebShareScope,\n','        SplitDirection,\n')
        t=t.replace(' SourceFile={} WebShare={}",',' SourceFile={}",')
        t=t.replace('            size_of::<SourceFileRequest>(),\n            size_of::<WebShareRequest>(),\n','            size_of::<SourceFileRequest>(),\n')
        t=re.sub(r'\n        assert_box_serializes_like_value\(WebShareRequest::Create\(CreateWebShareRequest \{[\s\S]*?\n        \}\)\);','',t)
        return t
    edit(root/'crates/rmux-proto/src/request.rs',request)

    def response(t: str) -> str:
        t=t.replace('        assert_eq!(size_of::<Box<WebShareResponse>>(), 8);\n','')
        t=re.sub(r'\n        assert!\(\n            size_of::<WebShareResponse>\(\) > size_of::<Response>\(\),\n            "WebShareResponse must remain boxed while it is larger than Response"\n        \);','',t)
        t=re.sub(r'\n        let web_share = WebShareResponse::Config\(WebShareConfigResponse \{[\s\S]*?\n        assert_transparent\(Response::WebShare\(Box::new\(web_share\.clone\(\)\)\), &web_share\);','',t)
        return t
    edit(root/'crates/rmux-proto/src/response.rs',response)

    edit(root/'crates/rmux-server/src/handler.rs',lambda t: t.replace('    dispatch_with_expected_window_identity, dispatch_with_expected_window_occurrence_identity,\n','    dispatch_with_expected_window_occurrence_identity,\n'))

    def attach(t: str) -> str:
        t=re.sub(r'\n\s*validate_attach_recovery_frame\(&render_frame, options\.bounded_recovery\)\?;','',t)
        t=re.sub(r'\nfn validate_attach_recovery_frame\([\s\S]*?\n\}\n\n','\n',t)
        return t
    edit(root/'crates/rmux-server/src/handler_attach.rs',attach)

    def req_identity(t: str) -> str:
        t=re.sub(r'\npub\(in crate::handler\) async fn with_expected_window_identity<T, F>\([\s\S]*?\n\}\n\nasync fn with_expected_window_occurrence_identity','\nasync fn with_expected_window_occurrence_identity',t)
        t=re.sub(r'\npub\(in crate::handler\) async fn dispatch_with_expected_window_identity\([\s\S]*?\n\}\n\npub\(in crate::handler\) async fn dispatch_with_expected_window_occurrence_identity','\npub(in crate::handler) async fn dispatch_with_expected_window_occurrence_identity',t)
        return t
    edit(root/'crates/rmux-server/src/handler/request_identity.rs',req_identity)

    # Remove no-op loops left after WebShare registry pruning.
    for rel in [
        'crates/rmux-server/src/handler_pane/by_id.rs',
        'crates/rmux-server/src/handler_pane/management.rs',
        'crates/rmux-server/src/handler_pane/transfer.rs',
        'crates/rmux-server/src/handler_pane/lifecycle.rs',
    ]:
        edit(root/rel,lambda t: re.sub(r'\n\s*for \([^\n]*\) in [^\{\n]+\{\s*\}','',t))

    edit(root/'crates/rmux-server/src/handler_window/move_window_effects.rs',lambda t: t.replace(
        '                    session_name,\n                    session_id,\n                    detach_on_destroy: _,\n                    event,',
        '                    session_name: _,\n                    session_id: _,\n                    detach_on_destroy: _,\n                    event,'
    ))

    def top_level(t: str) -> str:
        t=re.sub(r'\n/// Applies the top-level execution-mode rules before the public `claude`[\s\S]*?\npub\(super\) fn accept_compatibility_options','\npub(super) fn accept_compatibility_options',t)
        return t
    edit(root/'src/cli/top_level.rs',top_level)
    edit(root/'src/cli/startup.rs',lambda t: t.replace('    let mut config = match selection {','    let config = match selection {'))

    print('stage10 remote-test/dead-code cleanup complete'); return 0
if __name__=='__main__': raise SystemExit(main())
