"""Event delegation E2E — click bubbling + stopPropagation.

Steps:
  1. launch, screenshot to locate the delegation card
  2. (manual coordinates after visual inspection in this first run)

Success criteria (from the host log's console.info lines):
  - clicking the BUBBLE child logs: child line, then parent line with
    currentTarget != target (delegation worked)
  - clicking the STOP child logs ONLY the child line (propagation halted)
"""
import ctypes
import subprocess
import sys
import time
from ctypes import wintypes

TITLE = "nativets × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\release\nativets-host.exe"
LOG = r"E:\AI-workspace\gpui-perryts\build\deleg.log"
SHOT = r"E:\AI-workspace\gpui-perryts\build\deleg-full.png"

u = ctypes.windll.user32
u.SetProcessDPIAware()


def click(x, y):
    u.SetForegroundWindow(hwnd)
    u.SetCursorPos(x, y)
    time.sleep(0.15)
    u.mouse_event(0x0002, 0, 0, 0, 0)  # left down
    time.sleep(0.05)
    u.mouse_event(0x0004, 0, 0, 0, 0)  # left up
    time.sleep(0.4)


def shot(path):
    from PIL import ImageGrab
    r = wintypes.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom)).save(path)


logf = open(LOG, "w", encoding="utf-8", errors="replace")
proc = subprocess.Popen([EXE], stdout=logf, stderr=subprocess.STDOUT)
try:
    hwnd = 0
    for _ in range(50):
        hwnd = u.FindWindowW(None, TITLE)
        if hwnd:
            break
        time.sleep(0.1)
    if not hwnd:
        raise SystemExit("window never appeared")
    time.sleep(1.5)
    shot(SHOT)
    print("screenshot:", SHOT)
finally:
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except Exception:
        proc.kill()
    logf.close()
