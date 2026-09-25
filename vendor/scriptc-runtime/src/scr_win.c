/* win32 libc shims — the POSIX/BSD functions the runtime calls that
 * mingw-w64's CRT does not provide (declared in scr_runtime.h's _WIN32
 * block). Compiled into win32-target builds only (native-toolchain.ts adds this TU and
 * -ladvapi32 for windows triples); an empty TU anywhere else, so POSIX
 * builds cannot change by a byte. */
#ifdef _WIN32

#include "scr_runtime.h"

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <windows.h>

#ifndef CREATE_WAITABLE_TIMER_HIGH_RESOLUTION
#define CREATE_WAITABLE_TIMER_HIGH_RESOLUTION 0x00000002
#endif

#define SCR_WIN_FILETIME_UNIX_EPOCH UINT64_C(116444736000000000)
#define SCR_WIN_TICKS_PER_SECOND UINT64_C(10000000)

static int scr_win_error(DWORD error) {
  switch (error) {
    case ERROR_INVALID_PARAMETER:
      errno = EINVAL;
      break;
    case ERROR_NOT_ENOUGH_MEMORY:
    case ERROR_OUTOFMEMORY:
      errno = ENOMEM;
      break;
    default:
      errno = EIO;
      break;
  }
  return -1;
}

static int scr_win_clock_gettime(clockid_t clock_id, int64_t *seconds,
                                 long *nanoseconds) {
  if (clock_id == CLOCK_REALTIME || clock_id == CLOCK_REALTIME_COARSE) {
    FILETIME file_time;
    ULARGE_INTEGER ticks;
    GetSystemTimePreciseAsFileTime(&file_time);
    ticks.LowPart = file_time.dwLowDateTime;
    ticks.HighPart = file_time.dwHighDateTime;
    if (ticks.QuadPart < SCR_WIN_FILETIME_UNIX_EPOCH) {
      errno = EOVERFLOW;
      return -1;
    }
    ticks.QuadPart -= SCR_WIN_FILETIME_UNIX_EPOCH;
    *seconds = (int64_t)(ticks.QuadPart / SCR_WIN_TICKS_PER_SECOND);
    *nanoseconds = (long)((ticks.QuadPart % SCR_WIN_TICKS_PER_SECOND) * 100);
    return 0;
  }
  if (clock_id == CLOCK_MONOTONIC) {
    LARGE_INTEGER counter;
    LARGE_INTEGER frequency;
    if (!QueryPerformanceFrequency(&frequency) || frequency.QuadPart <= 0 ||
        !QueryPerformanceCounter(&counter) || counter.QuadPart < 0) {
      return scr_win_error(GetLastError());
    }
    *seconds = counter.QuadPart / frequency.QuadPart;
    *nanoseconds = (long)(((uint64_t)(counter.QuadPart % frequency.QuadPart) *
                             UINT64_C(1000000000)) /
                            (uint64_t)frequency.QuadPart);
    return 0;
  }
  errno = EINVAL;
  return -1;
}

static int scr_win_nanosleep(int64_t seconds, long nanoseconds,
                             int64_t *remaining_seconds,
                             long *remaining_nanoseconds) {
  if (seconds < 0 || nanoseconds < 0 || nanoseconds >= 1000000000L) {
    errno = EINVAL;
    return -1;
  }
  uint64_t subsecond_ticks = ((uint64_t)nanoseconds + 99) / 100;
  if ((uint64_t)seconds >
      ((uint64_t)INT64_MAX - subsecond_ticks) / SCR_WIN_TICKS_PER_SECOND) {
    errno = EINVAL;
    return -1;
  }
  uint64_t ticks =
      (uint64_t)seconds * SCR_WIN_TICKS_PER_SECOND + subsecond_ticks;
  if (ticks == 0) {
    Sleep(0);
  } else {
    HANDLE timer = CreateWaitableTimerExW(
        NULL, NULL, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
        TIMER_MODIFY_STATE | SYNCHRONIZE);
    if (timer == NULL) timer = CreateWaitableTimerW(NULL, FALSE, NULL);
    if (timer == NULL) return scr_win_error(GetLastError());
    LARGE_INTEGER due;
    due.QuadPart = -(LONGLONG)ticks;
    if (!SetWaitableTimer(timer, &due, 0, NULL, NULL, FALSE)) {
      DWORD error = GetLastError();
      CloseHandle(timer);
      return scr_win_error(error);
    }
    DWORD wait = WaitForSingleObject(timer, INFINITE);
    DWORD error = wait == WAIT_OBJECT_0 ? ERROR_SUCCESS : GetLastError();
    CloseHandle(timer);
    if (wait != WAIT_OBJECT_0) return scr_win_error(error);
  }
  if (remaining_seconds != NULL) *remaining_seconds = 0;
  if (remaining_nanoseconds != NULL) *remaining_nanoseconds = 0;
  return 0;
}

