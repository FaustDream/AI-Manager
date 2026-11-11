; AI Manager · NSIS 安装包脚本（免管理员 / per-user）
;
; 为什么手写而不是 `cargo tauri build`：本机没有 `cargo tauri` 子命令（未安装 tauri-cli），
; 但 `%LOCALAPPDATA%\tauri\NSIS\makensis.exe` 已经被缓存好了，直接调 makensis 即可，
; 不必为了打一个包去装一整套 CLI。
;
; 构建命令（在仓库根目录执行，路径按脚本所在目录解析）：
;   "%LOCALAPPDATA%\tauri\NSIS\makensis.exe" /INPUTCHARSET UTF8 packaging\nsis\ai-manager.nsi
;
; 两条硬约定：
;   1. `RequestExecutionLevel user` + 装到 `$LOCALAPPDATA\Programs\AI Manager`：
;      全程不弹 UAC，普通用户双击即装（对应 phase-7-polish §2.6）
;   2. 卸载**只删程序文件**，`$INSTDIR\data` 一律保留（AGENTS.md §4 第 2 条）。
;      2.0 的数据目录是「exe 同级的 data/」，它就在安装目录里面，
;      所以卸载器不能 `RMDir /r "$INSTDIR"`——那样会把用户的账号库、会话索引一起删掉。
;      这里改成逐个删程序文件，最后 `RMDir "$INSTDIR"`（不带 /r，非空则静默失败）。

Unicode true

!define PRODUCT_NAME "AI Manager"
!define PRODUCT_VERSION "2.0.11"
!define PRODUCT_PUBLISHER "FaustDream"
!define PRODUCT_EXE "ai-manager.exe"
!define INSTALL_DIR "$LOCALAPPDATA\Programs\AI Manager"
!define SRC_DIR "..\..\src-tauri\target\release"
!define OUT_FILE "..\..\release\AI-Manager-${PRODUCT_VERSION}-windows-x64-setup.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\AIManager"

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "${INSTALL_DIR}"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "FileVersion" "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=1033 "CompanyName" "${PRODUCT_PUBLISHER}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "© 2026 ${PRODUCT_PUBLISHER}"
VIAddVersionKey /LANG=1033 "FileDescription" "AI Manager installer"

!include "MUI2.nsh"
!include "FileFunc.nsh"

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${PRODUCT_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Run AI Manager now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"

; ------------------------------------------------------------------ 安装

Section "AI Manager" SEC_MAIN
  SectionIn RO
  ; 覆盖安装前先停掉常驻托盘的旧实例：关窗口不退出，不杀进程 File 必报占用
  nsExec::Exec 'taskkill /F /IM "${PRODUCT_EXE}" /T'
  SetOutPath "$INSTDIR"

  ; 主程序。Tauri 2 把前端资源嵌进 exe，安装目录里只有这一个可执行文件。
  File "${SRC_DIR}\${PRODUCT_EXE}"

  ; WebView2 走系统自带（Win10 1803+ 默认安装）；若构建产物里有加载器一并带上。
  ; 必须用编译期 `!if /FileExists`：`File` 是编译期指令，运行期的 IfFileExists 跳不过它。
  !if /FileExists "${SRC_DIR}\WebView2Loader.dll"
    File "${SRC_DIR}\WebView2Loader.dll"
  !endif

  ; 保留数据：全新安装时建一个空的 data 目录，卸载器按约定不碰它
  CreateDirectory "$INSTDIR\data"

  CreateDirectory "$SMPROGRAMS\AI Manager"
  CreateShortCut "$SMPROGRAMS\AI Manager\AI Manager.lnk" "$INSTDIR\${PRODUCT_EXE}" "" "$INSTDIR\${PRODUCT_EXE}" 0
  CreateShortCut "$SMPROGRAMS\AI Manager\Uninstall AI Manager.lnk" "$INSTDIR\uninstall.exe"
  ; 桌面图标固定叫 `AI Manager`。v1（同一文件名 `AI Manager.lnk`）已下线、安装包不再分发，
  ; 同名只会覆盖那条指向旧 exe 的失效入口，不存在「装一个删掉另一个」的问题。
  CreateShortCut "$DESKTOP\AI Manager.lnk" "$INSTDIR\${PRODUCT_EXE}" "" "$INSTDIR\${PRODUCT_EXE}" 0
  ; 2.0.10 及更早的 v2 安装包建的是 `AI Manager v2.lnk`，升级上来时清掉，桌面别留两个图标。
  Delete "$DESKTOP\AI Manager v2.lnk"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${PRODUCT_EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  ; 体积只统计程序文件，不含 data/（用户数据不该算进「程序占用」）
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

; ------------------------------------------------------------------ 卸载

Section "Uninstall"
  ; 关掉正在运行的实例：单实例互斥体存在时 exe 会被占用，删不掉。
  ; 走 nsExec 而不是 ExecWait——后者会闪一个控制台窗口。
  nsExec::Exec 'taskkill /F /IM "${PRODUCT_EXE}" /T'
  Pop $0

  Delete "$DESKTOP\AI Manager.lnk"
  ; 旧版（≤2.0.10）的图标名，卸载时一并清掉
  Delete "$DESKTOP\AI Manager v2.lnk"
  Delete "$SMPROGRAMS\AI Manager\AI Manager.lnk"
  Delete "$SMPROGRAMS\AI Manager\Uninstall AI Manager.lnk"
  RMDir "$SMPROGRAMS\AI Manager"

  Delete "$INSTDIR\${PRODUCT_EXE}"
  Delete "$INSTDIR\WebView2Loader.dll"
  Delete "$INSTDIR\uninstall.exe"

  DeleteRegKey HKCU "${UNINST_KEY}"

  ; 关键：不带 /r。$INSTDIR\data 里是用户的账号库、会话索引、技能库与备份，
  ; 卸载一律保留（用户重装后还能直接用，也符合 AGENTS「卸载不删数据」）。
  ; 目录里只剩 data/ 时这条会静默失败，正是想要的结果。
  RMDir "$INSTDIR"

  ; 顺手清掉自启项（值名 AIManager，见 app/process.rs 的 AUTOSTART_NAME）：
  ; 留着会指向一个不存在的 exe
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "AIManager"
SectionEnd
