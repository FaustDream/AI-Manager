# AI Manager

Windows 桌面端的 AI 编程客户端统一工作台：账号与登录态切换、每日签到与积分、本地会话库管理、技能与 MCP 分发、日志与维护。

## 功能

- **账号中心**：扫描 Qoder / WorkBuddy / ZCode / Trae / CodeBuddy 的本地登录态，加密入库、一键切换、恢复会话。
- **签到与积分**：按平台规则每日签到、积分与额度刷新、批量执行、积分台账。
- **会话管理**：本地索引、筛选分页、归档与独立归档列表（按软件/项目/账号分组）、彻底删除、正文预览。
- **技能与 MCP**：统一技能库，按平台部署/移除；MCP 以标准格式导入并分发到 8 个目标。
- **日志**：一份 `app.jsonl` 支撑「全部日志 / 执行动作」双视图，支持级别、模块、结果、触发方、主体、时间与关键词筛选及导出。
- **设置与维护**：主题、开机自启、运行配置（并发/超时/采集间隔等）、网络自检、定时任务、数据目录占用、配置历史、平台技能回收站。

## 技术栈

- 后端：Rust + Tauri 2，SQLite（rusqlite），Windows DPAPI，reqwest
- 前端：React 19 + TypeScript + Vite 6 + Tailwind CSS 4 + TanStack React Query
- 类型同步：`ts-rs` 生成 `src/lib/types/`

## 环境

- Windows 10 1803+，系统自带 WebView2
- Node.js 22+
- Rust stable，`cargo` 需在 PATH 中

## 开发

```powershell
npm install
npm run dev                 # 仅前端（Vite）
```

完整构建（必须带 `custom-protocol`，否则 release exe 会连本地 devUrl 导致白屏）：

```powershell
npm run build
C:\Users\Lynn\.cargo\bin\cargo.exe build --release --features custom-protocol
```

开发版启动脚本 `启动开发版.vbs` 会把 `AIMANAGER_DATA_DIR` 指向项目根 `data/`。


## 数据

所有运行数据在 exe 同级 `data/`：

- `app.db`：账号、签到、积分、会话索引、技能与 MCP
- `config.toml`：应用配置（签到并发/间隔、请求超时、用量采集间隔等）。改完重启生效；
  也可以在「设置 → 配置」里调整（立即生效并写回本文件）
- `logs/`：`app.log`、`app.jsonl`、`crash.log`
- `backups/`：写客户端配置前的自动备份
- `skills/`：技能库
- `trash/`：平台技能移除时的回收站（会话删除不再进回收站）

凭据使用 Windows DPAPI 加密，与当前 Windows 用户绑定。整个文件夹拷走即可迁移；`data/` 与 `data/backups/` 不要手工删除或上传。

## 已知问题与想法

单人维护的轻量 backlog：一句话一条，做完就删。

- 国际版链路依赖系统代理节点，节点抖动时「网络错误」是预期表现（设置里可查自检）
- 更新目前是手动下载安装包；若升级变频繁再考虑 Tauri updater 自动更新
- 会话页大列表在几千条时的滚动性能尚未压测

## 许可证

见 [LICENSE](LICENSE)。
