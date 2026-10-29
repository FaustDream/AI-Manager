# -*- coding: utf-8 -*-
"""把当前代码打成免安装包，并生成带版本号的压缩包。

用法（在项目根执行，需已装 PyInstaller，见 packaging/requirements-build.txt）：

    .venv-build\\Scripts\\python.exe packaging\\build.py      # Windows
    .venv-build/bin/python packaging/build.py                # Linux / macOS

产物：
    dist/AIManager/                                  免安装目录（可直接运行）
    dist/AI-Manager-<版本>-<平台>-<架构>.zip|.tar.gz  发布用压缩包

版本号只从 app/__init__.py 读，不在这里重复写一份——避免打包脚本和程序里
显示的版本对不上（历史上 tag 与 APP_VERSION 就走过一次岔）。
"""
from __future__ import annotations

import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APP_INIT = ROOT / "app" / "__init__.py"
SPEC = ROOT / "packaging" / "ai_manager.spec"
DIST = ROOT / "dist"
BUILD = ROOT / "build"
APP_NAME = "AIManager"

PLATFORM_TAG = {"win32": "windows", "darwin": "macos"}.get(sys.platform, "linux")
ARCH_TAG = {
    "AMD64": "x64", "x86_64": "x64", "x64": "x64",
    "arm64": "arm64", "aarch64": "arm64", "ARM64": "arm64",
}.get(platform.machine(), platform.machine().lower() or "unknown")

VERSION_TEMPLATE = """VSVersionInfo(
  ffi=FixedFileInfo(
    filevers=({a}, {b}, {c}, 0),
    prodvers=({a}, {b}, {c}, 0),
    mask=0x3f,
    flags=0x0,
    OS=0x40004,
    fileType=0x1,
    subtype=0x0,
    date=(0, 0)
  ),
  kids=[
    StringFileInfo([
      StringTable('040904B0', [
        StringStruct('CompanyName', 'FaustDream'),
        StringStruct('FileDescription', 'AI Manager 工作台'),
        StringStruct('FileVersion', '{version}'),
        StringStruct('InternalName', 'AIManager'),
        StringStruct('LegalCopyright', '© 2026 FaustDream'),
        StringStruct('OriginalFilename', 'AIManager.exe'),
        StringStruct('ProductName', 'AI Manager'),
        StringStruct('ProductVersion', '{version}')
      ])
    ]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])
  ]
)
"""


def read_version() -> str:
    text = APP_INIT.read_text(encoding="utf-8")
    match = re.search(r'APP_VERSION\s*=\s*"([^"]+)"', text)
    if not match:
        raise SystemExit("app/__init__.py 里找不到 APP_VERSION")
    return match.group(1)


def write_windows_version_file(version: str) -> Path:
    """给 exe 嵌入版本资源：右键属性里能看到版本与版权，而不是空白。"""
    parts = [int(p) for p in version.split(".") if p.isdigit()]
    while len(parts) < 3:
        parts.append(0)
    target = BUILD / "version_info.txt"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        VERSION_TEMPLATE.format(a=parts[0], b=parts[1], c=parts[2], version=version),
        encoding="utf-8",
    )
    return target


def check_pyinstaller() -> None:
    try:
        import PyInstaller  # noqa: F401
    except ImportError:
        raise SystemExit(
            "缺少 PyInstaller。先建一个独立的打包环境：\n"
            "  uv venv .venv-build --python 3.12\n"
            "  uv pip install --python .venv-build pyside6-essentials requests cryptography"
            " -r packaging/requirements-build.txt\n"
            "再用 .venv-build 里的解释器运行本脚本（不要往 .venv 里装）。"
        )


def run_pyinstaller(version: str) -> None:
    env = dict(os.environ)
    env.pop("AI_MANAGER_VERSION_FILE", None)
    if sys.platform == "win32":
        env["AI_MANAGER_VERSION_FILE"] = str(write_windows_version_file(version))
    cmd = [sys.executable, "-m", "PyInstaller", str(SPEC),
           "--noconfirm", "--clean",
           "--distpath", str(DIST), "--workpath", str(BUILD)]
    subprocess.run(cmd, cwd=str(ROOT), env=env, check=True)


def make_archive(version: str) -> Path:
    """压缩包内顶层就是 AIManager/，解压即得可运行目录。"""
    stem = f"AI-Manager-{version}-{PLATFORM_TAG}-{ARCH_TAG}"
    fmt = "zip" if sys.platform == "win32" else "gztar"
    target = shutil.make_archive(str(DIST / stem), fmt, root_dir=str(DIST), base_dir=APP_NAME)
    return Path(target)


def clean_outputs() -> None:
    """删掉上一次的产物；删不动就明确报错，不要带着旧文件继续打包。"""
    for path in (DIST, BUILD):
        if not path.exists():
            continue
        try:
            shutil.rmtree(path)
        except Exception as exc:  # noqa: BLE001 - 权限/占用/安全策略都可能有各自的异常类型
            raise SystemExit(f"清理 {path} 失败：{exc}\n请手动删除该目录后重试")


def main() -> int:
    version = read_version()
    check_pyinstaller()
    clean_outputs()

    print(f"[build] AI Manager {version} · {PLATFORM_TAG}-{ARCH_TAG} · python {sys.version.split()[0]}")
    run_pyinstaller(version)
    app_dir = DIST / APP_NAME
    if not (app_dir / APP_NAME).exists() and not (app_dir / f"{APP_NAME}.exe").exists():
        raise SystemExit(f"打包结果不完整：{app_dir} 里没有可执行文件")

    archive = make_archive(version)
    size_mb = archive.stat().st_size / 1024 / 1024
    print(f"[build] 目录：{app_dir}")
    print(f"[build] 压缩包：{archive}（{size_mb:.1f} MB）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
