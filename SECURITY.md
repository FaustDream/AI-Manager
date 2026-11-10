<sub><b>English</b> · <a href="SECURITY.zh-CN.md">简体中文</a></sub>

# Security policy

## Supported versions

Security fixes go to the latest release only. Please upgrade before reporting.

| Version | Supported |
| --- | --- |
| Latest release | Yes |
| Older releases | No |

## Reporting a vulnerability

**Do not open a public issue.**

Use GitHub's private advisory flow: <https://github.com/FaustDream/AI-Manager/security/advisories/new>

Please include the affected version, steps to reproduce, and the impact (what can be read, what can be written).

## Priority areas

This application reads and writes credentials, so the following are highest priority:

- **Credential exposure** — ciphertext under `data/` decryptable by a different Windows user; plaintext tokens appearing in logs, exports or the UI
- **Unauthorised writes** — rewriting client config files without confirmation (`data/backups/` exists so that this stays reversible)
- **Local port exposure** — the web debug shell (`ai-manager-web`) is supposed to bind `127.0.0.1` only and validate the `x-ai-manager-web` header plus an Origin allowlist; bypassing either is a vulnerability
- **Path traversal** — writing outside `data/` through a crafted path when importing a skill or MCP server

## Out of scope

- Issues that require local administrator rights or physical access
- Vulnerabilities in the third-party clients themselves (Qoder / WorkBuddy / Trae / ZCode) — report those to the respective vendor
- Exposure caused by sending `data/` and its ciphertext to someone else (DPAPI binds it to the current Windows user; it cannot be decrypted elsewhere by design)

## Response

Single maintainer; best effort first reply within 7 days. Confirmed issues get a fix first, disclosure afterwards.
