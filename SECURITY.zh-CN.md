<sub><a href="SECURITY.md">English</a> · <b>简体中文</b></sub>

# 安全策略

## 支持范围

只对最新 Release 提供安全修复。旧版本请先升级再报。

| 版本 | 支持 |
| --- | --- |
| 最新 Release | 是 |
| 更早版本 | 否 |

## 报告漏洞

**不要开公开 Issue。**

用 GitHub 的私密报告：<https://github.com/FaustDream/AI-Manager/security/advisories/new>

报告里请包含：影响版本、复现步骤、影响面（能读到什么 / 能写什么）。

## 重点关注

这个程序会读写凭据，以下几类优先级最高：

- **凭据泄露**：`data/` 下的密文被非当前 Windows 用户解开；日志、导出或界面里出现明文 token
- **越权写入**：不经确认就改写客户端配置文件（`data/backups/` 的存在就是为了让这件事可回滚）
- **本地端口暴露**：网页调试壳（`ai-manager-web`）本应只绑 `127.0.0.1`、校验 `x-ai-manager-web` 头与 Origin 白名单；能绕过其中任一即为漏洞
- **路径穿越**：导入技能或 MCP 时通过构造路径写到 `data/` 之外

## 不在范围内

- 需要本机管理员权限或物理接触才能利用的问题
- 第三方客户端（Qoder / WorkBuddy / Trae / ZCode）自身的漏洞，请报给对应厂商
- 自己把 `data/` 连同密文一起发出去导致的泄露（DPAPI 绑定当前 Windows 用户，换用户本来就解不开）

## 响应

单人维护，尽力在 7 天内给首次回复。确认的漏洞先出修复，再公开说明。
