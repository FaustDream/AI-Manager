@echo off
chcp 65001 >nul
cd /d "%~dp0"

echo ============================================
echo   AI Manager - 环境初始化 / 修复
echo ============================================
echo.

set "UV=%USERPROFILE%\.local\bin\uv.exe"
if not exist "%UV%" set "UV=uv"

echo [1/2] 按 pyproject.toml + uv.lock 同步依赖...
echo       首次约 250MB，之后秒级完成
"%UV%" sync --python 3.12
if errorlevel 1 (
    echo.
    echo [X] 同步失败。请确认已安装 uv：
    echo     https://docs.astral.sh/uv/getting-started/installation/
    pause
    exit /b 1
)

echo.
echo [2/2] 环境就绪
echo       解释器：.venv\Scripts\pythonw.exe
echo       启动：  双击桌面「AI Manager」
echo       开发：  .venv\Scripts\python.exe dev.py
echo.
pause
