# -*- coding: utf-8 -*-
"""ZCode 领取要过的阿里云验证码。

上游对 billing/claim 强制要求验证码参数，而验证码只能在浏览器里跑（需要官方 SDK）。
这里起一个只监听 127.0.0.1 的临时页面：加载 SDK → 无痕验证 → 把参数 POST 回来 → 关服务。
窗口关掉就结束，不留常驻端口，也不依赖 QtWebEngine。
"""
from __future__ import annotations

import json
import threading
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import requests

from . import zquota

TIMEOUT = 180
DEFAULT = {"enabled": True, "region": "cn", "prefix": "no8xfe", "scene_id": "11xygtvd"}

_PAGE = """<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>ZCode 领取验证</title>
<style>
body{margin:0;background:#0a0a0c;color:#e8e8ee;font:14px/1.6 system-ui,"Microsoft YaHei",sans-serif;
display:flex;align-items:center;justify-content:center;height:100vh}
.box{max-width:420px;padding:28px 32px;border:1px solid #24242c;border-radius:14px;background:#131318}
h1{font-size:16px;margin:0 0 10px}
#msg{color:#9a9aa8;font-size:13px;min-height:20px}
#dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:#3b82f6;margin-right:8px}
#dot.ok{background:#22c55e}#dot.err{background:#ef4444}
button{margin-top:16px;padding:8px 18px;border-radius:8px;border:1px solid #3b82f6;background:#1d4ed8;
color:#fff;font-size:13px;cursor:pointer}
#cap-holder{margin-top:14px}
</style></head>
<body><div class="box">
<h1>ZCode 活动领取</h1>
<div><span id="dot"></span><span id="msg">正在准备验证…</span></div>
<div id="cap-holder"></div>
<button id="cap-btn" hidden>点此完成验证</button>
</div>
<script>
var CFG = __CFG__, DONE = false;
function say(t, tone) {
  document.getElementById("msg").textContent = t;
  document.getElementById("dot").className = tone || "";
}
function finish(param) {
  if (DONE || !param) return;
  DONE = true;
  say("验证通过，正在提交领取…", "ok");
  fetch("/done", {method: "POST", headers: {"Content-Type": "application/json"},
    body: JSON.stringify({param: param})})
    .then(function () { say("已完成，可以关闭本页", "ok"); setTimeout(function () { window.close(); }, 1200); })
    .catch(function () { say("回传失败，请回到 AI Manager 查看", "err"); });
}
function loadSdk() {
  return new Promise(function (resolve, reject) {
    if (typeof window.initAliyunCaptcha === "function") return resolve();
    var s = document.createElement("script");
    s.src = "https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js";
    s.onload = resolve;
    s.onerror = function () { reject(new Error("验证码组件加载失败")); };
    document.head.appendChild(s);
  });
}
loadSdk().then(function () {
  window.AliyunCaptchaConfig = {region: CFG.region, prefix: CFG.prefix};
  var timer = setTimeout(function () {
    document.getElementById("cap-btn").hidden = false;
    say("需要手动完成一次验证", "");
  }, 8000);
  window.initAliyunCaptcha({
    SceneId: CFG.scene_id, mode: "popup", language: "zh-CN", showErrorTip: false,
    element: "#cap-holder", button: "#cap-btn",
    getInstance: function (instance) {
      if (typeof instance.startTracelessVerification === "function") {
        instance.startTracelessVerification();
      } else {
        document.getElementById("cap-btn").hidden = false;
      }
    },
    success: function (p) { clearTimeout(timer); finish(typeof p === "string" ? p : p && p.captchaVerifyParam); },
    fail: function () { clearTimeout(timer); document.getElementById("cap-btn").hidden = false; say("验证未通过，请重试", "err"); },
    onError: function (e) { clearTimeout(timer); say("验证出错：" + ((e && e.message) || e), "err"); }
  });
}).catch(function (e) { say(e.message, "err"); });
</script></body></html>
"""


class CaptchaError(RuntimeError):
    pass


def config() -> dict:
    """验证码场景参数由上游下发；拿不到就退回长期观测到的默认值。"""
    try:
        resp = requests.get("https://zcode.z.ai/api/v1/client/configs",
                            headers=zquota.zai_headers(""), timeout=10)
        data = resp.json()
        node = ((data.get("data") or {}).get("configs") or {}).get("captcha") or {}
        if data.get("code") in (0, 200) and node.get("sceneId"):
            return {
                "enabled": bool(node.get("enabled", True)),
                "region": str(node.get("region") or "cn"),
                "prefix": str(node.get("prefix") or DEFAULT["prefix"]),
                "scene_id": str(node.get("sceneId")),
            }
    except (requests.RequestException, ValueError, AttributeError):
        pass
    return dict(DEFAULT)


class _Server:
    def __init__(self, cfg: dict) -> None:
        self.param = ""
        self._done = threading.Event()
        page = _PAGE.replace("__CFG__", json.dumps(cfg, ensure_ascii=False))
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args) -> None:  # noqa: N802 - 别往 stderr 写
                return

            def do_GET(self) -> None:  # noqa: N802
                body = page.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self) -> None:  # noqa: N802
                length = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(length) if length else b"{}"
                try:
                    outer.param = str(json.loads(raw.decode("utf-8")).get("param") or "")
                except (ValueError, AttributeError):
                    outer.param = ""
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"ok":true}')
                outer._done.set()

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}/"

    def wait(self, timeout: int) -> str:
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        try:
            webbrowser.open(self.url)
            if not self._done.wait(timeout):
                raise CaptchaError("等待验证码超时，请重新发起领取")
        finally:
            self.httpd.shutdown()
            self.httpd.server_close()
        if not self.param:
            raise CaptchaError("验证码参数为空，请重新发起领取")
        return self.param


def request(timeout: int = TIMEOUT) -> tuple[str, str]:
    """打开浏览器过验证，返回 (验证码参数, region)。"""
    cfg = config()
    if not cfg.get("enabled") or not cfg.get("scene_id"):
        raise CaptchaError("上游没有开启验证码，无法领取")
    return _Server(cfg).wait(timeout), str(cfg.get("region") or "")
