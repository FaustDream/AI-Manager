<sub><b>English</b> · <a href="CONTRIBUTING.zh-CN.md">简体中文</a></sub>

# Contributing

Single maintainer, limited time. Every rule here serves one goal: **make the problem reproducible in one pass**. A clear report usually gets fixed within one release; a vague one costs three rounds of guessing.

## Before you file

Check these three in order — most problems end at step two:

1. **Is your version current?** The version number is at the bottom of the Settings page. Check [Releases](https://github.com/FaustDream/AI-Manager/releases) for something newer, then search [CHANGELOG.md](CHANGELOG.md) for your symptom.
2. **Is it a `data/` problem?** All state lives in `data/` next to the executable. If `data/` was moved, renamed or deleted — or the exe was moved to another folder — the symptom is always "all my accounts are gone". Confirm `data/app.db` is still in place before filing.
3. **Do the logs already answer it?** Logs page → runtime log, review by date. Every failure carries a `kind` and the original message, which says far more than the one line shown in the UI.

## Reporting a bug

Use the [bug report](https://github.com/FaustDream/AI-Manager/issues/new?template=bug.yml) form. All four fields are required:

| Field | Where to get it |
| --- | --- |
| Version | Bottom of the Settings page, or the exe filename |
| Steps to reproduce | From launch to failure, one step per line |
| Expected vs actual | One sentence each |
| Logs | Logs page → export, or the dated file under `data/logs/` |

Scan the log for account names, tokens and phone numbers before pasting — delete those lines first.

## Screenshots

Screenshots go into `assets/` and must be sanitised. This is the one place in the repo where information leaks easily:

- Use **demo accounts** (`demo-01` style names), never real account nicknames
- Mask emails, phone numbers, avatars, balance figures, token prefixes and session titles
- Window at 1180×760 (the default size), cropped to content height
- Three shots: account center → `01-accounts.png`, sessions → `02-sessions.png`, skills & MCP → `03-hub.png`
- Save as PNG at 2× resolution; do not rename the three `img` references in the READMEs

## Local development

Environment and commands are in the README's "Build from source". Two additions:

- After changing an IPC type on the Rust side, run `cargo test --features ts` and commit `src/lib/types/`; CI enforces this
- Tests must never touch the real `data/`: every test builds its own directory with `tempfile`

## Commits

Commit messages are written in Chinese, with one of four prefixes, matching the existing history:

```text
功能: 新增 xxx
修复: xxx 判断错了
测试: 补 xxx 用例
发布: v2.0.9 —— 一句话主题
```

One commit, one concern. Unrelated refactors across modules belong in their own commit or PR — the worst review is a bug fix buried under ten unrelated edits.

## Pull requests

Welcome, but to be explicit: **no promise on merge latency, and no promise of merging at all**. The project has hard architectural constraints (single entry points, strongly typed errors, generated types committed). A change that violates them gets rejected even if it works. Open an issue first to align on direction — it saves you writing code that gets thrown away.
