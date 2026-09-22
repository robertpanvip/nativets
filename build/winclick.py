"""Focus the GPUI window, then click at a *client-area-relative* coordinate.

Screen coordinates are fragile here: the window moves and gets occluded, and
`desktop_ctrl.py move` can steal focus. Resolving (relX, relY) against the live
client origin every time keeps clicks honest.

    python build/winclick.py <relX> <relY> [title]
"""

import ctypes
import sys
import time
from ctypes import wintypes

import pyautogui

TITLE = sys.argv[3] if len(sys.argv) > 3 else "PerryTS \u00d7 GPUI"
user32 = ctypes.windll.user32

hwnd = user32.FindWindowW(None, TITLE)
if not hwnd:
    raise SystemExit("window not found: %r" % TITLE)

user32.ShowWindow(hwnd, 9)  # SW_RESTORE
user32.SetForegroundWindow(hwnd)
time.sleep(0.4)

origin = wintypes.POINT(0, 0)
user32.ClientToScreen(hwnd, ctypes.byref(origin))
rect = wintypes.RECT()
user32.GetClientRect(hwnd, ctypes.byref(rect))

rel_x, rel_y = int(sys.argv[1]), int(sys.argv[2])
if not (0 <= rel_x < rect.right and 0 <= rel_y < rect.bottom):
    raise SystemExit(
        "rel (%d,%d) outside client %dx%d" % (rel_x, rel_y, rect.right, rect.bottom)
    )

pyautogui.click(origin.x + rel_x, origin.y + rel_y)
print(
    "clicked rel=(%d,%d) screen=(%d,%d) client=%dx%d"
    % (rel_x, rel_y, origin.x + rel_x, origin.y + rel_y, rect.right, rect.bottom)
)
