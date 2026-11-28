<div align="center">

<sub><b>English</b> · <a href="README.zh-CN.md">简体中文</a></sub>

# AI Manager

**You have three AI coding clients, and you spend the day switching between their logins.**
This tool puts switching, check-in, sessions and skills into one browser page.

Windows · local backend + browser UI · multi-account login switching · daily check-in & credits · local session library · skills & MCP distribution

[![CI](https://github.com/FaustDream/AI-Manager/actions/workflows/ci.yml/badge.svg)](https://github.com/FaustDream/AI-Manager/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/FaustDream/AI-Manager?style=flat-square)](https://github.com/FaustDream/AI-Manager/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2B-0078D4?style=flat-square)](#install)
[![License](https://img.shields.io/badge/license-custom-lightgrey?style=flat-square)](LICENSE)

</div>

<p align="center">
  <img src="assets/01-accounts.png" width="100%" alt="Account center: accounts across platforms with credential expiry, credit balance and today's check-in status in one view">
</p>

## Why

Qoder, WorkBuddy, Trae and ZCode each keep their own login state, their own credential file, and their own daily check-in. With a few accounts, switching becomes manual labour: open the client, find the account, log in again, and one missed day breaks the streak.

AI Manager folds all of it into one page: it scans every account already logged in on this machine and switches with one click, records check-ins and credits automatically, indexes sessions so you can browse, delete and restore them in one place, and imports skills or MCP servers once to distribute them to multiple clients.

Since 2.0.26 the interface lives in your browser: a local backend serves the page and the API on `127.0.0.1`, and credentials never leave this machine. There is no desktop window, tray icon or autostart.

## Features

| Module | What it does |
| --- | --- |
| Dashboard | Account verdicts, credit flow and token usage on one screen |
| Account center | Scan local accounts, switch login state, daily check-in, credit ledger |
| Sessions | Browse and preview the local session library, delete to trash, restore |
| Skills & MCP | Import, distribute to each client, remove to trash, collect official skills |
| Logs | Action log and runtime log, date-range review, export |
| Settings | Themes, desktop notifications, scheduled jobs, storage and recycle bin |

## Screens

Session library: one index across platforms, filter by source and state — deleted sessions go to trash and can be restored.

<p align="center">
  <img src="assets/02-sessions.png" width="100%" alt="Session library: unified cross-platform browsing with preview, delete and restore">
</p>

Skills & MCP: import once, distribute per platform, and the matrix shows at a glance which platforms are still missing it.

<p align="center">
  <img src="assets/03-hub.png" width="100%" alt="Skills and MCP: skill library with a per-platform deployment matrix">
</p>

## Supported platforms

| Platform | Account switching | Session library |
| --- | --- | --- |
| Qoder (China / International) | Yes | Yes |
| WorkBuddy (China / International) | Yes | Yes |
| ZCode | Yes | Yes |
| Doubao (personal) | No (added by pasting a Cookie) | — |
| Trae | No | — |

Trae stores its login state in a device-bound encrypted format that cannot be written back safely, so switch accounts inside the Trae client itself. Check-in and credits are implemented per provider; the UI only shows the entry points a platform actually supports.

Doubao has no local login state this tool can read (its cookies live in Chromium's encrypted store), so accounts are added by pasting a Cookie under "Add account → Doubao"; one Cookie per account. For Doubao this tool is **read-only on credits**: it does not check in and does not claim quota (the claim endpoint triggers risk control — check in from the Doubao client instead). The platform only returns a cumulative quota figure and no per-batch expiry, so Doubao rows show "not provided" for the nearest expiry.

> The application UI is currently Chinese only. The screenshots above show the real interface.

## Install

Both artifacts live in [Releases](https://github.com/FaustDream/AI-Manager/releases):

| Artifact | Use it when |
| --- | --- |
| `AI-Manager-<ver>-windows-x64-setup.exe` | Normal install. Goes to `%LOCALAPPDATA%\Programs\AI Manager`, no admin rights, uninstall keeps `data\` |
| `AI-Manager-<ver>-windows-x64-portable.zip` | Unzip and run. Move the whole folder to migrate |

Requirements: Windows 10 or later plus a modern browser (Edge, Chrome, …). No .NET, no WebView2, no administrator rights.

> Daily use is handled by the **tray launcher** `ai-manager-launcher.exe`: double-click it (or the Start-menu/desktop shortcut) and it lives in the system tray, starts the local backend and opens the page. The tray icon reflects the service state; the right-click menu starts / stops / restarts the project, and a left click opens a status window with the backend console output and the structured call-chain log. Closing the browser tab does **not** stop the backend — quit via the tray menu "关闭项目" or the power button in the page's top bar.
>
> `start.cmd` remains the development fallback: it incrementally rebuilds the backend before launching. Autostart on login is toggled from the launcher's tray menu.

> **Unsigned build.** On first launch Windows SmartScreen may show "Windows protected your PC" — choose "More info" → "Run anyway". Every release ships a `SHA256SUMS.txt` so you can verify what you downloaded.

## Build from source

Prerequisites: Rust stable, Node.js 22. Building the installer additionally needs NSIS.

```bash
npm ci

# Daily use: build once if needed, start the backend, open the browser
start.cmd
# Or build the tray launcher once, then manage start/stop/restart from the tray
cd launcher && cargo build --release

# Development: one command starts Vite + the backend and opens a browser
powershell -NoProfile -ExecutionPolicy Bypass -File tools\dev-run.ps1

# Manual equivalent
npm run dev                              # terminal 1: Vite dev server (127.0.0.1:5273)
cd src-tauri && cargo run --bin ai-manager  # terminal 2: local backend (127.0.0.1:5274)
# then open http://127.0.0.1:5273 in a browser

# Release build (frontend first: dist/ is hosted by the exe at runtime)
npm run build
cd src-tauri && cargo build --release
```

`src-tauri` has a single binary, `ai-manager`: a local axum service that hosts the built frontend from its own `dist/` folder and exposes `/cmd` plus `/events` on the same port. `launcher/` is a standalone tray-launcher crate (`ai-manager-launcher.exe`) that only manages the backend lifecycle. A release folder therefore needs `ai-manager.exe` + `ai-manager-launcher.exe` + `dist/` + `start.cmd` together — a missing `dist/` means a 404 page.

The backend binds `127.0.0.1` only and holds the single-instance lock, so a second copy is refused by design (two schedulers must not write the same database).

The pre-commit gates match CI:

```bash
npm test && npm run build

cd src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets --features ts -- -D warnings
cargo test --lib --tests --features ts
```

`--features ts` regenerates `src/lib/types/`. If you change an IPC type on the Rust side you must commit the generated files too; CI verifies that the generated output matches what was committed.

## Data and privacy

Everything lives in a `data/` directory next to the executable. Nothing is uploaded anywhere:

```text
data/app.db      accounts, check-ins, credits, session index, skills and MCP (SQLite)
data/logs/       runtime logs
data/backups/    automatic backup taken before writing client configs
data/skills/     skill library
data/trash/      recycle bin for removed items
```

Account credentials are encrypted with Windows DPAPI and bound to the current Windows user. After switching user or machine you must log in again — that is deliberate, not a defect.

## Known limitations

- Windows only: credentials depend on Windows DPAPI
- The account vault does not migrate: copying `data/` over still requires a fresh login
- Upgrading from v1: the v1 data importer is gone; the app neither reads nor deletes the old directory

## Contributing

This is a single-maintainer project. Please use [Issues](https://github.com/FaustDream/AI-Manager/issues/new/choose) and read [CONTRIBUTING.md](CONTRIBUTING.md) first. Do not open a public issue for security problems — see [SECURITY.md](SECURITY.md). Changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## License

Custom terms — read [LICENSE](LICENSE). In short:

- **Allowed**: use it (including at work), modify it for yourself, redistribute it unmodified with attribution intact
- **Not allowed**: remove or alter the attribution, redistribute a rebranded copy, sell it or bundle it into a paid product
- SPDX identifier: `LicenseRef-AI-Manager-1.0`. GitHub reports this as "Other" because it is not an OSI-approved license.
