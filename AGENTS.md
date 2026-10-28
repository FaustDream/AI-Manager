# AI Manager · 项目规范

本文件是本仓库对 AI 编码助手的唯一约定来源。在此仓库内工作前，先读完全文。

## 1. 项目定位与技术栈

Windows 桌面工具，把「三家 AI 编程平台的每日签到」与「本地会话库管理」合并到一个界面。

- 语言：代码兼容 Python 3.10+（使用 `X | None` 语法，不要降级到 3.9），实际运行固定 3.12
- GUI：PySide6（Qt 6），仅 QtWidgets，**不使用 QML、不使用 Web 前端**
- 第三方依赖**只有 3 个**：`pyside6-essentials`、`requests`、`cryptography`，其余一律标准库
- 入口：`run.pyw`（pythonw，无控制台）→ `app/main.py`
- 形态：单进程桌面应用，无服务端、无独立数据库服务

**运行环境由 `pyproject.toml` 声明、`uv.lock` 锁定、`uv sync` 落到项目内 `.venv`（Python 3.12）。**

| 场景 | 命令 |
| --- | --- |
| 启动软件 | `.venv\Scripts\pythonw.exe run.pyw` |
| 开发（热重载） | `.venv\Scripts\python.exe dev.py` |
| 重建 / 修复环境 | 双击 `setup.vbs`（静默无窗口）；要看输出用 `setup.bat`；等价命令 `uv sync` |

**不要用第三方软件自带的 Python**（如 WorkBuddy 的 `~/.workbuddy/binaries/python/`）：它会随该软件升级被整体替换，第三方包一并丢失，表现为双击没反应。历史上已因此故障一次。
**不要用系统 `python` 直接启动**：`C:\Toolchain\Python314` 里没有本项目的锁定依赖，会 `ModuleNotFoundError`。

## 2. 目录与模块地图

```text
pyproject.toml          依赖声明 + 版本号（唯一真源）
uv.lock                 锁定 13 个包的精确版本；不要手改，用 `uv lock` 生成
.python-version         固定 3.12
setup.vbs / setup.bat   重建环境：vbs 静默，bat 显示详细输出
.venv/                  实际运行环境，uv 生成，不要手工往里装东西
CHANGELOG.md            版本更新记录，发布说明的唯一来源
.gitignore              入库范围：data/ legacy/ .venv/ 及工具私有目录

run.pyw                 正式启动入口（pythonw，无控制台），异常落日志
dev.py                  开发入口：改 app/ 下 .py 自动重建窗口
app/main.py             QApplication、DPI、单实例、数据迁移、create_window

app/core/               基础设施层，不含业务逻辑
  paths.py              所有数据文件路径的唯一来源
  config.py             配置读写（JSON，先写临时文件再替换）
  tasks.py              后台任务 submit() / parallel_map()，结果用信号回主线程
  bus.py                跨页面通知
  logbook.py            运行日志落盘（缓冲 + 定时 flush）
  winproc.py            进程启停与运行检测（ctypes，不产生窗口）
  shortcut.py           桌面快捷方式（powershell 隐藏执行）
  migrate.py            旧版数据一次性拷贝
  format.py             大小 / 时间格式化

app/ui/                 表现层，只做渲染与交互
  theme.py              PALETTE 色板 + qss() 全局样式
  widgets.py            Card / Badge / DataTable / EmptyState / TimeField 等通用控件
  models.py             RowModel（QAbstractTableModel）
  mainwindow.py         导航、页面懒加载、日志面板；PAGES / NAV_ITEMS 在此注册
  button_config.py      按钮可见性与排序配置
  dialogs.py            添加账号等对话框
  pages/                各功能页，均继承 pages/base.py 的 BasePage

app/checkin/            签到域
  base.py               Provider 抽象（scan / status / credits / checkin / client_exe）
  registry.py           平台注册表（PROVIDERS / LABELS / ORDER）
  service.py            编排：扫描、查询、签到、时刻推算
  store.py              账号库（Windows DPAPI 加密）
  ledger.py             积分台账（记录本工具亲眼发放的批次）
  logins.py             浏览器回调与设备码登录流程
  switcher.py           写回客户端登录态
  qoder.py / trae.py / workbuddy.py   三个平台适配器

app/sessions/           会话域
  editions.py           客户端版本目录与只读 SQLite 连接
  wbdb.py               会话库读写（写前必须 checkpoint）
  service.py            快照聚合 build_snapshot / build_unified
  profiles.py / backup.py / trash.py / cloud.py / codebuddy.py

data/                   运行期数据：config.json、accounts.dat、checked.json、
                        credits.json、backups/、logs/、profiles/、trash/
legacy/                 历史版本 zip 归档，只读
tools/                  一次性脚本（图标生成、截图）
```

改动定位原则：

