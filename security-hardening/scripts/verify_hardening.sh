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
FILES="$(mktemp -t rmux-hardening-files.XXXXXX)"
trap 'rm -f "$TMP" "$FILES"' EXIT
: > "$TMP"
: > "$FILES"

# Active Rust/Cargo source inventory only. Security-hardening scripts are intentionally excluded.
find "$ROOT/src" "$ROOT/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -print0 > "$FILES"
printf '%s\0' "$ROOT/Cargo.toml" >> "$FILES"

scan(){
  local pat="$1"
  : > "$TMP"
  if xargs -0 grep -nHE "$pat" < "$FILES" > "$TMP" 2>/dev/null; then
    return 0
  fi
  return 1
}

# Production source may use UnixListener/UnixStream, but not Internet sockets,
# browser/WebShare transports, tunneling helpers, Claude integration, or Windows code.
PATTERNS=(
  'TcpListener' 'TcpStream' 'UdpSocket' 'SocketAddr' 'AF_INET' 'AF_INET6' 'SOCK_DGRAM' 'IPPROTO_TCP' 'IPPROTO_UDP'
  'tokio::net::Tcp' 'tokio::net::Udp' 'std::net::Tcp' 'std::net::Udp'
  'WebSocket' 'websocket' 'tungstenite' 'axum' 'hyper::' 'reqwest' 'httparse'
  'share\.rmux\.io' 'localhost\.run' 'serveo' 'tailscale' 'cloudflared' 'ngrok' 'funnel'
  'WebShare' 'web_share' 'web-share' 'CAPABILITY_WEB_SHARE' 'rmux-web-crypto'
  'Claude' 'claude' '[Pp]ower[Ss]hell' '[Cc]on[Pp][Tt][Yy]'
  'cfg\(windows\)' 'target_os[[:space:]]*=[[:space:]]*"windows"' 'windows_sys' 'windows-sys'
  'AttachedWindows' 'WindowsConsole' 'WINDOWS_CONSOLE' 'CAPABILITY_[A-Z0-9_]*WINDOWS'
)
for pat in "${PATTERNS[@]}"; do
  if scan "$pat"; then
    fail "forbidden runtime/platform pattern matched: $pat"
    cat "$TMP" >&2
  fi
done

# Helper execution must not automatically launch known networking tools.
: > "$TMP"
if find "$ROOT/src" "$ROOT/crates" -type f -name '*.rs' -print0 | \
   xargs -0 grep -nHE 'Command::new\([^)]*(ssh|tailscale|cloudflared|ngrok|curl|wget)' > "$TMP" 2>/dev/null; then
  fail 'network helper launcher remains'
  cat "$TMP" >&2
else
  pass 'no automatic network helper launcher found'
fi

# Unix-domain IPC must remain as the only transport boundary.
if [[ -d "$ROOT/crates/rmux-ipc" ]]; then
  if find "$ROOT/crates/rmux-ipc" -type f -name '*.rs' -print0 | \
     xargs -0 grep -qE 'UnixListener|UnixStream'; then
    pass 'AF_UNIX IPC implementation present'
  else
    fail 'rmux-ipc remains but no UnixListener/UnixStream implementation found'
  fi
fi

# No platform/remote feature-specific tracked paths in the active tree.
: > "$TMP"
while IFS= read -r -d '' f; do
  case "$f" in security-hardening/*|STRUCTURAL-REDUCTION.json|README.md) continue;; esac
  lower="$(printf '%s' "$f" | tr '[:upper:]' '[:lower:]')"
  case "$lower" in *web-share*|*web_share*|*tunnel*|*claude*|*conpty*|*powershell*|*windows*) printf '%s\n' "$f" >> "$TMP";; esac
done < <(git -C "$ROOT" ls-files -z)
if [[ -s "$TMP" ]]; then
  fail 'feature/platform-specific paths remain'
  cat "$TMP" >&2
else
  pass 'forbidden feature-specific tracked paths absent'
fi

# Root/workspace manifests must not re-enable the deleted surfaces.
for manifest in "$ROOT/Cargo.toml" "$ROOT"/crates/*/Cargo.toml; do
  [[ -f "$manifest" ]] || continue
  if grep -nE 'rmux-web-crypto|windows-sys|^[[:space:]]*web[[:space:]]*=' "$manifest" > "$TMP" 2>/dev/null; then
    fail "forbidden manifest feature/dependency remains: $manifest"
    cat "$TMP" >&2
  fi
done

if [[ "$FAIL" -ne 0 ]]; then
  echo 'FINAL_STATUS=FAIL' >&2
  exit 1
fi
echo 'FINAL_STATUS=PASS'
