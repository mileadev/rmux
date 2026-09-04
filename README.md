# RMUX macOS local-only

Security-reduced macOS-only fork of Helvesec/rmux v0.10.0.

- Local PTYs and local terminal multiplexing only.
- Same-user Unix-domain IPC only.
- No Web Share, browser terminal, TCP/UDP listener, HTTP/WebSocket service, tunnel provider, SSH reverse forwarding, Tailscale Funnel/Serve, telemetry, or Claude integration.
- Remote/sharing functionality is intentionally unsupported and removed from the active source tree.

See `security-hardening/` for the security gates and validation contract.