- 改界面 → `app/ui/pages/<页面>.py`，必要时同步 `theme.py`
- 加页面 → 继承 `BasePage`，注册到 `mainwindow.py` 的 `PAGES`；要出现在左侧导航再加 `NAV_ITEMS`
- 加平台 → `app/checkin/` 下新增适配器，并在 `registry.py` 注册
- 改数据读写 → 先从 `app/core/paths.py` 确认路径来源
- **不要在 `app/ui/` 里直接读写 SQLite 或文件**

## 3. 编码规范

- 每个模块首行 `# -*- coding: utf-8 -*-`，随后一句中文 docstring 说明**为什么存在**，不写「本模块定义了什么类」
- 文件头统一 `from __future__ import annotations`
- 函数写返回类型注解；内部分支复杂时补参数类型
- 命名用 `snake_case`；Qt 覆写方法保持驼峰并加 `# noqa: N802`
- 注释只解释**非显然的取舍**（例如「这里必须 checkpoint，否则客户端读不到」），不解释代码字面意思
- 耗时操作（网络、文件、SQLite）必须 `from ..core import tasks` 后 `tasks.submit(...)`，**不得在 UI 线程执行**
- 后台线程不触碰任何 QWidget，结果一律通过回调回主线程
- 异常处理：禁止裸 `except: pass`；写盘失败必须上报或落日志
- 新增 UI 控件类名需在 `theme.py` 的 `qss()` 中补样式，否则外观会掉出设计体系
- 时间一律转本地时区再显示，紧凑格式用 `%m-%d %H:%M`
- 表格列已接近饱和（账号表 9 列）；单个账号的明细优先做成表格下方的面板，不要继续加列

## 4. 禁区与红线

以下为硬约束，任何情况下不得违反：

1. **不新增第三方依赖**。确需引入时，先说明为什么标准库做不到，等确认后再改；批准后必须同步写进 `pyproject.toml` 并 `uv lock`，禁止 `pip install` 到系统环境或 `.venv` 手工装
2. **不删除或清空** `data/`、`legacy/`、`.codebuddy/` 下的任何内容
3. **不在 UI 线程**做网络请求、文件读写、SQLite 查询
4. **不裸吞异常**，尤其是 `store.py`、`ledger.py`、`config.py` 的写盘路径
5. **不在官方客户端未关闭时写其 SQLite**，写前必须 `PRAGMA wal_checkpoint(TRUNCATE)`
6. **不输出、不记录、不提交**任何 token、`_AT_REST_SECRET`、DPAPI 解密结果、账号明文
7. **不引入** ORM、asyncio / qasync、Web 框架、打包工具变更——当前架构已定，超出范围
8. **不改** `app/__init__.py` 的 `APP_VERSION`，除非本次任务明确要求发版
9. **不重构**与当前任务无关的模块。发现别的问题，报告即可，不要顺手改
10. **不动** `data/backups/` 下的历史备份目录
11. **任何情况下不得弹出控制台窗口**。GUI 以 `pythonw` 运行，`sys.stdout` 为 `None`：
    - 不写 `print()`（需要输出时写 `data/logs/`，或包 `try/except AttributeError`）
    - 不裸调 `subprocess` / `os.system` / `os.popen`，禁止 `shell=True`
    - 外部命令一律走 `winproc.run_hidden()`（内含 `CREATE_NO_WINDOW` + `SW_HIDE`）
    - 启动客户端/打开目录一律 `winproc.start_exe()` / `open_path()`（`os.startfile`，不产生窗口）
    - 进程存在性判断用 `winproc.process_running()`（ctypes 快照），不要用 `tasklist`
    - 运维脚本若由用户双击，必须提供 `.vbs` 静默入口（参考 `setup.vbs`），不能让 bat 直接弹黑框
12. **不手工修改 `.venv`**。环境异常一律用 `setup.vbs` / `uv sync` 重建，不要在里面 `pip install` 或删文件

## 5. 验证与验收

改动完成后，按影响面选择验证方式，并在回复中说明**实际执行了什么**：

| 改动类型 | 必须做 |
| --- | --- |
| 任何改动 | `.venv\Scripts\python.exe run.pyw` 能启动且无 traceback；异常看 `data/logs/` |
| 涉及签到 / 账号逻辑 | `.venv\Scripts\python.exe run.pyw --selftest`，看 `data/logs/selftest.log` |
| 涉及 UI | 手工打开对应页面，确认布局、按钮、空态正常 |
| 涉及 `data/` 下任一文件 | 改前整目录复制到 `data/backups/manual-<时间戳>/` |
| 涉及官方客户端 SQLite | 先确认客户端进程已退出，再操作，并在副本上先试 |
| 涉及网络接口 | 说明是哪个域名与路径，失败时的降级行为是什么 |
| 涉及子进程 / 外部命令 / 脚本 | 用 `.venv\Scripts\pythonw.exe` 实跑一遍，确认**整个过程没有闪出控制台窗口** |

