# -*- coding: utf-8 -*-
"""AI Manager 启动入口（无控制台窗口）。

桌面快捷方式指向 pythonw.exe + 本文件；打包成 exe 后入口为 exe 本身。
pythonw 没有控制台，启动期异常若不兜底就直接静默闪退——
这里统一捕获并弹窗 + 落日志，方便定位问题。
"""
import os
import sys
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from app.main import main  # noqa: E402

if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except Exception:
        detail = traceback.format_exc()
        try:
            from app.core import logbook
            logbook.append(detail)
        except Exception:
            pass
        try:
            from PySide6.QtWidgets import QApplication, QMessageBox
            app = QApplication.instance() or QApplication(sys.argv)
            QMessageBox.critical(None, "AI Manager 启动失败", detail[-1500:])
        except Exception:
            # 连 Qt 都起不来时写旁路文件，至少留个痕迹
            try:
                with open(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                       "data", "logs", "crash.log"), "a",
                          encoding="utf-8") as fh:
                    fh.write(detail + "\n")
            except OSError:
                pass
        sys.exit(1)
