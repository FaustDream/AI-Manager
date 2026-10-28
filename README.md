# AI Manager

Windows 桌面工具：Trae / Qoder / WorkBuddy 三家 AI 编程平台的每日签到，加上本地会话库、账号档案与积分管理。

## 安装（不需要懂命令）

1. 点页面右上角 **Code → Download ZIP**，下载后解压到任意目录
2. 双击解压目录里的 **`一键安装.vbs`**
3. 等待 1–5 分钟（首次会下载约 250MB 运行环境），完成后软件自动打开，桌面会多一个 **AI Manager**

以后双击桌面快捷方式即可启动。

## 首次使用

1. 打开要管理的客户端（Trae / Qoder / WorkBuddy）并登录
2. 回到 AI Manager，进入「账号中心」，点「刷新登录态」
3. 选中任意账号，下方会显示积分明细、签到状态与下次签到倒计时

## 双击没反应怎么办

| 现象 | 处理 |
| --- | --- |
| 提示「Windows 已保护你的电脑」 | 点「更多信息」→「仍要运行」 |
| 文件被标记为来自网络 | 右键 `一键安装.vbs` → 属性 → 勾选「解除锁定」→ 确定 |
| 杀毒软件拦截 `.vbs` | 安装器只是脚本，源码全在本仓库可自行审阅；也可改用下面的手动方式 |

手动方式（任选其一）：

```text
双击 setup.bat        → 看到详细安装过程
双击 setup.vbs        → 静默安装，失败时才提示
命令行：uv sync --python 3.12
```

装好后直接运行 `.venv\Scripts\pythonw.exe run.pyw`。

## 环境要求

- Windows 10 / 11（64 位）
- 需要联网下载运行环境：uv + Python 3.12 + PySide6 等，约 250MB
- **不需要预装 Python，也不需要管理员权限**

## 开发

```powershell
uv sync --python 3.12              # 建环境（或双击 setup.vbs）
.venv\Scripts\python.exe dev.py    # 开发模式：改 app/ 下代码自动重建窗口
.venv\Scripts\python.exe run.pyw --selftest   # 只读自检，结果写入 data/logs/
```

- 项目约定见 [`AGENTS.md`](AGENTS.md)
- 更新记录见 [`CHANGELOG.md`](CHANGELOG.md)
- 运行期数据在 `data/`（不进仓库），删掉该目录等于恢复出厂设置

## 说明

本工具读取各客户端本机已登录的凭证来调用官方 Web 接口，不模拟界面点击，也不修改官方客户端的登录文件（切换账号功能会先备份再写回）。自动化签到可能违反平台服务条款，请自行评估风险。
