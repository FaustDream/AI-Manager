@echo off
rem AI Manager launcher (browser edition): start the local backend, then open
rem the UI in your default browser. The minimized console window is the
rem backend's log console -- close it (or Ctrl+C inside) to stop the backend,
rem or use the power button in the page's top bar.
rem
rem Zero packaging: the backend is the cargo debug build. If it is missing,
rem this script builds it first (requires Rust toolchain + npm install once).
rem
rem ASCII only: cmd.exe reads batch files in the OEM codepage.

setlocal
set "ROOT=%~dp0"
rem Two layouts, one launcher: packaged (zip / installed) keeps the exe next to
rem this script; the repo keeps it under src-tauri\target\debug. Sibling first --
rem a packaged folder has no cargo tree to build from.
set "EXE=%ROOT%ai-manager.exe"
if not exist "%EXE%" set "EXE=%ROOT%src-tauri\target\debug\ai-manager.exe"
set "DIST=%ROOT%dist"
set "URL=http://127.0.0.1:5274/"
set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"

rem --- frontend assets: the backend serves dist/ ; build once if missing ---
if not exist "%DIST%\index.html" (
    echo [start] dist\index.html not found - building frontend...
    pushd "%ROOT%"
    call npm run build
    popd
    if not exist "%DIST%\index.html" (
        echo [start] frontend build failed. Run "npm install" then "npm run build" manually.
        pause
        exit /b 1
    )
)

rem --- backend exe: build once if missing, repo layout only ---
if not exist "%EXE%" (
    if not exist "%ROOT%src-tauri\Cargo.toml" (
        echo [start] ai-manager.exe not found. Keep ai-manager.exe, dist\ and start.cmd in the same folder.
        pause
        exit /b 1
    )
    echo [start] backend exe not found - building (debug^)...
    if not exist "%CARGO%" set "CARGO=cargo"
    pushd "%ROOT%src-tauri"
    "%CARGO%" build --bin ai-manager
    popd
    if not exist "%EXE%" (
        echo [start] backend build failed. Check the Rust toolchain.
        pause
        exit /b 1
    )
)

rem --- launch minimized: the console stays as the backend log window ---
echo [start] launching backend (minimized console = log window^)...
start "AI Manager backend" /D "%ROOT%" /MIN "%EXE%"

rem --- wait for the HTTP port, then open the browser ---
set /a TRIES=0
:wait
set /a TRIES+=1
curl -s -o nul "%URL%" >nul 2>&1
if not errorlevel 1 goto open
if %TRIES% GEQ 30 (
    echo [start] backend did not come up within 30s. Check the minimized console window.
    pause
    exit /b 1
)
timeout /t 1 /nobreak >nul
goto wait

:open
echo [start] opening %URL%
start "" "%URL%"
endlocal
exit /b 0
