AI Manager 2.0.15 · 免安装版（Windows x64）
==========================================

一、怎么用
----------
1. 把整个文件夹解压到任意位置（建议放到固定目录，别放临时目录）。
2. 双击 ai-manager.exe 即可启动。不需要安装、不需要管理员权限、不需要 .NET。
   系统要求：Windows 10 1803 及以上（依赖系统自带的 WebView2 运行时；Win11 自带）。
3. 关窗口不会退出：程序收到托盘里继续跑每日签到。要真正退出，右键托盘图标选「退出」。
4. 开机自启在「设置 → 通用」里打开，开关直接读写注册表 HKCU，不需要管理员。

二、数据放哪
------------
全部数据都在 exe 同级的 data\ 目录里：

    ai-manager.exe
    data\app.db            账号、签到、积分、会话索引、技能与 MCP（SQLite）
    data\logs\             运行日志
    data\backups\          写客户端配置前的自动备份
    data\skills\           技能库
    data\trash\            平台技能移除的回收站

- 换机器 / 备份：整个文件夹拷走即可，账号库跟着走。
- 从 v1 升级：v1 数据导入器已在 v2 下线，程序不会读取或删除旧版目录；
  如需保留旧数据，请自行备份 data-v1-archive\（本程序不会触碰该目录）。
- 账号库的凭据用 Windows DPAPI 加密，与当前 Windows 用户绑定：换用户或换机器后
  账号需要重新登录（这是有意为之，不是缺陷）。

三、卸载
--------
免安装版没有安装器，删掉程序文件就是卸载：

- 想彻底清干净：先退出程序（托盘 → 退出），再删掉整个文件夹（含 data\）。
- 只想删程序、留下数据：删掉 ai-manager.exe，保留 data\ 目录；以后换新版 exe
  放回同一目录，数据照旧可用。
- 如果开过开机自启，删文件夹前先在「设置 → 通用」里关掉，或手工删掉注册表
  HKCU\Software\Microsoft\Windows\CurrentVersion\Run 下的 AIManager 值。

四、校验
--------
版本：2.0.15    产物：AI-Manager-2.0.15-windows-x64-portable.zip
安装版请用 AI-Manager-2.0.15-windows-x64-setup.exe（装到 %LOCALAPPDATA%\Programs\AI Manager，
免管理员，卸载同样保留 data\）。
