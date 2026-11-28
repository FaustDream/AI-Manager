; AI Manager · Windows 安装包（NSIS，per-user / 浏览器形态）
;
; 2.x 运行形态：ai-manager.exe 是只绑 127.0.0.1:5274 的本机 axum 后端，页面由
; exe 同级 dist\ 静态托管；ai-manager-launcher.exe 是托盘常驻的启动器，负责
; 后端的启动 / 关闭 / 重启（快捷方式与完成页指向它）。start.cmd 保留作为
; 开发期兜底入口。没有 Tauri/WebView2/.NET 依赖。
;
; 安装包因此带：ai-manager.exe + ai-manager-launcher.exe + dist\ + start.cmd
; + README.txt + icon.ico。前端不嵌进 exe，也不需要 WebView2。
;
; 构建（仓库根目录，先 npm run build + 两个 cargo build --release）：
;   makensis.exe /INPUTCHARSET UTF8 packaging\nsis\ai-manager.nsi
; CI 用 choco 安装 NSIS；本地推荐走 tools\build-release.ps1，它会查找 NSIS。
;
; 两条不可退让的约定：
;   1. RequestExecutionLevel user + 装到 $LOCALAPPDATA\Programs\AI Manager：
;      全程不弹 UAC，普通用户双击即装。
;   2. 卸载只删程序文件。$INSTDIR\data 是账号库/会话/技能/备份，一律保留：
;      不能用 RMDir /r "$INSTDIR"，只能逐项删程序文件后 RMDir "$INSTDIR"
;      （不带 /r，data\ 还在时静默失败，正是想要的结果）。

Unicode true

!define PRODUCT_NAME "AI Manager"
; 版本单一真源是 Cargo.toml；读不到直接编译失败，避免产物名与包版本脱节。
!searchparse /file "..\..\src-tauri\Cargo.toml" `version = "` PRODUCT_VERSION `"`
!define PRODUCT_PUBLISHER "FaustDream"
!define PRODUCT_EXE "ai-manager.exe"
!define LAUNCHER_EXE "ai-manager-launcher.exe"
!define LAUNCHER_SRC "..\..\launcher\target\release"
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
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${LAUNCHER_EXE}"
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

  ; 覆盖安装前停掉常驻后端与托盘启动器，否则 exe 被占用会报 File 失败。
  ; 进程本来就不存在时 taskkill 返回非 0，这里有意忽略，但必须 Pop 平衡栈。
  nsExec::Exec 'taskkill /F /IM "${PRODUCT_EXE}" /T'
  Pop $0
  nsExec::Exec 'taskkill /F /IM "${LAUNCHER_EXE}" /T'
  Pop $0

  SetOutPath "$INSTDIR"

  ; 后端主程序（纯 axum 本机服务）
  File "${SRC_DIR}\${PRODUCT_EXE}"

  ; 托盘启动器（管理后端生命周期）
  File "${LAUNCHER_SRC}\${LAUNCHER_EXE}"

  ; 页面由 exe 同级 dist\ 托管：漏带 dist 会 404。用 * 覆盖无扩展名文件。
  SetOutPath "$INSTDIR\dist"
  File /r "${DIST_DIR}\*"
  SetOutPath "$INSTDIR"

  ; 启动器、随包说明、快捷方式图标
  File "..\..\start.cmd"
  File "README.txt"
  File "${ICON_FILE}"

  ; 全新安装建空 data\；覆盖安装时原样保留
  CreateDirectory "$INSTDIR\data"

  CreateDirectory "$SMPROGRAMS\AI Manager"
  CreateShortCut "$SMPROGRAMS\AI Manager\AI Manager.lnk" "$INSTDIR\${LAUNCHER_EXE}" "" "$INSTDIR\icon.ico" 0
  CreateShortCut "$SMPROGRAMS\AI Manager\Uninstall AI Manager.lnk" "$INSTDIR\uninstall.exe"
  CreateShortCut "$DESKTOP\AI Manager.lnk" "$INSTDIR\${LAUNCHER_EXE}" "" "$INSTDIR\icon.ico" 0
  ; 旧版曾用 AI Manager v2.lnk，升级上来时清掉，避免两个入口并存。
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

  ; 总大小减掉 data\ 才是「程序占用」；新增随包文件自动计入，不必维护清单。
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  ${GetSize} "$INSTDIR\data" "/S=0K" $3 $4 $5
  IntOp $0 $0 - $3
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

; ------------------------------------------------------------------ 卸载

Section "Uninstall"
  ; 单实例锁存在时 exe 会被占用，必须先停掉。用 nsExec 而不是 ExecWait，
  ; 后者会闪一个控制台窗口；返回值同样有意忽略，Pop 平衡栈。
  nsExec::Exec 'taskkill /F /IM "${LAUNCHER_EXE}" /T'
  Pop $0
  nsExec::Exec 'taskkill /F /IM "${PRODUCT_EXE}" /T'
  Pop $0

  Delete "$DESKTOP\AI Manager.lnk"
  Delete "$DESKTOP\AI Manager v2.lnk"
  Delete "$SMPROGRAMS\AI Manager\AI Manager.lnk"
  Delete "$SMPROGRAMS\AI Manager\Uninstall AI Manager.lnk"
  RMDir "$SMPROGRAMS\AI Manager"

  Delete "$INSTDIR\${PRODUCT_EXE}"
  Delete "$INSTDIR\${LAUNCHER_EXE}"
  Delete "$INSTDIR\start.cmd"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  ; dist\ 是可重建的纯程序文件，可整体删
  RMDir /r "$INSTDIR\dist"

  DeleteRegKey HKCU "${UNINST_KEY}"
  ; 浏览器形态之前留过自启项，值名 AIManager；现在自启由托盘启动器管理（AIManagerLauncher）。
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "AIManager"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "AIManagerLauncher"

  ; 关键：不带 /r。data\ 里是账号库、会话、技能与备份，卸载一律保留。
  ; 目录里只剩 data\ 时这条会静默失败，正是想要的结果。
  RMDir "$INSTDIR"
SectionEnd