**完成 = 能启动 + 相关页面可用 + 无新增静默异常。** 仅「代码写完了」不算完成。

## 6. 领域事实（改逻辑前必读）

这几条是最容易被误改的契约，改动前先确认影响面：

- **签到状态以接口为准**：`data/checked.json` 只记录本工具成功签到的**时刻**（保留 3 天），接口返回 `checked_in` 时以接口为准，本地记录只做兜底
- **冷却口径两套**：`cooldown_hours = 0` 按自然日（次日 0 点可签）；`24` 按上次签到 + 24h（Qoder）。`service.next_checkin_at()` 只按本地记录推算，界面文案统一用「约」
- **积分批次不含获得时间**：平台接口只返回「金额 + 到期日 + 名称」。`live_credit=True`（WorkBuddy / Trae）**从不写台账**，`live_credit=False`（Qoder）才由 `ledger.add()` 记录发放时刻
- **账号主键**：`iid = f"{platform}:{account_id}"`，被账号库、积分台账、签到记录、UI 行 key 共用；改主键等于改四处存储
- **官方客户端 SQLite**：只读连接用 `mode=ro`；写前必须关闭进程并 `PRAGMA wal_checkpoint(TRUNCATE)`
- **单实例**：`Local\` 命名空间互斥体，句柄持有到进程退出，不要手工 `CloseHandle`

## 7. Git 与版本管理

### 7.1 提交

- 远端：`https://github.com/FaustDream/AI-Manager.git`，主分支 `main`
- 每次完整改动并验证后创建**一个** commit；一个 commit 对应一个可独立回退的改动，不混入格式化改动
- 提交信息用中文，格式 `<类型>: <一句话说明>`，类型取 `修复 / 新增 / 重构 / 文档 / 依赖 / 发布`
- 高风险改动（`data/`、`app/checkin/store.py`、`app/sessions/wbdb.py`）动手前先提交一个已验证版本作为回滚点
- 入库范围由 `.gitignore` 决定：`data/`、`legacy/`、`.venv/`、`.idea/`、`.codebuddy/`、`.workbuddy/`、`__pycache__/` 一律不入库
- 不提交任何 token、DPAPI 解密结果、账号明文

### 7.2 版本号

三段式 `主.次.修订`，当前 **1.0.0**：

| 段 | 含义 | 当前 |
| --- | --- | --- |
| `主` | 产品代次，只有结构性重写或用户明确要求才变 | `1` |
| `次` | 阶段预留段，日常保持 `0` | `0` |
| `修订` | 日常改动段，唯一日常递增的一位 | `0` |

- 三段均为非负整数，不用前导零（`1.0.01` 会被读成 `1.0.1`）
- 日常改动（优化、修复、新增功能）一律**修订 +1**：`1.0.0 → 1.0.1 → 1.0.2`，直接升级，无需确认
- 不跨档跳号：不得从 `1.0.0` 直接跳到 `1.1.0`
- 只有 `9` 结尾进位时才需要确认（`1.0.9 → 1.0.10`、`1.0.99 → 1.1.0`）；未确认则保持该 `9` 版本作为最新版本，后续改动并入该版本，不得自动进位
- 版本号只升不降；同一版本号不重复发布不同内容
- 两段式迁移为三段式：`A.B` 记作 `A.0.B`，只是格式换算，不算升级

### 7.3 每次升版必须同步三处

| # | 位置 | 说明 |
| --- | --- | --- |
| 1 | `app/__init__.py` 的 `APP_VERSION` | 运行时唯一来源，窗口标题与日志都读它 |
| 2 | `pyproject.toml` 的 `version` | 必须与上者完全一致 |
| 3 | `CHANGELOG.md` | 顶部新增该版本分组，写日期，条目只写最终行为 |

三处不一致即视为发布事故。

### 7.4 发布流程

1. 按第 5 节完成验证
2. 同步三处版本号
3. `git commit -m "发布: 1.0.1"`
4. `git tag -a v1.0.1 -m "AI Manager 1.0.1"`
5. `git push && git push --tags`
6. 在 GitHub Releases 用该 tag 建 Release，说明取 `CHANGELOG.md` 当前版本分组

Tag 与 Release 标题统一用完整版本号前缀 `v`（`v1.0.1` / `AI Manager 1.0.1`）。

## 8. 输出与协作方式

- 用中文回答
- **先读代码再回答**。涉及具体实现时，引用必须带 `路径:行号`，不凭印象描述
- 非平凡改动（涉及 2 个以上文件或涉及数据结构）**先给方案，确认后再动手**
- 不确定就说不确定。不猜测 API 名称、字段名、第三方客户端的内部格式
- 结论先行，不复述需求，不写「我将要…」的预告
- 发现本规范未覆盖的坑，指出并建议补进本文件，不要自己临时定规矩
