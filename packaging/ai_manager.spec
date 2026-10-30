# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller 打包配置：onedir 免安装形态，Windows / Linux / macOS 共用一份。

为什么用 onedir 而不是 onefile：
    这程序启动时就要读 Qt 插件、图标和会话库，onefile 每次启动都要把上百 MB
    解压到临时目录，冷启动能多出好几秒；免安装目录整个拷走就能用，也更符合
    「数据跟着程序目录走」的既有设计（app/core/paths.py）。

为什么单独做过滤：
    PySide6 的 hook 会把整包 Qt 插件和全部语言包都收进来，其中 opengl32sw.dll
    一项就 20MB，而本程序只用 QtWidgets 的 raster 绘制。这里在 Analysis 之后
    裁掉用不到的 DLL、插件与翻译，把解压后体积从约 110MB 压到 80MB 以内；
    release 只发压缩包，下载体积因此下降约三分之一。

图标与版本资源只在 Windows 生效，其他平台传 None 即可。
"""
import os
import sys
from pathlib import Path

from PyInstaller.building.datastruct import TOC

ROOT = Path(SPECPATH).resolve().parent  # noqa: F821 - SPECPATH 由 PyInstaller 注入
ASSETS = ROOT / "assets"
VERSION_FILE = os.environ.get("AI_MANAGER_VERSION_FILE") or None
ICON_FILE = str(ASSETS / "app.ico") if sys.platform == "win32" else None

# 极少数机器（无 GPU 且禁用了 D3D）需要软件 OpenGL 兜底；默认裁掉省 20MB，
# 真遇到启动即崩可以带这个变量重新打包。
KEEP_SOFTGL = os.environ.get("AI_MANAGER_KEEP_SOFTGL") == "1"

# docs/ 只在开发者本机存在（已 gitignore，CI 检出后没有该目录），
# 所以这里按存在与否逐个加入，缺失时跳过，避免打包直接报错。
DATAS = [
    (str(ASSETS / "app.ico"), "assets"),
    (str(ASSETS / "app.png"), "assets"),
    (str(ROOT / "LICENSE"), "."),
]
for _doc in ("README.md", "CHANGELOG.md"):
    _path = ROOT / "docs" / _doc
    if _path.is_file():
        DATAS.append((str(_path), "."))

# 不需要的 Qt 插件目录：TLS / 网络信息 / QtSql / 打印 / 触摸屏
_DROP_PLUGIN_GROUPS = ("generic", "networkinformation", "printsupport", "sqldrivers", "tls")
# 代码里只出现 QtCore / QtGui / QtWidgets / QtSvg，其余 Qt 模块连 DLL 一起清掉
_DROP_BINARIES = {"qt6network.dll", "qtnetwork.pyd"}
# 只保留中文翻译（QMessageBox、QFileDialog 的按钮文案走这套）
_KEEP_TRANSLATION = "qtbase_zh_cn.qm"


def _keep_plugin(path: str, name: str) -> bool:
    """平台 / 图片 / 图标插件只留必需项。

    只裁 Windows：Linux 是 libqxcb.so、macOS 是 libqcocoa.dylib，按这份白名单
    过滤会把整个平台插件删光，包能打出来但启动就是一句 Qt platform plugin 报错。
    这些 .dll 在 PyInstaller 里走 binaries 而不是 datas，所以两处都要过一遍。
    """
    if sys.platform != "win32":
        return True
    if "/plugins/platforms/" in path:
        return name == "qwindows.dll"       # minimal / offscreen / direct2d 用不到
    if "/plugins/imageformats/" in path:
        return name == "qico.dll"           # PNG 内置；JPEG/TIFF/WEBP 没有对应资源
    if "/plugins/iconengines/" in path:
        return name == "qsvgicon.dll"
    return True


def _keep_binary(dest: str) -> bool:
    path = dest.replace("\\", "/").lower()
    name = path.rsplit("/", 1)[-1]
    if name in _DROP_BINARIES or name.startswith("libqt6network"):
        return False
    if name == "opengl32sw.dll" and not KEEP_SOFTGL:
        return False
    if any(f"/plugins/{group}/" in path for group in _DROP_PLUGIN_GROUPS):
        return False
    return _keep_plugin(path, name)


def _keep_data(dest: str) -> bool:
    path = dest.replace("\\", "/").lower()
    name = path.rsplit("/", 1)[-1]
    if "/translations/" in path:
        return name == _KEEP_TRANSLATION
    return _keep_plugin(path, name)


a = Analysis(  # noqa: F821
    [str(ROOT / "run.pyw")],
    pathex=[str(ROOT)],
    binaries=[],
    datas=DATAS,
    hiddenimports=[],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    # 只用 QtWidgets：QML / Quick / WebEngine / 3D / 多媒体都不进包，能省下几十 MB
    excludes=[
        "tkinter", "unittest", "pydoc_data", "test", "lib2to3",
        "setuptools", "pkg_resources", "_distutils_hack",
        "PySide6.QtQml", "PySide6.QtQuick", "PySide6.QtQuickWidgets",
        "PySide6.QtQuick3D", "PySide6.QtWebEngineCore", "PySide6.QtWebEngineWidgets",
        "PySide6.QtWebChannel", "PySide6.QtMultimedia", "PySide6.QtMultimediaWidgets",
        "PySide6.Qt3DCore", "PySide6.Qt3DRender", "PySide6.QtCharts",
        "PySide6.QtDataVisualization", "PySide6.QtBluetooth", "PySide6.QtPositioning",
        "PySide6.QtNetwork", "PySide6.QtSql", "PySide6.QtXml", "PySide6.QtTest",
        "PySide6.QtDBus", "PySide6.QtPrintSupport", "PySide6.QtOpenGL",
        "PySide6.QtOpenGLWidgets", "PySide6.QtUiTools",
    ],
    noarchive=False,
    # 2 = 去掉 assert 与 docstring；项目里没有拿 assert 做校验的地方
    optimize=2,
)

a.binaries = TOC([item for item in a.binaries if _keep_binary(item[0])])
a.datas = TOC([item for item in a.datas if _keep_data(item[0])])

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
    upx=False,              # UPX 会触发杀软误报，体积靠裁插件和 zip 高压缩解决
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
