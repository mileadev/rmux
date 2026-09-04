#!/usr/bin/env python3
"""Remove top-level CLI/startup WebShare fossils after protocol/server removal."""
from __future__ import annotations
import os,re,sys
from pathlib import Path

def atomic(p: Path,text: str) -> None:
    tmp=p.with_name(p.name+'.rmux-stage7-tmp'); tmp.write_text(text,encoding='utf-8'); os.replace(tmp,p)
def edit(p: Path,fn) -> None:
    if not p.exists(): return
    old=p.read_text(encoding='utf-8'); new=fn(old)
    if new!=old: atomic(p,new); print(f'edited {p}')
def drop(p: Path) -> None:
    if p.exists(): p.unlink(); print(f'deleted {p}')

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    drop(root/'src/cli/terminal_theme.rs')

    def dispatch(t: str) -> str:
        t=t.replace('use super::web_commands::run_web_share;\n','')
        t=t.replace('        | Command::WebShare(_)\n','')
        t=t.replace('        Command::WebShare(args) => run_web_share(args, socket_path, command_startup),\n','')
        t=t.replace('        Command::WebShare(args) => web_share_creates_share(args),\n','')
        t=re.sub(r'\nfn web_share_creates_share\([\s\S]*?\n\}\n\nfn command_requires_web_daemon\([\s\S]*?\n\}\n','\n',t)
        t=t.replace('    let command_startup = startup.for_command(\n        command_has_start_server_flag(&command),\n        command_requires_web_daemon(&command),\n        start_server_args,\n    );','    let command_startup = startup.for_command(command_has_start_server_flag(&command));')
        t=t.replace('    let start_server_args = match &command {\n        Command::StartServer(args) => Some(args),\n        _ => None,\n    };\n','')
        return t
    edit(root/'src/cli/dispatch.rs',dispatch)

    def startup(t: str) -> str:
        t=t.replace('use crate::cli_args::{Cli, Command, ConfigFileSelection, StartServerArgs, TopLevelCommandScan};','use crate::cli_args::{Cli, Command, ConfigFileSelection, TopLevelCommandScan};')
        t=re.sub(r'    pub\(in crate::cli\) fn for_command\(\n        &self,\n        command_has_start_server_flag: bool,\n        command_requires_web: bool,\n        start_server_args: Option<&StartServerArgs>,\n    \) -> Self \{[\s\S]*?\n    \}\n','''    pub(in crate::cli) fn for_command(&self, command_has_start_server_flag: bool) -> Self {
        Self {
            no_start_server: self.no_start_server || !command_has_start_server_flag,
            config: self.config.clone(),
            endpoint: self.endpoint.clone(),
        }
    }
''',t)
        t=t.replace('    pub(super) web_frontend: Option<String>,\n    pub(super) web_port: Option<u16>,\n','')
        t=t.replace('    let web = start_server_web_args(command);\n','    let _ = command;\n')
        t=t.replace('                web_frontend: web.web_frontend.clone(),\n                web_port: web.web_port,\n','')
        t=t.replace('    config.auto_start = apply_web_auto_start_config(config.auto_start, &web);\n','')
        t=re.sub(r'\nfn start_server_web_args\([\s\S]*?\n\}\n\nfn apply_web_auto_start_config\([\s\S]*?\n\}\n','\n',t)
        t=re.sub(r'\nfn apply_web_daemon_config\([\s\S]*?\n\}\n','\n',t)
        old='''    let config = apply_web_daemon_config(
        apply_server_startup_config(
            DaemonConfig::new(socket_path.to_path_buf()),
            &startup_config.server,
        ),
        startup_config,
    );'''
        new='''    let config = apply_server_startup_config(
        DaemonConfig::new(socket_path.to_path_buf()),
        &startup_config.server,
    );'''
        t=t.replace(old,new)
        return t
    edit(root/'src/cli/startup.rs',startup)

    edit(root/'src/cli_response.rs',lambda t: t.replace('        Response::WebShare(_) if command_name == "web-share" => Ok(()),\n','').replace('        Response::WebShare(_) => "web-share",\n',''))

    def main_rs(t: str) -> str:
        t=t.replace('    web_frontend: Option<String>,\n    web_port: Option<u16>,\n','')
        t=t.replace('    let mut web_frontend = None;\n    let mut web_port = None;\n','')
        t=t.replace('                &mut web_frontend,\n                &mut web_port,\n','')
        t=t.replace('            &mut web_frontend,\n            &mut web_port,\n','')
        t=t.replace('        web_frontend,\n        web_port,\n','')
        t=t.replace('    web_frontend: &mut Option<String>,\n    web_port: &mut Option<u16>,\n','')
        t=re.sub(r'\n        Some\("--web-port"\) => \{[\s\S]*?\n        \}\n        Some\("--frontend-url" \| "--web-frontend"\) => \{[\s\S]*?\n        \}\n','\n',t)
        t=t.replace('    reject_unsupported_web_args(&args)?;\n\n','')
        t=re.sub(r'\n    if let Some\(port\) = args\.web_port \{\n        config = config\.with_web_port\(port\);\n    \}\n    if let Some\(frontend\) = args\.web_frontend \{\n        config = config\.with_web_frontend\(frontend\);\n    \}\n','\n',t)
        t=re.sub(r'\n#\[cfg\(any\(not\(feature = "tiny-cli"\), debug_assertions\)\)\]\nfn reject_unsupported_web_args\([\s\S]*?\n\}\n','\n',t)
        return t
    edit(root/'src/main.rs',main_rs)

    def daemon_main(t: str) -> str:
        t=t.replace('    web_frontend: Option<String>,\n    web_port: Option<u16>,\n','')
        t=t.replace('    let mut web_frontend = None;\n    let mut web_port = None;\n','')
        t=t.replace('                &mut web_frontend,\n                &mut web_port,\n','')
        t=t.replace('            &mut web_frontend,\n            &mut web_port,\n','')
        t=t.replace('        web_frontend,\n        web_port,\n','')
        t=t.replace('    web_frontend: &mut Option<String>,\n    web_port: &mut Option<u16>,\n','')
        t=re.sub(r'\n        Some\("--web-port"\) => \{[\s\S]*?\n        \}\n        Some\("--frontend-url" \| "--web-frontend"\) => \{[\s\S]*?\n        \}\n','\n',t)
        t=t.replace('    reject_unsupported_web_args(&args)?;\n\n','')
        t=re.sub(r'\n    if let Some\(port\) = args\.web_port \{\n        config = config\.with_web_port\(port\);\n    \}\n    if let Some\(frontend\) = args\.web_frontend \{\n        config = config\.with_web_frontend\(frontend\);\n    \}\n','\n',t)
        t=re.sub(r'\nfn reject_unsupported_web_args\([\s\S]*?\n\}\n','\n',t)
        return t
    edit(root/'src/daemon_main.rs',daemon_main)

    print('stage7 CLI/startup cleanup complete'); return 0
if __name__=='__main__': raise SystemExit(main())
