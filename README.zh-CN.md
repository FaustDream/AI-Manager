<div align="center">

<sub><a href="README.md">English</a> · <b>简体中文</b></sub>

# AI Manager

**你有三个 AI 编程客户端，每天在它们的登录态之间来回切。**
这个工具把切换、签到、会话、技能收进一个浏览器页面。

Windows · 本机后端 + 浏览器界面 · 多账号登录态切换 · 每日签到与积分 · 本地会话库 · 技能与 MCP 分发

[![CI](https://github.com/FaustDream/AI-Manager/actions/workflows/ci.yml/badge.svg)](https://github.com/FaustDream/AI-Manager/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/FaustDream/AI-Manager?style=flat-square)](https://github.com/FaustDream/AI-Manager/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2B-0078D4?style=flat-square)](#安装)
[![License](https://img.shields.io/badge/license-custom-lightgrey?style=flat-square)](LICENSE)

</div>

<p align="center">
  <img src="assets/01-accounts.png" width="100%" alt="账号中心：多平台账号一览，凭据到期、积分余额与今日签到状态同屏">
</p>

## 为什么需要它

Qoder、WorkBuddy、Trae、ZCode 各有一套登录态，各存一份凭据，各自要签到。账号一多，切换就成了体力活：翻客户端、找账号、重新登录、忘签一天断掉连续记录。

AI Manager 把这些收进一个页面：扫出本机所有已登录账号，一键切过去；签到与积分自动记账；会话库统一浏览、删除与恢复；技能和 MCP 一次导入、分发到多个客户端。

2.0.26 起界面在浏览器里：本机后端在 `127.0.0.1` 上同时托管页面与接口，凭据不出本机。没有桌面窗口、托盘图标和开机自启。

## 功能

| 模块 | 做什么 |
| --- | --- |
| 仪表盘 | 账号结论、积分收支、Token 用量同屏看完 |
| 账号中心 | 扫描本机账号、一键切换登录态、每日签到、积分台账 |
| 会话 | 本地会话库浏览与预览、删除进回收站、可恢复 |
| 技能 MCP | 技能与 MCP 导入、分发到各客户端、移除进回收站、官方技能收录 |
| 日志 | 执行动作与运行日志双视图、按日期回看、导出 |
| 设置 | 主题、桌面通知、定时任务、空间与回收站 |

## 界面

会话库：跨平台会话统一索引，按来源与状态筛选，删除进回收站还能捞回来。

<p align="center">
  <img src="assets/02-sessions.png" width="100%" alt="会话库：跨平台会话统一浏览，可预览、删除与恢复">
</p>

技能 MCP：导入一次，按平台分发，矩阵里一眼看出哪些平台还缺。

<p align="center">
  <img src="assets/03-hub.png" width="100%" alt="技能 MCP：技能库与各平台部署状态矩阵">
</p>

## 支持平台

| 平台 | 账号切换 | 会话库 |
| --- | --- | --- |
| Qoder（国内版 / 国际版） | 支持 | 支持 |
| WorkBuddy（国内版 / 国际版） | 支持 | 支持 |
| ZCode | 支持 | 支持 |
| 豆包（个人版） | 不支持（粘贴 Cookie 添加） | — |
| Trae | 不支持 | — |

Trae 的登录态是设备绑定的加密格式，无法安全写回，请在 Trae 客户端内手动切换。签到与积分按各家接口分别实现，界面上只显示该平台实际支持的入口。

豆包没有可回写的本地登录态（Cookie 在 Chromium 加密库里），用「添加账号 → 豆包 → 粘贴 Cookie」添加；多开就是多贴一份 Cookie。本工具对豆包**只做只读的积分查询**：不签到、不领取（领取接口会被风控，签到请在豆包客户端里点）。平台只回累计体验额度，不下发批次到期日，所以豆包账号的「最近到期」显示「平台未提供」。

## 安装

两个产物都在 [Releases](https://github.com/FaustDream/AI-Manager/releases)：

| 产物 | 适合 |
| --- | --- |
| `AI-Manager-<ver>-windows-x64-setup.exe` | 常规安装。装到 `%LOCALAPPDATA%\Programs\AI Manager`，免管理员，卸载保留 `data\` |
| `AI-Manager-<ver>-windows-x64-portable.zip` | 解压即用。换机器把整个文件夹拷走 |

系统要求：Windows 10 及以上 + 一个现代浏览器（Edge / Chrome 等）。不需要 .NET，不需要 WebView2，不需要管理员权限。

> 日常使用交给**托盘启动器** `ai-manager-launcher.exe`：双击（或由开始菜单/桌面快捷方式）后常驻系统托盘，自动拉起本机后端并打开页面；托盘图标反映运行状态，右键菜单可启动 / 关闭 / 重启项目，左键点开状态窗口查看后端输出与调用链日志。关掉浏览器标签页**不会**停后端——要退出服务用托盘菜单「关闭项目」，或点页面右上角的电源按钮。
>
> `start.cmd` 保留为开发兜底：它会增量重建后端再拉起。设置里的「开机自启」由启动器的托盘菜单管理。

> **未签名构建。** 首次启动时 Windows SmartScreen 可能提示「Windows 已保护你的电脑」，点「更多信息」→「仍要运行」即可。每个 Release 都附带 `SHA256SUMS.txt`，可自行校验下载内容。

## 从源码构建

前置：Rust stable、Node.js 22。打安装包另需 NSIS。

```bash
npm ci

# 日常使用：缺产物先构建，然后拉后端 + 开浏览器
start.cmd
# 或者构建一次托盘启动器，之后用它管理服务的启动 / 关闭 / 重启
cd launcher && cargo build --release

# 开发：一条命令，自动启动 Vite + 后端，并用浏览器打开
powershell -NoProfile -ExecutionPolicy Bypass -File tools\dev-run.ps1

# 手动等价：开两个终端
npm run dev                                 # 终端 1：Vite 开发服务器（127.0.0.1:5273）
cd src-tauri && cargo run --bin ai-manager   # 终端 2：本机后端（127.0.0.1:5274）
# 然后浏览器打开 http://127.0.0.1:5273

# 发布构建（先前端：dist/ 由 exe 运行时托管）
npm run build
cd src-tauri && cargo build --release
```

`src-tauri` 只有一个二进制 `ai-manager`：本机 axum 服务，从自身同级 `dist/` 托管前端产物，并在同一端口提供 `/cmd` 与 `/events`。`launcher/` 是独立的托盘启动器 crate（`ai-manager-launcher.exe`），只管后端生命周期，不承载业务。发布目录里 `ai-manager.exe` + `ai-manager-launcher.exe` + `dist/` + `start.cmd` 必须齐备——漏带 `dist/` 页面直接 404。

后端只绑 `127.0.0.1`，并持有单实例锁：第二份实例会被拒绝，这是设计行为（两个调度器不能同时写同一个库）。

提交前的门禁与 CI 一致：

```bash
npm test && npm run build

cd src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets --features ts -- -D warnings
cargo test --lib --tests --features ts
```

`--features ts` 会重新生成 `src/lib/types/`。改了 Rust 端 IPC 类型就必须把生成物一起提交，CI 会校验「生成物 = 提交物」。

## 数据与隐私

全部数据都在 exe 同级的 `data/` 目录，不上传任何地方：

```text
data/app.db      账号、签到、积分、会话索引、技能与 MCP（SQLite）
data/logs/       运行日志
data/backups/    写客户端配置前的自动备份
data/skills/     技能库
data/trash/      移除项的回收站
```

账号凭据用 Windows DPAPI 加密，与当前 Windows 用户绑定。换用户或换机器后需要重新登录——这是有意为之，不是缺陷。

## 已知限制

- 仅 Windows：凭据依赖 Windows DPAPI
- 账号库不跨机器迁移：拷走 `data/` 之后仍需重新登录
- 从 v1 升级：v1 数据导入器已下线，程序不会读取或删除旧版目录

## 参与

单人维护的项目。问题请走 [Issue](https://github.com/FaustDream/AI-Manager/issues/new/choose)，提之前先看 [CONTRIBUTING.zh-CN.md](CONTRIBUTING.zh-CN.md)。安全问题不要开公开 Issue，见 [SECURITY.zh-CN.md](SECURITY.zh-CN.md)。改动记录在 [CHANGELOG.md](CHANGELOG.md)。

## 许可

自定义条款，见 [LICENSE](LICENSE)。三句话说完：

- **可以**：自用（含公司内部使用）、自行修改、原样分发并保留署名
- **不可以**：去掉或篡改署名、改名后分发、打包出售或捆绑进付费产品
- SPDX 标识：`LicenseRef-AI-Manager-1.0`。GitHub 显示为 `Other`，因为它不是 OSI 认可的开源协议
