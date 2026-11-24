# AGENTS.md

本文件是仓库对 AI 编码助手的唯一约定；动手前先读完，与通用编程直觉冲突时以本文为准。

## 1. 技术栈与环境

- Rust + axum 本机 HTTP 服务 + React 19 + TS + Vite 6 + Tailwind 4，浏览器承载，无桌面壳。
- 运行期数据在 exe 同级 `data/`（SQLite + Windows DPAPI 凭据）；debug 构建固定用仓库根 `data/` 与 `dist/`。
- 前端在 `src/`（`capabilities/` 每目录用 `manifest.ts` 自动注册路由与导航），后端在 `src-tauri/`（`app/` 业务域、`desktop/` 壳层、`tests/` 测试）。
- Windows 终端没有 `cargo` 环境变量，执行 cargo 一律写完整路径 `C:\Users\Lynn\.cargo\bin\cargo.exe`。
- 开发自测用 `tools\dev-run.ps1`（Vite 5273 + 后端 5274 + 浏览器）；日常启动用 `start.cmd`。

## 2. 红线

1. 不删除、清空、上传 `data/` 与 `data/backups/` 下任何内容——真实账号库与唯一回退依据。
2. 测试不碰真实 `data/`，一律用 `tempfile` 建独立临时目录。
3. token、密码等凭据不得写进代码、日志、Git 提交或界面。
4. 只改当前任务涉及的调用链与函数邻域；别处的问题只记录并汇报，不顺手重构。

## 3. 架构单一入口（改动前必查）

- 前端命令面唯一入口 `src/lib/cmd.ts`（`runCmd` / `queryCmd` / `useBusy` / `errMsg`）；页面不得直接 fetch `/cmd`，也不得自拼错误话术。
- 确认弹窗一律 `confirmAction()`，禁止 `window.confirm`。
- 日志：后端 `log::action` / `log::item` / `log::fail`，界面动作由 `cmd.ts` 回传 `log_action`；页面不自拼日志、不另建任务状态。
- 定时任务规则与存储只在 `app/schedule/`，调度循环只在 `desktop/background.rs::spawn_scheduler`；业务代码禁止自写 `loop { sleep }`。
- 会话平台差异只扩充 `app/sessions/backend.rs` 的 `SessionBackend` trait；Provider 只在 `app/providers/mod.rs::make()` 构造。
- 错误统一 `AppError::new(ErrorKind, msg)`；配置唯一真源 `data/config.toml`（`app/config.rs`），可调阈值不硬编码。
- SQL 只写在所属 store（`accounts/repo.rs`、`checkin/store.rs`、`sessions/store.rs`、`sessions/marks.rs`、`db.rs`）。
- 表格用 `components/ui/table.tsx` 的 `DataTable` 列定义，不手写裸表格。
- 改 Rust IPC 类型后必须 `cargo test --features ts` 重新生成 `src/lib/types/` 并一起提交。
- 实时事件：后端只从 `app::events::emit` 发，前端只在 `src/lib/events.ts` 收（SSE `/events`）。
- HTTP 壳唯一入口 `bin/ai-manager.rs` + `desktop/web.rs`，dispatch 与 `cmd.ts` 名单成对改（`tests/command_surface.rs` 校验）；只绑 127.0.0.1；单实例锁保留，双开被拒是设计行为。

## 4. 测试与验证

提交前至少跑：

```powershell
npm test          # 前端纯函数单测
npm run build     # tsc --noEmit + vite build
npm run test:rust # 后端单测与契约（同时生成 ts-rs 类型）
```

- 需要时补 `npm run check:rust`（clippy 零警告）与 `npm run fmt:rust`。
- `cargo test` / `clippy` / `npm run build` 必须走 `tools\wb-heavy-run.ps1`（默认 `-Jobs 2`）；内联执行会导致客户端退出，并发只许降不许升。
- `tests/live_probe.rs` 等真机探针带 `#[ignore]`，只在开发机手动跑，不进日常门禁。

## 5. 版本与发布

- 三段式 `主.次.修订`，日常只递增修订段：每次代码改动 +1，自动执行。`9` 结尾进位（如 `2.0.9` → `2.0.10`）须先问用户，未确认则改动并入当前版本。版本只升不降。
- 发布流：改齐版本 → 跑通 §4 → 提交 → `git tag vX.Y.Z` → 推送 main 与 tag，`release.yml` 产出 setup.exe 与 portable.zip 两件产物。
- 构建只走 `tools\build-release.ps1`（前端构建 + release 构建 + zip + 清理）；失败不清理，清理器不碰 `data/`。

## 6. Git 与提交

- 提交署名固定 `凌致 <935411629@qq.com>`（`git config` 已固定，提交前不改）。

## 7. 编码约定

- 注释只写「为什么」与业务取舍，不复述代码字面。
- 页面是任务台：不放页面大标题与说明、不放名词解释图例；解释只留在 tooltip、空状态、错误提示与 README。
- 不铺 TODO 面板、不搞过度设计与多余抽象。
- 修 Bug 先补一条能稳定复现的失败测试，再改实现。

## 8. 平台故障排查（先按顺序查）

1. 网络：系统代理是否启用、目标域名是否可达。国际版（`www.workbuddy.ai`、`openapi.qoder.sh`、`qoder.com`）直连必超时；没有代理就照实说，不改网络代码。
2. 档案（仅 WorkBuddy）：`data/profiles/{cn,intl}/<uid>/` 要同时有 `app/session` 与 `auth.info`；缺件需在客户端登录一次。禁止手工改 `data/`。
3. 凭据：先点「刷新凭据」重扫平台写回最新凭据与有效期，仍过期再重新登录（会清失败标记）。

一键自检：`powershell -NoProfile -ExecutionPolicy Bypass -File tools\intl-doctor.ps1`。能用一次 401/200 证明链路通，就不要改网络代码。
