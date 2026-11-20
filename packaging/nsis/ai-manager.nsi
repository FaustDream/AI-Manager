; AI Manager · NSIS 安装包脚本（免管理员 / per-user · 浏览器形态）
;
; 2.0.26 起为纯浏览器形态：exe 是本机 axum 后端，页面由 exe 同级 dist/ 托管，
; start.cmd 负责拉后端 + 开浏览器。安装包因此带四样东西：
;   ai-manager.exe + dist/ + start.cmd + README.txt（外加 icon.ico 供快捷方式）。
; 不再嵌前端进 exe、不再需要 WebView2。
;
; 构建命令（在仓库根目录执行，路径按脚本所在目录解析；需先 npm run build + cargo build --release）：
;   "%LOCALAPPDATA%\tauri\NSIS\makensis.exe" /INPUTCHARSET UTF8 packaging\nsis\ai-manager.nsi
;
; 两条硬约定：
;   1. `RequestExecutionLevel user` + 装到 `$LOCALAPPDATA\Programs\AI Manager`：
;      全程不弹 UAC，普通用户双击即装。
;   2. 卸载**只删程序文件**，`$INSTDIR\data` 一律保留（AGENTS.md §2 第 2 条）。
;      数据目录是「exe 同级的 data/」，就在安装目录里，
;      所以卸载器不能 `RMDir /r "$INSTDIR"`——那样会把账号库、会话索引一起删掉。
;      这里逐个删程序文件（含 dist/），最后 `RMDir "$INSTDIR"`（不带 /r，非空则静默失败）。

Unicode true

!define PRODUCT_NAME "AI Manager"
!define PRODUCT_VERSION "2.0.30"
!define PRODUCT_PUBLISHER "FaustDream"
!define PRODUCT_EXE "ai-manager.exe"
!define INSTALL_DIR "$LOCALAPPDATA\Programs\AI Manager"
!define SRC_DIR "..\..\src-tauri\target\release"
!define DIST_DIR "..\..\dist"
!define ICON_FILE "..\..\src-tauri\icons\icon.ico"
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
; exe 不再内嵌图标（tauri build.rs 已删），安装器与快捷方式统一用 icons/icon.ico
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
; 浏览器形态入口是 start.cmd（拉后端 + 开浏览器），不是裸 exe
!define MUI_FINISHPAGE_RUN "$INSTDIR\start.cmd"
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
  ; 覆盖安装前先停掉常驻后端旧实例：进程不退出 File 必报占用
  nsExec::Exec 'taskkill /F /IM "${PRODUCT_EXE}" /T'
  SetOutPath "$INSTDIR"

  ; 后端主程序（纯 axum 本机服务）
  File "${SRC_DIR}\${PRODUCT_EXE}"

  ; 前端页面：exe 运行时从自身同级 dist/ 托管，漏带 dist 页面 404
  SetOutPath "$INSTDIR\dist"
  File /r "${DIST_DIR}\*.*"
  SetOutPath "$INSTDIR"

  ; 启动器 + 说明 + 图标（快捷方式 / 卸载登记用）
  File "..\..\start.cmd"
  File "README.txt"
  File "${ICON_FILE}"

  ; 保留数据：全新安装时建一个空的 data 目录，卸载器按约定不碰它
  CreateDirectory "$INSTDIR\data"

  CreateDirectory "$SMPROGRAMS\AI Manager"
  CreateShortCut "$SMPROGRAMS\AI Manager\AI Manager.lnk" "$INSTDIR\start.cmd" "" "$INSTDIR\icon.ico" 0
  CreateShortCut "$SMPROGRAMS\AI Manager\Uninstall AI Manager.lnk" "$INSTDIR\uninstall.exe"
  ; 桌面图标固定叫 `AI Manager`。同名只会覆盖旧入口，不存在「装一个删掉另一个」。
  CreateShortCut "$DESKTOP\AI Manager.lnk" "$INSTDIR\start.cmd" "" "$INSTDIR\icon.ico" 0
  ; 2.0.10 及更早的 v2 安装包建的是 `AI Manager v2.lnk`，升级上来时清掉。
  Delete "$DESKTOP\AI Manager v2.lnk"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
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
  ; 关掉正在运行的后端实例：单实例锁存在时 exe 会被占用，删不掉。
  ; 走 nsExec 而不是 ExecWait——后者会闪一个控制台窗口。
  nsExec::Exec 'taskkill /F /IM "${PRODUCT_EXE}" /T'
  Pop $0

  Delete "$DESKTOP\AI Manager.lnk"
  Delete "$DESKTOP\AI Manager v2.lnk"
  Delete "$SMPROGRAMS\AI Manager\AI Manager.lnk"
  Delete "$SMPROGRAMS\AI Manager\Uninstall AI Manager.lnk"
  RMDir "$SMPROGRAMS\AI Manager"

  Delete "$INSTDIR\${PRODUCT_EXE}"
  Delete "$INSTDIR\start.cmd"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  ; 前端页面目录整体删除（纯程序文件，可重建）
  RMDir /r "$INSTDIR\dist"

  DeleteRegKey HKCU "${UNINST_KEY}"

  ; 关键：不带 /r。$INSTDIR\data 里是用户的账号库、会话索引、技能库与备份，
  ; 卸载一律保留。目录里只剩 data/ 时这条会静默失败，正是想要的结果。
  RMDir "$INSTDIR"

  ; 顺手清掉旧版自启项（值名 AIManager）：浏览器形态已无自启，留着会指向不存在的 exe
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "AIManager"
SectionEnd
