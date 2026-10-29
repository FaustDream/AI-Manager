# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller 打包配置：onedir 免安装形态，Windows / Linux / macOS 共用一份。

为什么用 onedir 而不是 onefile：
    这程序启动时就要读 Qt 插件、图标和会话库，onefile 每次启动都要把上百 MB
    解压到临时目录，冷启动能多出好几秒；免安装目录整个拷走就能用，也更符合
    「数据跟着程序目录走」的既有设计（app/core/paths.py）。

图标与版本资源只在 Windows 生效，其他平台传 None 即可。
"""
import os
import sys
from pathlib import Path

ROOT = Path(SPECPATH).resolve().parent  # noqa: F821 - SPECPATH 由 PyInstaller 注入
ASSETS = ROOT / "assets"
VERSION_FILE = os.environ.get("AI_MANAGER_VERSION_FILE") or None
ICON_FILE = str(ASSETS / "app.ico") if sys.platform == "win32" else None

a = Analysis(  # noqa: F821
    [str(ROOT / "run.pyw")],
    pathex=[str(ROOT)],
    binaries=[],
    datas=[
        (str(ASSETS / "app.ico"), "assets"),
        (str(ASSETS / "app.png"), "assets"),
        (str(ROOT / "LICENSE"), "."),
        (str(ROOT / "README.md"), "."),
        (str(ROOT / "CHANGELOG.md"), "."),
    ],
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    # 只用 QtWidgets：QML / Quick / WebEngine / 3D / 多媒体都不进包，能省下几十 MB
    excludes=[
        "tkinter", "unittest", "pydoc_data", "test", "lib2to3",
        "PySide6.QtQml", "PySide6.QtQuick", "PySide6.QtQuickWidgets",
        "PySide6.QtQuick3D", "PySide6.QtWebEngineCore", "PySide6.QtWebEngineWidgets",
        "PySide6.QtWebChannel", "PySide6.QtMultimedia", "PySide6.QtMultimediaWidgets",
        "PySide6.Qt3DCore", "PySide6.Qt3DRender", "PySide6.QtCharts",
        "PySide6.QtDataVisualization", "PySide6.QtBluetooth", "PySide6.QtPositioning",
    ],
    noarchive=False,
    optimize=0,
)

pyz = PYZ(a.pure)  # noqa: F821

exe = EXE(  # noqa: F821
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="AIManager",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=False,          # GUI 程序：Windows 上等于 pythonw，不弹控制台
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon=ICON_FILE,
    version=VERSION_FILE,
)

coll = COLLECT(  # noqa: F821
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=False,
    upx_exclude=[],
    name="AIManager",
)
