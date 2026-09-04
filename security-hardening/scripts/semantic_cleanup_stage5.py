#!/usr/bin/env python3
"""Remove WebShare protocol/capability surface while preserving wire ordinals as reserved tombstones."""
from __future__ import annotations
import os,re,sys
from pathlib import Path

TOMBSTONE='ReservedRemoteAccessRemoved'

def atomic(p: Path,text: str) -> None:
    tmp=p.with_name(p.name+'.rmux-stage5-tmp'); tmp.write_text(text,encoding='utf-8'); os.replace(tmp,p)

def edit(p: Path,fn) -> None:
    if not p.exists(): return
    old=p.read_text(encoding='utf-8'); new=fn(old)
    if new!=old: atomic(p,new); print(f'edited {p}')

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()

    def request(t: str) -> str:
        marker='    /// Internal idle-only shutdown endpoint used by seamless upgrades.\n    ShutdownIfIdle(ShutdownIfIdleRequest),\n'
        tomb='    /// Permanently reserved former remote-access wire slot. No payload or behavior.\n    ReservedRemoteAccessRemoved,\n'
        if TOMBSTONE not in t and marker in t: t=t.replace(marker,marker+tomb)
        t=t.replace('            Self::WebShare(_) => "web-share",\n','')
        anchor='            Self::ShutdownIfIdle(_) => "shutdown-if-idle",\n'
        if 'Self::ReservedRemoteAccessRemoved =>' not in t and anchor in t:
            t=t.replace(anchor,anchor+'            Self::ReservedRemoteAccessRemoved => "__reserved-remote-access-removed",\n')
        return t
    edit(root/'crates/rmux-proto/src/request.rs',request)

    def response(t: str) -> str:
        marker='    /// Success payload for internal idle-only daemon shutdown.\n    ShutdownIfIdle(ShutdownIfIdleResponse),\n'
        tomb='    /// Permanently reserved former remote-access wire slot. Never emitted.\n    ReservedRemoteAccessRemoved,\n'
        if TOMBSTONE not in t and marker in t: t=t.replace(marker,marker+tomb)
        anchor='            Self::ShutdownIfIdle(_) => "shutdown-if-idle",\n'
        if 'Self::ReservedRemoteAccessRemoved =>' not in t and anchor in t:
            t=t.replace(anchor,anchor+'            Self::ReservedRemoteAccessRemoved => "__reserved-remote-access-removed",\n')
        t=t.replace('            Self::WebShare(response) => response.command_output(),\n','')
        none_anchor='            | Self::ShutdownIfIdle(_) => None,\n'
        if '| Self::ReservedRemoteAccessRemoved => None,' not in t and none_anchor in t:
            t=t.replace(none_anchor,'            | Self::ShutdownIfIdle(_)\n            | Self::ReservedRemoteAccessRemoved => None,\n')
        return t
    edit(root/'crates/rmux-proto/src/response.rs',response)

    def frame(t: str) -> str:
        t=t.replace('    /// Browser-visible pane sharing.\n    Web,\n','')
        t=t.replace('        Request::WebShare(_) => c2s(114),\n',f'        Request::{TOMBSTONE} => c2s(114),\n')
        t=t.replace('        Response::WebShare(_) => s2c(93),\n',f'        Response::{TOMBSTONE} => s2c(93),\n')
        t=re.sub(
            r'    entry\(\n        c2s\(114\),\n        FrameDirection::ClientToServer,\n        ACTIVE,\n        "WebShareRequest",\n        FrameFeature::Web,\n        None,\n        "Browser-visible pane sharing command family; pinned bincode tag 114\.",\n    \),',
            '    entry(\n        c2s(114),\n        FrameDirection::ClientToServer,\n        RESERVED,\n        "(removed-remote-access-request)",\n        FrameFeature::Reserved,\n        None,\n        "Former remote-access request slot. Permanently reserved and never reused.",\n    ),',t)
        t=re.sub(
            r'    entry\(\n        s2c\(93\),\n        FrameDirection::ServerToClient,\n        ACTIVE,\n        "WebShareResponse",\n        FrameFeature::Web,\n        None,\n        "Browser-visible pane sharing command response; pinned bincode tag 93\.",\n    \),',
            '    entry(\n        s2c(93),\n        FrameDirection::ServerToClient,\n        RESERVED,\n        "(removed-remote-access-response)",\n        FrameFeature::Reserved,\n        None,\n        "Former remote-access response slot. Permanently reserved and never reused.",\n    ),',t)
        return t
    edit(root/'crates/rmux-proto/src/frame_kind.rs',frame)

    def caps(t: str) -> str:
        t=re.sub(r'/// Stable feature id for browser-visible pane sharing\.[\s\S]*?pub const CAPABILITY_WEB_SHARE: &str = "web\.share";\n\n','',t)
        t=re.sub(
            r'/// Builds the capability inventory for a binary with the supplied optional[\s\S]*?pub fn capabilities_for_features\(web_share: bool\) -> Vec<&\'static str> \{[\s\S]*?\n\}\n\n',
            '/// Returns the capability inventory for this local-only protocol build.\n#[must_use]\npub fn capabilities_for_features(_removed_remote_access: bool) -> Vec<&\'static str> {\n    SUPPORTED_CAPABILITIES.to_vec()\n}\n\n',t)
        return t
    edit(root/'crates/rmux-proto/src/capabilities.rs',caps)

    edit(root/'crates/rmux-proto/src/lib.rs',lambda t: t.replace('    CAPABILITY_TARGET_CLIENT_COMMANDS, CAPABILITY_WEB_SHARE, SUPPORTED_CAPABILITIES,\n','    CAPABILITY_TARGET_CLIENT_COMMANDS, SUPPORTED_CAPABILITIES,\n'))
    edit(root/'src/cli/capabilities.rs',lambda t: t.replace('    capabilities_for_features(cfg!(all(any(unix, windows), feature = "web")))','    capabilities_for_features(false)'))

    print('stage5 protocol tombstone cleanup complete'); return 0
if __name__=='__main__': raise SystemExit(main())