int clock_gettime32(clockid_t clock_id, struct _timespec32 *tp) {
  int64_t seconds;
  long nanoseconds;
  if (scr_win_clock_gettime(clock_id, &seconds, &nanoseconds) != 0) return -1;
  if (seconds < INT32_MIN || seconds > INT32_MAX) {
    errno = EOVERFLOW;
    return -1;
  }
  tp->tv_sec = (__time32_t)seconds;
  tp->tv_nsec = nanoseconds;
  return 0;
}

int clock_gettime64(clockid_t clock_id, struct _timespec64 *tp) {
  int64_t seconds;
  long nanoseconds;
  if (scr_win_clock_gettime(clock_id, &seconds, &nanoseconds) != 0) return -1;
  tp->tv_sec = (__time64_t)seconds;
  tp->tv_nsec = nanoseconds;
  return 0;
}

int nanosleep32(const struct _timespec32 *request, struct _timespec32 *remain) {
  int64_t remaining_seconds;
  long remaining_nanoseconds;
  int result = scr_win_nanosleep(
      request->tv_sec, request->tv_nsec,
      remain == NULL ? NULL : &remaining_seconds,
      remain == NULL ? NULL : &remaining_nanoseconds);
  if (result == 0 && remain != NULL) {
    remain->tv_sec = (__time32_t)remaining_seconds;
    remain->tv_nsec = remaining_nanoseconds;
  }
  return result;
}

int nanosleep64(const struct _timespec64 *request, struct _timespec64 *remain) {
  int64_t remaining_seconds;
  long remaining_nanoseconds;
  int result = scr_win_nanosleep(
      request->tv_sec, request->tv_nsec,
      remain == NULL ? NULL : &remaining_seconds,
      remain == NULL ? NULL : &remaining_nanoseconds);
  if (result == 0 && remain != NULL) {
    remain->tv_sec = (__time64_t)remaining_seconds;
    remain->tv_nsec = remaining_nanoseconds;
  }
  return result;
}

/* POSIX.1-2008 stpcpy: strcpy returning the END of the copy — scr_number.c
 * (untouchable by project rule; ryu-adjacent) builds "e+"/"e-" exponent
 * tails with it. */
char *stpcpy(char *dst, const char *src) {
  size_t n = strlen(src);
  memcpy(dst, src, n + 1);
  return dst + n;
}

/* arc4random_buf over RtlGenRandom (advapi32's SystemFunction036) — the
 * kernel CSPRNG behind rand_s and BCryptGenRandom, available everywhere
 * without a bcrypt link. Failure aborts: like the BSD/glibc original, the
 * callers (Math.random, crypto.randomUUID/randomBytes) have no error arm
 * and returning predictable bytes is never acceptable. */
BOOLEAN NTAPI SystemFunction036(PVOID buffer, ULONG length);

void arc4random_buf(void *buf, size_t n) {
  unsigned char *p = buf;
  while (n > 0) {
    ULONG step = n > 0x7fffffffUL ? 0x7fffffffUL : (ULONG)n;
    if (!SystemFunction036(p, step)) {
      fputs("scriptc: RtlGenRandom failed\n", stderr);
      abort();
    }
    p += step;
    n -= step;
  }
}

/* POSIX gmtime_r: the win32 CRT's gmtime already answers from per-thread
 * storage, so the reentrant spelling is a copy-out (scr_http.c's Date
 * header formatter is the caller). */
struct tm *gmtime_r(const time_t *t, struct tm *out) {
  struct tm *g = gmtime(t);
  if (g == NULL) return NULL;
  *out = *g;
  return out;
}

/* GNU/BSD strcasestr — scr_http.c's Connection-token scan. Naive
 * quadratic scan like the musl original; header values are tiny. */
char *strcasestr(const char *hay, const char *needle) {
  size_t n = strlen(needle);
  if (n == 0) return (char *)hay;
  for (; *hay != '\0'; hay++) {
    size_t i = 0;
    while (i < n && hay[i] != '\0' &&
           tolower((unsigned char)hay[i]) == tolower((unsigned char)needle[i]))
      i++;
    if (i == n) return (char *)hay;
  }
  return NULL;
}

#else /* !_WIN32 */

/* Empty TU off-Windows: native-toolchain.ts only compiles this file for windows triples,
 * but an accidental link elsewhere must stay harmless. */
typedef int scr_win_unused;

#endif /* _WIN32 */
