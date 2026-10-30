; Inno Setup 6 安装包脚本：把 dist/AIManager 装成单文件 setup.exe。
;
; 为什么装到 %LOCALAPPDATA%\Programs：
;   程序把 data/ 放在 exe 同目录（app/core/paths.py），装到 Program Files
;   会因 UAC 虚拟化导致配置写不进去。装到用户目录还能免管理员提权。
;
; 版本号、源目录、输出名都由 packaging/build.py 通过 /D 传入，不要写死。
; 编码必须是 UTF-8 with BOM，否则 ISCC 按 ANSI 解析，中文全成乱码。

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef MySourceDir
  #define MySourceDir "..\dist\AIManager"
#endif
#ifndef MyOutputDir
  #define MyOutputDir "..\dist"
#endif
#ifndef MyOutputBaseFilename
  #define MyOutputBaseFilename "AI-Manager-setup"
#endif

[Setup]
AppId={{6F1E7C2A-9D34-4B58-8E71-3C5A0B9D2E64}
AppName=AI Manager
AppVersion={#MyAppVersion}
AppVerName=AI Manager {#MyAppVersion}
AppPublisher=FaustDream
AppPublisherURL=https://github.com/FaustDream/AI-Manager
AppSupportURL=https://github.com/FaustDream/AI-Manager/issues
AppUpdatesURL=https://github.com/FaustDream/AI-Manager/releases
DefaultDirName={localappdata}\Programs\AI Manager
DefaultGroupName=AI Manager
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableWelcomePage=yes
PrivilegesRequired=lowest
OutputDir={#MyOutputDir}
OutputBaseFilename={#MyOutputBaseFilename}
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\AIManager.exe
UninstallDisplayName=AI Manager
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no
AllowNoIcons=yes
VersionInfoVersion={#MyAppVersion}
VersionInfoCompany=FaustDream
VersionInfoDescription=AI Manager 安装程序
VersionInfoProductName=AI Manager
VersionInfoProductVersion={#MyAppVersion}

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: checkedonce

[Files]
Source: "{#MySourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\AI Manager"; Filename: "{app}\AIManager.exe"
Name: "{autodesktop}\AI Manager"; Filename: "{app}\AIManager.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\AIManager.exe"; Description: "启动 AI Manager"; Flags: nowait postinstall skipifsilent
