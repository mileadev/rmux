#!/usr/bin/env python3
from __future__ import annotations
import os,re,sys
from pathlib import Path

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    p=root/'crates/rmux-server/src/handler.rs'
    t=p.read_text(encoding='utf-8'); old=t
    t=t.replace('#[path = "handler/web_request_identity.rs"]\nmod web_request_identity;\n','')
    t=re.sub(r'#\[cfg\(all\(any\(unix, windows\), feature = "web"\)\)\]\n#\[path = "handler_web\.rs"\]\nmod web_support;\n','',t)
    t=re.sub(r'#\[cfg\(not\(all\(any\(unix, windows\), feature = "web"\)\)\)\]\n#\[path = "handler_web_disabled\.rs"\]\nmod web_support;\n','',t)
    t=re.sub(r'#\[cfg\(all\(test, any\(unix, windows\), feature = "web"\)\)\]\npub\(crate\) use web_support::[^;]+;\n','',t)
    t=re.sub(r'#\[cfg\(all\(any\(unix, windows\), feature = "web"\)\)\]\npub\(crate\) use web_support::\{.*?\n\};\n','',t,flags=re.S)
    t=re.sub(r'#\[cfg\(all\(any\(unix, windows\), feature = "web"\)\)\]\nuse crate::web::WebShareRegistry;\n','',t)
    if t!=old:
        tmp=p.with_name(p.name+'.rmux-stage4-tmp'); tmp.write_text(t,encoding='utf-8'); os.replace(tmp,p); print(f'edited {p}')
    print('stage4 cleanup complete'); return 0
if __name__=='__main__': raise SystemExit(main())
