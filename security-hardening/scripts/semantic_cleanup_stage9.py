#!/usr/bin/env python3
"""Rebuild listener.rs from the last clean local/Web-split version and collapse it to local-only.

`tokio::select!` contains cfg-gated expression fragments that cannot be safely removed by the
Stage8 source-unit stripper. This script deliberately starts from the known-good pre-Stage8 commit,
removes the Web listener/dispatch guard paths with exact transforms, and requires zero Web feature
cfgs to remain.
"""
from __future__ import annotations
import os,re,subprocess,sys
from pathlib import Path

SOURCE='651dc6e0c6ab136475ea201ff0e909d0fe8067e4'
REL='crates/rmux-server/src/listener.rs'

def atomic(path: Path,text: str) -> None:
    tmp=path.with_name(path.name+'.rmux-stage9-tmp'); tmp.write_text(text,encoding='utf-8'); os.replace(tmp,path)

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    text=subprocess.check_output(['git','show',f'{SOURCE}:{REL}'],cwd=root,text=True)

    # Local server construction only. No port/front-end settings, no web-required listener startup.
    text=re.sub(
        r'\n    #\[cfg\(all\(any\(unix, windows\), feature = "web"\)\)\]\n    let web_required = options\.web_required;\n'
        r'    #\[cfg\(all\(any\(unix, windows\), feature = "web"\)\)\]\n    let handler = Arc::new\([\s\S]*?\n    \);\n'
        r'    #\[cfg\(not\(all\(any\(unix, windows\), feature = "web"\)\)\)\]\n    let handler = Arc::new\(RequestHandler::with_owner_uid_and_subscription_limits\(\n'
        r'        options\.owner_uid,\n        options\.subscription_limits,\n    \)\);',
        '\n    let handler = Arc::new(RequestHandler::with_owner_uid_and_subscription_limits(\n        options.owner_uid,\n        options.subscription_limits,\n    ));',
        text,
    )
    text=re.sub(
        r'\n    #\[cfg\(all\(any\(unix, windows\), feature = "web"\)\)\]\n    if web_required \{[\s\S]*?\n    \}\n',
        '\n',text,
    )

    # Local detached request dispatch only; preserve cancellation semantics around the future.
    text=text.replace('                #[cfg(feature = "web")]\n                let mut undelivered_web_share;\n','')
    text=re.sub(
        r'                        #\[cfg\(feature = "web"\)\]\n'
        r'                        let dispatch = handler\.dispatch_for_connection_with_web_share_guard\([\s\S]*?\n                        \);\n'
        r'                        #\[cfg\(not\(feature = "web"\)\)\]\n'
        r'                        let dispatch = handler\.dispatch_for_connection\(\n'
        r'                            requester\.pid,\n                            connection_id,\n                            request,\n                        \);',
        '                        let dispatch = handler.dispatch_for_connection(\n                            requester.pid,\n                            connection_id,\n                            request,\n                        );',
        text,
    )
    text=re.sub(
        r'                            \) => \{\n'
        r'                                #\[cfg\(feature = "web"\)\]\n'
        r'                                \{\n                                    undelivered_web_share = outcome\.1;\n                                    outcome\.0\n                                \}\n'
        r'                                #\[cfg\(not\(feature = "web"\)\)\]\n                                outcome\n                            \},',
        '                            ) => outcome,',
        text,
    )
    text=re.sub(
        r'\n                #\[cfg\(feature = "web"\)\]\n                if let Some\(guard\) = undelivered_web_share\.as_mut\(\) \{\n                    guard\.disarm\(\);\n                \}\n',
        '\n',text,
    )

    # Windows-only cleanup is handled separately; this stage's hard assertion is specifically that
    # no remote/Web feature gate survives in the transport listener.
    leftovers=[line for line in text.splitlines() if 'feature = "web"' in line or 'feature="web"' in line]
    if leftovers:
        print('unhandled listener Web cfgs:',file=sys.stderr)
        for line in leftovers: print(line,file=sys.stderr)
        return 2
    if 'WebShare' in text or 'web_share' in text or 'ensure_web_share_listener' in text:
        print('unhandled listener WebShare identifier remains',file=sys.stderr)
        return 3

    path=root/REL
    current=path.read_text(encoding='utf-8') if path.exists() else ''
    if current!=text:
        atomic(path,text); print('rebuilt listener.rs as explicit local-only transport')
    else:
        print('listener.rs already local-only')
    return 0

if __name__=='__main__': raise SystemExit(main())
