"""Smoke test: launch the DEBUG exe (QUICKJS_EMBED), screenshot, exit.

Reuses form_shot's launch logic but points at the debug binary so we can verify
a debug build renders without panicking. Screenshot saved to build/smoke.png.
"""
import ctypes
import os
import sys
import time
from ctypes import wintypes

TITLE = "nativets × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\debug\gpui-perryts-host.exe"
OUT = r"E:\AI-workspace\gpui-perryts\build\smoke.png"

u = ctypes.windll.user32
u.SetProcessDPIAware()


def shot(hwnd, path):
    from PIL import ImageGrab
    r = wintypes.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom))
    img.save(path)
    print("shot ->", path, img.size)
    return img


def main():
    log = open(r"E:\AI-workspace\gpui-perryts\build\smoke.log", "w", encoding="utf-8")
    env = dict(os.environ)
    env["QUICKJS_EMBED"] = "1"
    proc = __import__("subprocess").Popen(
        [EXE], stdout=log, stderr=__import__("subprocess").STDOUT, env=env
    )
    try:
        hwnd = 0
        for _ in range(80):
            hwnd = u.FindWindowW(None, TITLE)
            if hwnd:
                break
            time.sleep(0.1)
        if not hwnd:
            raise SystemExit("window never appeared")
        time.sleep(2.2)
        shot(hwnd, OUT)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        log.close()


if __name__ == "__main__":
    main()
