#!/usr/bin/env python3
"""Physically remove dead positive `cfg(feature="web")` code and make no-web branches local defaults.

The WebShare implementation, feature definition, and dependencies are already gone. Keeping stale
Web-only cfg blocks would leave dormant source and produce `unexpected_cfgs` warnings. This pass
removes the positive branch as a complete Rust syntactic unit and preserves only the local branch.
"""
from __future__ import annotations
import os,re,sys
from pathlib import Path

POS = re.compile(r'(?m)^(?P<indent>[ \t]*)#\[cfg\((?P<expr>[^\n]*feature\s*=\s*"web"[^\n]*)\)\][ \t]*\n')
ATTR = re.compile(r'(?m)^[ \t]*#\[cfg_attr\([^\n]*feature\s*=\s*"web"[^\n]*\)\][ \t]*\n')

def is_positive(expr: str) -> bool:
    compact=re.sub(r'\s+','',expr)
    if compact.startswith('not(') and 'feature="web"' in compact:
        return False
    return not re.search(r'not\s*\(\s*feature\s*=\s*"web"\s*\)',expr)

def clean_negative_attr(expr: str) -> str:
    compact=re.sub(r'\s+','',expr)
    if compact.startswith('not(') and 'feature="web"' in compact:
        return ''
    expr=re.sub(r'\s*,?\s*not\s*\(\s*feature\s*=\s*"web"\s*\)\s*,?\s*',', ',expr)
    expr=re.sub(r',\s*,+',',',expr).strip(' ,')
    m=re.fullmatch(r'all\((.*)\)',expr)
    if m:
        inner=m.group(1).strip(); depth=0; commas=0
        for ch in inner:
            if ch=='(': depth+=1
            elif ch==')': depth-=1
            elif ch==',' and depth==0: commas+=1
        if commas==0: expr=inner
    if not expr: return ''
    expr=expr.replace('any(unix, windows)','unix')
    return f'#[cfg({expr})]\n'

def skip_space(text: str,i: int) -> int:
    while i<len(text) and text[i].isspace(): i+=1
    return i

def unit_end(text: str,start: int) -> int:
    i=skip_space(text,start)
    while text.startswith('#[',i):
        k=text.find(']\n',i)
        if k<0: break
        i=skip_space(text,k+2)
    while text.startswith('///',i) or text.startswith('//!',i):
        k=text.find('\n',i); i=len(text) if k<0 else skip_space(text,k+1)
    head=text[i:i+200]
    item_like=bool(re.match(r'(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:fn|struct|enum|impl|trait|mod)\b',head))
    prefer_semicolon=bool(re.match(r'(?:pub(?:\([^)]*\))?\s+)?(?:use|const|static|type|let)\b',head))
    par=brack=brace=0; j=i; state='code'; block_comment=0; started_brace=False
    while j<len(text):
        c=text[j]; n=text[j+1] if j+1<len(text) else ''
        if state=='code':
            if c=='/' and n=='/': state='line'; j+=2; continue
            if c=='/' and n=='*': state='block'; block_comment=1; j+=2; continue
            if c=='"': state='string'; j+=1; continue
            if c=='(': par+=1
            elif c==')': par=max(0,par-1)
            elif c=='[': brack+=1
            elif c==']': brack=max(0,brack-1)
            elif c=='{': brace+=1; started_brace=True
            elif c=='}':
                brace=max(0,brace-1)
                if item_like and started_brace and par==0 and brack==0 and brace==0:
                    k=j+1
                    while k<len(text) and text[k] in ' \t': k+=1
                    if k<len(text) and text[k]==';': k+=1
                    if k<len(text) and text[k]=='\n': k+=1
                    return k
            elif c==';' and par==0 and brack==0 and brace==0:
                return j+2 if j+1<len(text) and text[j+1]=='\n' else j+1
            elif c==',' and par==0 and brack==0 and brace==0 and not item_like and not prefer_semicolon:
                return j+2 if j+1<len(text) and text[j+1]=='\n' else j+1
            j+=1
        elif state=='line':
            if c=='\n': state='code'
            j+=1
        elif state=='block':
            if c=='/' and n=='*': block_comment+=1; j+=2
            elif c=='*' and n=='/': block_comment-=1; j+=2; state='code' if block_comment==0 else 'block'
            else: j+=1
        else:
            if c=='\\': j+=2
            elif c=='"': state='code'; j+=1
            else: j+=1
    return len(text)

def clean_text(text: str) -> tuple[str,int]:
    text,nattr=ATTR.subn('',text); count=nattr
    while True:
        m=POS.search(text)
        if not m: break
        expr=m.group('expr')
        if not is_positive(expr):
            repl=clean_negative_attr(expr)
            text=text[:m.start()]+m.group('indent')+repl+text[m.end():]
            count+=1; continue
        end=unit_end(text,m.end()); start=m.start(); p=start
        while p>0:
            prev_nl=text.rfind('\n',0,p-1); line_start=0 if prev_nl<0 else prev_nl+1
            line=text[line_start:p].strip()
            if line.startswith('///') or line.startswith('//'):
                start=line_start; p=line_start
            else: break
        text=text[:start]+text[end:]; count+=1
    text,n=re.subn(r'cfg!\([^\n;]*feature\s*=\s*"web"[^\n;]*\)', 'false', text)
    return text,count+n

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve(); changed=0; rewrites=0
    files=list((root/'src').rglob('*.rs'))
    for srcdir in (root/'crates').glob('*/src'):
        files.extend(srcdir.rglob('*.rs'))
    for p in sorted(set(files)):
        old=p.read_text(encoding='utf-8'); new,n=clean_text(old)
        if new!=old:
            tmp=p.with_name(p.name+'.rmux-strip-web-tmp'); tmp.write_text(new,encoding='utf-8'); os.replace(tmp,p)
            print(f'web-cfg cleanup {p.relative_to(root)} rewrites={n}')
            changed+=1; rewrites+=n
    print(f'web-cfg cleanup changed_files={changed} rewrites={rewrites}'); return 0
if __name__=='__main__': raise SystemExit(main())
