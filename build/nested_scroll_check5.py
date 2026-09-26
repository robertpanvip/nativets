"""Nested-scroll regression v5 — fully log-driven, deterministic.

The app reports `scroll` events for BOTH scrollers (the main ScrollArea got
a probe onScroll). The host logs each as `[host] ev scroll id=<n>`.

Layout facts (1180x760 window, dpr=1 — verified stable across runs):
  - one wheel notch = 69px for both scrollers
  - OUTER: viewport 584, content 1509, max 925
  - INNER: viewport 168, content 506, max 338
  - the inner card is at client y=352 exactly when outer top=828
  - at outer top=0 the card sits at y≈1180 — off-screen (window is 760)

Choreography (each step's side effects are deliberate):
  1. wheel(590,200,+8)          → discover OUTER id from the log
  2. wheel(590,200,-30)         → reset outer to 0 (over-reset clamps)
  3. probe downward from y=280  → find INNER id; probe ticks scroll the
     outer 0→828 (that's what moves the card up to the probe) and leave
     the inner at ~138-207
  4. wheel(590,352,-20)         → reset inner to 0; the up-ticks past the
     top chain to the outer and pull it 828→0 (clamped)
  5. wheel(590,200,+12)         → outer back to exactly 828 (card at 352);
     cursor is over the outer panel, the inner stays at 0
  6. THE regression: wheel(590,352,+3) → inner must take all 3 ticks
     (0→207, well inside its 338 max): assert inner events>0, outer==0
  7. overshoot: wheel(590,352,+60) → inner hits its bottom edge and the
     remaining ticks chain to the outer: assert outer events>0

Usage: python build/nested_scroll_check5.py
"""

import ctypes
import re
import subprocess
import sys
import time
from collections import Counter
from ctypes import wintypes

TITLE = "PerryTS × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\release\gpui-perryts-host.exe"
LOG = r"E:\AI-workspace\gpui-perryts\build\ns5.log"
MOUSEEVENTF_WHEEL = 0x0800
WHEEL_DELTA = 120
SCROLL_RE = re.compile(r"ev scroll id=(\d+)")
OUTER_X = 590
INNER_X = 590
INNER_Y = 352

u = ctypes.windll.user32
u.SetProcessDPIAware()


def wheel(hwnd, x, y, notches, delay=0.03):
    u.SetForegroundWindow(hwnd)
    u.SetCursorPos(x, y)
    time.sleep(0.12)
    for _ in range(abs(notches)):
        u.mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -WHEEL_DELTA if notches > 0 else WHEEL_DELTA, 0)
        time.sleep(delay)
    time.sleep(0.4)


def counts_since(base):
    with open(LOG, encoding="utf-8", errors="replace") as f:
        text = f.read()
    c = Counter()
    for m in SCROLL_RE.finditer(text, base):
        c[int(m.group(1))] += 1
    return c, len(text)


def last_top_for(log_text, target_id):
    """Last reported CSS top for a scroller id, or None."""
    tops = re.findall(
        r"ev scroll id=%d .*?\"top\":(-?[\d.]+)" % target_id, log_text
    )
    return float(tops[-1]) if tops else None


def read_log():
    with open(LOG, encoding="utf-8", errors="replace") as f:
        return f.read()


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

        # 1. OUTER id.
        base = 0
        c, base = counts_since(base)
        wheel(hwnd, ox + OUTER_X, oy + 200, 8)
        c1, base = counts_since(base)
        if not c1:
            print("FAIL: outer scroller never reported (probe onScroll broken?)")
            return 1
        outer_id = c1.most_common(1)[0][0]
        print(f"outer id = {outer_id} (events: {sum(c1.values())})")

        # 2. reset OUTER to top.
        wheel(hwnd, ox + OUTER_X, oy + 200, -30, delay=0.012)
        c, base = counts_since(base)

        # 3. find INNER (probe drifts outer to 828, card rises to meet it).
        inner_id = None
        for py in range(280, 660, 12):
            wheel(hwnd, ox + OUTER_X, oy + py, 2)
            c, base = counts_since(base)
            others = {i: n for i, n in c.items() if i != outer_id}
            if others:
                inner_id = next(iter(others))
                break
        if inner_id is None:
            print("FAIL: inner scroller not found")
            return 1
        print(f"inner id = {inner_id}")

        # 4. reset INNER to 0 (chains pull outer 828→0 — expected).
        wheel(hwnd, ox + INNER_X, oy + INNER_Y, -20, delay=0.012)
        c, base = counts_since(base)

        # 5. outer to exactly 828 (12 notches × 69px); inner stays at 0.
        wheel(hwnd, ox + OUTER_X, oy + 200, 12)
        c, base = counts_since(base)
        text = read_log()
        outer_top = last_top_for(text, outer_id)
        inner_top = last_top_for(text, inner_id)
        print(f"pre-assert: outer top={outer_top}  inner top={inner_top}")
        if inner_top is None or abs(inner_top) > 1.0:
            print("FAIL: inner not at top before the regression wheel")
            return 1

        # 6. THE regression: 3 notches from top must stay inside the inner.
        wheel(hwnd, ox + INNER_X, oy + INNER_Y, 3)
        c2, base = counts_since(base)
        leaked = c2.get(outer_id, 0)
        inner_moved = c2.get(inner_id, 0)
        print(f"phase2: inner events={inner_moved}  outer leaked events={leaked}")

        # 7. overshoot → chaining allowed.
        wheel(hwnd, ox + INNER_X, oy + INNER_Y, 60, delay=0.012)
        c3, base = counts_since(base)
        chained = c3.get(outer_id, 0)
        print(f"phase3: outer chained after overshoot={chained} (expected > 0)")

        ok = leaked == 0 and inner_moved > 0 and chained > 0
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
