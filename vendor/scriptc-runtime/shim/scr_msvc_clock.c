/* scr_msvc_clock.c — exported clock_gettime/nanosleep for MSVC builds.
 *
 * scr_win.c carries the Win32 implementations but its exported spellings
 * (clock_gettime32/64) target MinGW's pthread-era symbols. For the MSVC
 * lane we export the POSIX names directly with the UCRT C11 timespec, and
 * route to MinGW's clockid_t numbers via the compat header. This file
 * reuses scr_win.c's static helpers is impossible across TUs, so this is a
 * compact reimplementation over the same Win32 primitives
 * (GetSystemTimePreciseAsFileTime / QueryPerformanceCounter /
 * waitable timers) with identical semantics. */
#include "scr_msvc_compat.h"

#include <stdint.h>
#include <windows.h>

#ifndef CREATE_WAITABLE_TIMER_HIGH_RESOLUTION
#define CREATE_WAITABLE_TIMER_HIGH_RESOLUTION 0x00000002
#endif

#define EPOCH_DIFF UINT64_C(116444736000000000)
#define TICKS_PER_SEC UINT64_C(10000000)

static int msvc_gettime(clockid_t clock_id, int64_t *sec, long *nsec) {
  if (clock_id == CLOCK_REALTIME || clock_id == CLOCK_REALTIME_COARSE) {
    FILETIME ft;
    ULARGE_INTEGER t;
    if (clock_id == CLOCK_REALTIME) {
      GetSystemTimePreciseAsFileTime(&ft);
    } else {
      GetSystemTimeAsFileTime(&ft);
    }
    t.LowPart = ft.dwLowDateTime;
    t.HighPart = ft.dwHighDateTime;
    if (t.QuadPart < EPOCH_DIFF) {
      errno = EOVERFLOW;
      return -1;
    }
    t.QuadPart -= EPOCH_DIFF;
    *sec = (int64_t)(t.QuadPart / TICKS_PER_SEC);
    *nsec = (long)((t.QuadPart % TICKS_PER_SEC) * 100);
    return 0;
  }
  if (clock_id == CLOCK_MONOTONIC) {
    static LARGE_INTEGER freq = {0};
    LARGE_INTEGER counter;
    if (freq.QuadPart == 0 && !QueryPerformanceFrequency(&freq)) {
      errno = ENOTSUP;
      return -1;
    }
    if (!QueryPerformanceCounter(&counter)) {
      errno = EIO;
      return -1;
    }
    *sec = (int64_t)(counter.QuadPart / freq.QuadPart);
    *nsec = (long)(((counter.QuadPart % freq.QuadPart) * 1000000000LL) / freq.QuadPart);
    return 0;
  }
  errno = EINVAL;
  return -1;
}

int clock_gettime(clockid_t clock_id, struct timespec *tp) {
  if (tp == NULL) {
    errno = EFAULT;
    return -1;
  }
  int64_t sec;
  long nsec;
  if (msvc_gettime(clock_id, &sec, &nsec) != 0) return -1;
  tp->tv_sec = (time_t)sec;
  tp->tv_nsec = nsec;
  return 0;
}

int nanosleep(const struct timespec *request, struct timespec *remain) {
  if (request == NULL || request->tv_nsec < 0 || request->tv_nsec >= 1000000000L) {
    errno = EINVAL;
    return -1;
  }
  HANDLE timer = CreateWaitableTimerExW(NULL, NULL,
                                        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                                        TIMER_ALL_ACCESS);
  if (!timer) timer = CreateWaitableTimerW(NULL, TRUE, NULL);
  if (!timer) {
    errno = EIO;
    return -1;
  }
  LARGE_INTEGER due;
  /* positive = relative 100ns units; negative signals relative */
  LONGLONG ticks100ns = (LONGLONG)request->tv_sec * 10000000LL +
                        (LONGLONG)request->tv_nsec / 100;
  due.QuadPart = -ticks100ns;
  int rc = 0;
  if (!SetWaitableTimer(timer, &due, 0, NULL, NULL, FALSE)) {
    errno = EIO;
    rc = -1;
  } else if (WaitForSingleObject(timer, INFINITE) != WAIT_OBJECT_0) {
    errno = EINTR;
    rc = -1;
  }
  CloseHandle(timer);
  if (remain != NULL) {
    remain->tv_sec = 0;
    remain->tv_nsec = 0;
  }
  return rc;
}
