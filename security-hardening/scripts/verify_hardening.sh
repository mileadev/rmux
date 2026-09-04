#!/usr/bin/env bash
set -euo pipefail
ROOT="${1:-.}"
ROOT="$(cd "$ROOT" && pwd)"
FAIL=0
fail(){ printf 'FAIL: %s\n' "$*" >&2; FAIL=1; }
pass(){ printf 'PASS: %s\n' "$*"; }

TMP="$(mktemp -t rmux-hardening.XXXXXX)"
FILES="$(mktemp -t rmux-hardening-files.XXXXXX)"
trap 'rm -f "$TMP" "$FILES"' EXIT

# Entire feature/platform trees that are forbidden in this macOS local-only fork.
FORBIDDEN_PATHS=(
  crates/rmux-web-crypto
  crates/rmux-server/src/web
  crates/rmux-server/tunnels
  crates/rmux-sdk/src/web_share
  src/cli/web_share_display
  web-frontend
  resources/windows
  resources/claude
  crates/rmux-client/src/attach_windows
  crates/rmux-pty/src/backend/windows
  crates/rmux-pty/tests/windows_conpty
  crates/rmux-ipc/tests/named_pipe_integration.rs
  docs/web-share.md
)
for p in "${FORBIDDEN_PATHS[@]}"; do
  [[ -e "$ROOT/$p" ]] && fail "forbidden path remains: $p" || pass "absent: $p"
done

# Production Rust only: exclude tests/benches/examples so fixtures cannot mask runtime policy.
: > "$FILES"
while IFS= read -r -d '' f; do
  case "$f" in
    */tests/*|*/benches/*|*/examples/*|*_test.rs|*_tests.rs) continue ;;
  esac
  printf '%s\0' "$f" >> "$FILES"
done < <(find "$ROOT/src" "$ROOT/crates" -type f -name '*.rs' -print0)

scan_prod(){
  local pat="$1"
  : > "$TMP"
  if xargs -0 grep -nHE "$pat" < "$FILES" > "$TMP" 2>/dev/null; then
    return 0
  fi
  return 1
}

# Executable macOS source must not contain Internet transports or removed remote features.
RUNTIME_PATTERNS=(
  'TcpListener' 'TcpStream' 'UdpSocket' 'AF_INET' 'AF_INET6' 'SOCK_DGRAM' 'IPPROTO_TCP' 'IPPROTO_UDP'
  'tokio::net::Tcp' 'tokio::net::Udp' 'std::net::Tcp' 'std::net::Udp'
  'WebSocket' 'websocket' 'tungstenite' 'axum' 'hyper::' 'reqwest' 'httparse'
  'share\.rmux\.io' 'localhost\.run' 'serveo' 'cloudflared' 'ngrok' 'tailscale[[:space:]]+(funnel|serve)'
  'WebShare' 'web_share' 'web-share' 'CAPABILITY_WEB_SHARE' 'rmux-web-crypto'
  'claude_launcher' 'claude_skill' 'dangerously-skip-permissions'
)
for pat in "${RUNTIME_PATTERNS[@]}"; do
  if scan_prod "$pat"; then
    fail "forbidden macOS runtime pattern matched: $pat"
    cat "$TMP" >&2
  fi
done

# No automatic launch of known networking/download helpers.
if scan_prod 'Command::new\([^)]*(ssh|tailscale|cloudflared|ngrok|curl|wget)'; then
  fail 'network/download helper launcher remains'
  cat "$TMP" >&2
else
  pass 'no automatic network/download helper launcher found'
fi

# Same-user AF_UNIX IPC is the intended transport boundary.
if find "$ROOT/crates/rmux-ipc" -type f -name '*.rs' -print0 | xargs -0 grep -qE 'UnixListener|UnixStream'; then
  pass 'AF_UNIX IPC implementation present'
else
  fail 'AF_UNIX IPC implementation not found'
fi

# No dedicated remote/Windows implementation paths should remain tracked.
: > "$TMP"
while IFS= read -r -d '' f; do
  case "$f" in security-hardening/*|STRUCTURAL-REDUCTION.json) continue;; esac
  lower="$(printf '%s' "$f" | tr '[:upper:]' '[:lower:]')"
  case "$lower" in
    *web-share*|*web_share*|*tunnel*|*claude*|*conpty*|*powershell*|*/windows/*|*windows_*.rs|*_windows.rs)
      printf '%s\n' "$f" >> "$TMP" ;;
  esac
done < <(git -C "$ROOT" ls-files -z)
if [[ -s "$TMP" ]]; then
  fail 'dedicated remote/platform implementation paths remain'
  cat "$TMP" >&2
else
  pass 'dedicated remote/platform implementation paths absent'
fi

# Cross-platform files may still contain dormant cfg(windows) compatibility branches,
# but Windows crates/targets and removed remote feature dependencies are prohibited.
for manifest in "$ROOT/Cargo.toml" "$ROOT"/crates/*/Cargo.toml; do
  [[ -f "$manifest" ]] || continue
  if grep -nE "rmux-web-crypto|windows-sys|\[target\.'cfg\(windows\)'\.dependencies\]|^[[:space:]]*web[[:space:]]*=" "$manifest" > "$TMP" 2>/dev/null; then
    fail "forbidden manifest feature/dependency remains: $manifest"
    cat "$TMP" >&2
  fi
done

# The public CLI must not advertise deleted functionality.
if [[ -x "$ROOT/target/release/rmux" ]]; then
  "$ROOT/target/release/rmux" --help > "$TMP" 2>&1 || true
  if grep -Ei 'web-share|share\.rmux\.io|tunnel|claude' "$TMP" >/dev/null; then
    fail 'release CLI still advertises removed remote/Claude functionality'
    cat "$TMP" >&2
  else
    pass 'release CLI does not advertise removed remote/Claude functionality'
  fi
fi

if [[ "$FAIL" -ne 0 ]]; then
  echo 'FINAL_STATUS=FAIL' >&2
  exit 1
fi
echo 'FINAL_STATUS=PASS'
