"""Event delegation E2E — click bubbling + stopPropagation.

Deterministic-enough choreography:
  1. wheel 14×3 over the stats-cards row → outer panel clamps to its bottom;
     anything else that scrolled also clamps to ITS bottom, so the layout
     reaches a stable state regardless of which scroller ate which tick.
  2. screenshot; find ALL long horizontal border lines; pair consecutive
     lines into boxes; the delegation card's two rows are the two LAST boxes
     with height 20..50px (log rows inside the card have no full-width
     borders; the card frame itself is much taller than 50px).
  3. click each row's center; assert from the host log:
       bubble row → child handler + parent (delegated) handler
       stop row   → child handler only
"""
import ctypes
import re
import subprocess
import sys
import time
from ctypes import wintypes

TITLE = "PerryTS × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\release\gpui-perryts-host.exe"
LOG = r"E:\AI-workspace\gpui-perryts\build\deleg.log"
SHOT = r"E:\AI-workspace\gpui-perryts\build\deleg-shot.png"
MOUSEEVENTF_WHEEL = 0x0800

u = ctypes.windll.user32
u.SetProcessDPIAware()


def wheel(hwnd, x, y, notches, delay=0.03):
    u.SetForegroundWindow(hwnd)
    u.SetCursorPos(x, y)
    time.sleep(0.12)
    for _ in range(abs(notches)):
        u.mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -120 if notches > 0 else 120, 0)
        time.sleep(delay)
    time.sleep(0.4)


def click(hwnd, x, y):
    u.SetForegroundWindow(hwnd)
    u.SetCursorPos(x, y)
    time.sleep(0.15)
    u.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.05)
    u.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.4)


def log_text():
    with open(LOG, encoding="utf-8", errors="replace") as f:
        return f.read()


def count(re_s, text):
    return len(re.findall(re_s, text))


def find_rows(hwnd):
    """Locate the two delegation rows. Returns (bubble_y, stop_y) or None."""
    from PIL import ImageGrab
    r = wintypes.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom))
    img.save(SHOT)
    px = img.load()
    w, h = img.size

    def borderish(c):
        return abs(c[0] - 38) <= 10 and abs(c[1] - 45) <= 10 and abs(c[2] - 61) <= 10

    ys = []
    for y in range(100, h - 30):
        run = best = 0
        for x in range(200, w - 160, 2):
            if borderish(px[x, y][:3]):
                run += 1
                best = max(best, run)
            else:
                run = 0
        if best > 350:
            ys.append(y)
    # pair consecutive border lines into boxes; a REAL row has a dark
    # interior (15,17,23) at its center — the gap between two rows pairs
    # geometrically but its interior is C.card (22,27,38), so it's dropped.
    boxes = []
    for a, b in zip(ys, ys[1:]):
        if 20 <= b - a <= 50 and px[600, (a + b) // 2][:3] == (15, 17, 23):
            boxes.append((a, b))
    if len(boxes) < 2:
        return None
    # After the 3-pass clamp the delegation card is the TOP-most card, so its
    # two rows are the FIRST two boxes on screen (TasksCard's rows sit lower;
    # the tasks card frame/“快速添加” button are taller than 50px or not
    # bordered like the rows).
    b1, b2 = boxes[0], boxes[1]
    return (b1[0] + b1[1]) // 2, (b2[0] + b2[1]) // 2


def main():
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

        pt = wintypes.POINT(0, 0)
        u.ClientToScreen(hwnd, ctypes.byref(pt))
        ox, oy = pt.x, pt.y
        # Window-rect origin (screenshots are taken against THIS frame):
        wr = wintypes.RECT()
        u.GetWindowRect(hwnd, ctypes.byref(wr))
        wx, wy = wr.left, wr.top

        for _ in range(3):
            wheel(hwnd, ox + 700, oy + 250, 14)

        rows = find_rows(hwnd)
        if not rows:
            print("FAIL: delegation rows not located")
            return 1
        bubble_y, stop_y = rows
        print(f"rows: bubble y={bubble_y}  stop y={stop_y}")

        base = log_text()
        # Click in WINDOW-rect coordinates to match the screenshot frame.
        click(hwnd, wx + 600, wy + bubble_y)
        mid = log_text()
        cb = count(r"冒泡行子块点击", mid) - count(r"冒泡行子块点击", base)
        pb = count(r"父级收到", mid) - count(r"父级收到", base)

        click(hwnd, wx + 600, wy + stop_y)
        end = log_text()
        cs = count(r"拦截行子块点击", end) - count(r"拦截行子块点击", mid)
        ps = count(r"父级收到", end) - count(r"父级收到", mid)

        print(f"bubble row: child={cb} parent={pb} (want 1 / 1)")
        print(f"stop row:   child={cs} parent={ps} (want 1 / 0)")
        ok = cb == 1 and pb == 1 and cs == 1 and ps == 0
        print("RESULT:", "PASS" if ok else "FAIL")
        return 0 if ok else 1
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        logf.close()


if __name__ == "__main__":
    sys.exit(main())
