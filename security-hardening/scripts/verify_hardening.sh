#!/usr/bin/env bash
set -euo pipefail
ROOT="${1:-.}"
ROOT="$(cd "$ROOT" && pwd)"
FAIL=0
fail(){ printf 'FAIL: %s\n' "$*" >&2; FAIL=1; }
pass(){ printf 'PASS: %s\n' "$*"; }

for p in crates/rmux-web-crypto crates/rmux-server/src/web crates/rmux-server/tunnels web-frontend resources/windows resources/claude docs/web-share.md; do
  [[ -e "$ROOT/$p" ]] && fail "forbidden path remains: $p" || pass "absent: $p"
done

TMP="$(mktemp -t rmux-hardening.XXXXXX)"
trap 'rm -f "$TMP"' EXIT
: > "$TMP"

# Production source may use UnixListener/UnixStream, but not Internet sockets or web/tunnel transports.
PATTERNS=(
  'TcpListener' 'TcpStream' 'UdpSocket' 'SocketAddr' 'AF_INET' 'AF_INET6' 'SOCK_DGRAM' 'IPPROTO_TCP' 'IPPROTO_UDP'
  'tokio::net::Tcp' 'tokio::net::Udp' 'std::net::Tcp' 'std::net::Udp'
  'WebSocket' 'websocket' 'tungstenite' 'axum' 'hyper::' 'reqwest' 'httparse'
  'share\.rmux\.io' 'localhost\.run' 'serveo' 'tailscale' 'cloudflared' 'ngrok' 'funnel'
  'WebShare' 'web_share' 'CAPABILITY_WEB_SHARE' 'rmux-web-crypto'
  'Claude' 'claude' 'powershell' 'conpty' 'windows-sys'
)
for pat in "${PATTERNS[@]}"; do
  if rg -n --hidden --glob '!target/**' --glob '!.git/**' --glob '!security-hardening/**' --glob '*.rs' --glob 'Cargo.toml' -e "$pat" "$ROOT/src" "$ROOT/crates" "$ROOT/Cargo.toml" >"$TMP" 2>/dev/null; then
    fail "forbidden runtime pattern matched: $pat"
    cat "$TMP" >&2
  fi
done

# Helper process execution must not automatically launch known networking tools.
if rg -n --hidden --glob '!target/**' --glob '!.git/**' --glob '!security-hardening/**' --glob '*.rs' \
  -e 'Command::new\([^\n]*(ssh|tailscale|cloudflared|ngrok|curl|wget)' "$ROOT/src" "$ROOT/crates" >"$TMP" 2>/dev/null; then
  fail 'network helper launcher remains'; cat "$TMP" >&2
else
  pass 'no automatic network helper launcher found'
fi

# Unix-domain IPC must exist if IPC crate is present.
if [[ -d "$ROOT/crates/rmux-ipc" ]]; then
  rg -n --glob '*.rs' -e 'UnixListener|UnixStream' "$ROOT/crates/rmux-ipc" >/dev/null 2>&1 \
    && pass 'AF_UNIX IPC implementation present' \
    || fail 'rmux-ipc remains but no UnixListener/UnixStream implementation found'
fi

# No platform-specific source files for Windows/Claude/tunnel/web-share in the active tree.
git -C "$ROOT" ls-files -z | while IFS= read -r -d '' f; do
  case "$f" in security-hardening/*|STRUCTURAL-REDUCTION.json|README.md) continue;; esac
  case "${f,,}" in *web-share*|*web_share*|*tunnel*|*claude*|*conpty*|*powershell*|*windows*) printf '%s\n' "$f";; esac
done >"$TMP"
if [[ -s "$TMP" ]]; then fail 'feature/platform-specific paths remain'; cat "$TMP" >&2; else pass 'forbidden feature-specific tracked paths absent'; fi

if [[ "$FAIL" -ne 0 ]]; then echo 'FINAL_STATUS=FAIL' >&2; exit 1; fi
echo 'FINAL_STATUS=PASS'
