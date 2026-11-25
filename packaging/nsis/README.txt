AI Manager · Windows 版（浏览器形态）
====================================

2.x 没有 Electron/Tauri 窗口、托盘和开机自启：本机只跑一个 axum 后端
（127.0.0.1:5274），页面在默认浏览器里。安装包与免安装 zip 用的是同一套文件，
本说明在两种产物里都会随包分发。

一、两种产物
------------
- AI-Manager-<版本>-windows-x64-setup.exe
  双击安装，装到 %LOCALAPPDATA%\Programs\AI Manager，不需要管理员权限，不弹 UAC。
  开始菜单和桌面会生成「AI Manager」入口，指向 start.cmd。
- AI-Manager-<版本>-windows-x64-portable.zip
  解压到固定目录后双击 start.cmd，不需要安装。

两者都需要 Windows 10 及以上 + Edge/Chrome 等现代浏览器；不需要 .NET、WebView2、
Tauri 运行时，也不需要局域网环境。

二、启动与退出
--------------
- 双击「AI Manager」/ start.cmd：脚本拉起 ai-manager.exe，轮询 127.0.0.1:5274，
  就绪后用默认浏览器打开页面。
- 会保留一个最小化的后端控制台窗口，那是日志窗口；关掉它或按 Ctrl+C 会停后端。
- 页面右上角的电源按钮会确认后停掉后端（推荐）。只关浏览器标签页不会停后端。
- 后端只监听 127.0.0.1，不对局域网开放；同一时间只允许一个实例，重复启动会退出。
- 端口 5274 被占用时后端会直接退出，请看最小化控制台/日志定位原因。

三、数据放哪
------------
发布版的数据目录是 exe 同级的 data\：

    ai-manager.exe    本机后端（axum）
    dist\             前端页面（后端静态托管，勿删）
    start.cmd         启动器
    README.txt        本说明
    data\app.db       账号、签到、积分、会话索引、技能与 MCP（SQLite）
    data\config.toml  本地配置
    data\logs\        运行日志
    data\backups\     备份及保留目录
    data\skills\      技能库
    data\trash\       平台技能移除的回收站

- 换机器 / 整机备份：拷走整个文件夹即可。
- 高级用法：设置环境变量 AIMANAGER_DATA_DIR 可把 data\ 指到其他位置；日常不用设。
- 凭据按 Windows 用户用 DPAPI 加密：换用户或换机器后账号需要重新登录，
  这是有意为之，不是数据丢失。
- v1 的数据导入器已下线，程序不会读取或删除 data-v1-archive\；要留旧数据请自行备份。

四、升级与卸载
--------------
- 覆盖安装安装包：程序文件会被替换，data\ 原样保留。
- 免安装版升级：替换 ai-manager.exe、dist\、start.cmd，保留 data\ 即可。
- 安装包卸载：只删程序文件，保留 data\。想彻底清掉数据，卸载后手动删除 data\。
- 免安装版没有卸载器：删掉程序文件就是卸载；保留 data\ 以后放回同目录仍可用。
- 浏览器形态没有开机自启；旧版残留的 HKCU\Software\Microsoft\Windows\CurrentVersion\Run
  下 AIManager 值会在安装包卸载时清掉。

五、版本与校验
--------------
- 版本号见产物文件名：AI-Manager-<版本>-windows-x64-setup.exe / -portable.zip。
- GitHub Release 附 SHA256SUMS.txt，下载后可用 PowerShell 校验：
    Get-FileHash .\AI-Manager-<版本>-windows-x64-setup.exe -Algorithm SHA256
