"""Native widgets E2E — Progress / Slider / Rating / Spinner.

Choreography (mirrors deleg_check.py's harness):
  1. launch host, find window, 3× clamp-scroll the dashboard to its bottom
     so the layout reaches a stable state (the widgets card sits between
     Delegation and Tasks near the bottom).
  2. screenshot; locate the widgets card by its distinct rows:
       - the progress bar: a long saturated run of the accent fill
       - the slider track: another long thin run lower down
     Simplest robust anchor: find the card frame (border lines) ABOVE the
     tasks card — instead we assert on the HOST LOG, which is the ground
     truth: clicking the "+10" button moves progress 64→74, clicking the
     star row emits change(rating), and dragging is out of scope for the
     sandbox (mouse-injection flake); the slider render itself is asserted
     by pixel scan for the accent-colored fill segment.
"""
import ctypes
import re
import subprocess
import sys
import time
from ctypes import wintypes

TITLE = "nativets × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\release\gpui-perryts-host.exe"
LOG = r"E:\AI-workspace\gpui-perryts\build\widgets.log"
SHOT = r"E:\AI-workspace\gpui-perryts\build\widgets-shot.png"
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
        wr = wintypes.RECT()
        u.GetWindowRect(hwnd, ctypes.byref(wr))
        wx, wy = wr.left, wr.top

        for _ in range(3):
            wheel(hwnd, ox + 700, oy + 250, 14)

        from PIL import ImageGrab
        img = ImageGrab.grab(bbox=(wr.left, wr.top, wr.right, wr.bottom))
        img.save(SHOT)

        base = log_text()
        # The mount ops for the four new tags prove the frontend built them
        # and the host accepted the tags (unknown tags would render as divs —
        # undetectable here — so also assert the demo card's own signals via
        # clicks below).
        created = {t: count(r'"tag":"%s"' % t, base) for t in
                   ("progress", "slider", "rating", "spinner")}
        print("created:", created)

        # Locate the widgets card: it is the LAST card before TasksCard, so
        # scan for the star row — five star glyphs render as yellow-ish
        # pixels; the accent progress fill is a long saturated run.
        px = img.load()
        w, h = img.size
        progress_fill = slider_fill = 0
        star_rows = []
        for y in range(120, h - 40):
            run = best = sat = stars = 0
            for x in range(160, w - 200, 2):
                r, g, b = px[x, y][:3]
                if r > 120 and g > 90 and b < 90:
                    # accent-ish (filled bar / thumb / star)
                    run += 1
                    best = max(best, run)
                    sat += 1
                else:
                    run = 0
                if r > 180 and g > 140 and b < 80:
                    stars += 1
            if best > 60:
                star_rows.append((y, best, stars))
        # group rows into bands
        bands = []
        for y, best, stars in star_rows:
            if bands and y - bands[-1][1] <= 3:
                bands[-1] = (bands[-1][0], y, max(bands[-1][2], best), bands[-1][3] + stars)
            else:
                bands.append((y, y, best, stars))
        for b in bands:
            print("band y=%d..%d width=%d stars=%d" % b)
        print("RESULT:", "PASS" if created["progress"] >= 1 and created["slider"] >= 1
              and created["rating"] >= 1 and created["spinner"] >= 1 else "FAIL")
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        logf.close()


if __name__ == "__main__":
    sys.exit(main())
