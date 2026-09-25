/* scr_msvc_compat.h — MSVC/UCRT compat shim for the scriptc runtime.
 *
 * Scope: the exact set of POSIX/BSD names the runtime's Win32 arm expects
 * from a MinGW-style CRT that UCRT does not ship. Each mapping stays at
 * the narrowest spelling that compiles the vendored sources unmodified
 * (the sources keep their POSIX call sites; we retarget the names).
 */
#ifndef SCR_MSVC_COMPAT_H
#define SCR_MSVC_COMPAT_H

#ifndef _MSC_VER
#error "this shim is MSVC-only; MinGW builds use the real POSIX headers"
#endif

#include <direct.h>   /* _getcwd, _mkdir */
#include <io.h>       /* _isatty, _access, _open, _read, _write, _close */
#include <process.h>  /* _getpid */
#include <errno.h>
/* ── unistd.h replacements ──────────────────────────────────────────── */
#define getcwd(buf, size) _getcwd((buf), (size))
#define isatty(fd) _isatty(fd)
#define access(path, mode) _access((path), (mode))
#define getpid _getpid
#define rmdir _rmdir
#define unlink _unlink
#define dup _dup
#define dup2 _dup2
#define read _read
#define write _write
#define close _close
#define lseek _lseek

/* access()' POSIX mode bits live in unistd.h; UCRT spells them in io.h
 * with an underscore prefix. Map the spellings the sources use. */
#ifndef F_OK
#define F_OK 0
#endif
#ifndef X_OK
#define X_OK 1 /* Windows has no execute bit; treat as a read check */
#endif
#ifndef W_OK
#define W_OK 2
#endif
#ifndef R_OK
#define R_OK 4
#endif

/* stdio POSIX additions (fileno is standard C11 again but keep the guard) */
#if !defined(fileno)
#define fileno _fileno
#endif

/* ssize_t: pointer-sized signed count (scr_runtime.h also defines it under
 * _MSC_VER; keep both definitions consistent). */
#include <basetsd.h>
typedef SSIZE_T ssize_t;

/* ── sys/stat.h gaps ────────────────────────────────────────────────── */
#include <sys/stat.h>
typedef unsigned int mode_t;
#ifndef S_ISDIR
#define S_ISDIR(m) (((m) & _S_IFMT) == _S_IFDIR)
#endif
#ifndef S_ISREG
#define S_ISREG(m) (((m) & _S_IFMT) == _S_IFREG)
#endif
#ifndef S_ISLNK
#define S_ISLNK(m) (0) /* UCRT stat never reports links */
#endif
#ifndef S_ISFIFO
#define S_ISFIFO(m) (((m) & _S_IFMT) == _S_IFIFO)
#endif
#ifndef S_ISCHR
#define S_ISCHR(m) (((m) & _S_IFMT) == _S_IFCHR)
#endif
#ifndef S_ISBLK
#define S_ISBLK(m) (0) /* no block-device type in the CRT's stat (no _S_IFBLK) */
#endif
#ifndef S_ISSOCK
#define S_ISSOCK(m) (0) /* no socket bits in the CRT's stat */
#endif

/* ── fcntl.h gaps ───────────────────────────────────────────────────── */
#include <fcntl.h>
#ifndef O_BINARY
#define O_BINARY _O_BINARY
#endif
#ifndef O_TEXT
#define O_TEXT _O_TEXT
#endif
#ifndef O_APPEND
#define O_APPEND _O_APPEND
#endif
#ifndef O_CREAT
#define O_CREAT _O_CREAT
#endif
#ifndef O_TRUNC
#define O_TRUNC _O_TRUNC
#endif
#ifndef O_EXCL
#define O_EXCL _O_EXCL
#endif
#ifndef O_RDONLY
#define O_RDONLY _O_RDONLY
#endif
#ifndef O_WRONLY
#define O_WRONLY _O_WRONLY
#endif
#ifndef O_RDWR
#define O_RDWR _O_RDWR
#endif
/* MinGW's O_* spelled the sync flags; MSVC has none — degrade like the
 * runtime's own O_SYNC shim in scr_lib.c. */
#ifndef O_SYNC
#define O_SYNC 0
#endif
#ifndef O_DIRECTORY
#define O_DIRECTORY 0
#endif
#ifndef O_CLOEXEC
#define O_CLOEXEC 0
#endif
#ifndef O_NOFOLLOW
#define O_NOFOLLOW 0
#endif

/* ── clock identifiers ────────────────────────────────────────────────
 * scr_win.c implements clock_gettime/nanosleep over Win32 primitives for
 * the MinGW arm; expose the same names to MSVC builds so call sites in
 * scr_bytes_io.c / scr_async.c / scr_lib.c compile and link against the
 * scr_win.c implementations. */
#ifndef CLOCK_REALTIME
#define CLOCK_REALTIME 0
#endif
#ifndef CLOCK_MONOTONIC
#define CLOCK_MONOTONIC 1
#endif
#ifndef CLOCK_REALTIME_COARSE
#define CLOCK_REALTIME_COARSE 2
#endif
#ifndef CLOCK_PROCESS_CPUTIME_ID
#define CLOCK_PROCESS_CPUTIME_ID 3
#endif
#ifndef CLOCK_THREAD_CPUTIME_ID
#define CLOCK_THREAD_CPUTIME_ID 4
#endif

/* The clockid_t + timespec shape scr_win.c's shims accept. MinGW's
 * pthread_time.h spells these; UCRT's timespec_get uses struct timespec
 * (C11) with a different clock model, so define the MinGW-compatible
 * view here and let scr_win.c's exported shims serve the calls. */
#ifndef SCR_MSVC_CLOCKID_T_DEFINED
#define SCR_MSVC_CLOCKID_T_DEFINED
typedef int clockid_t;
#endif

/* C11 struct timespec exists on UCRT (time.h) when _C11 has it; make the
 * POSIX spellings resolve. UCRT does provide struct timespec since VS2015. */
#include <time.h>

/* ── Winsock fd discipline ────────────────────────────────────────────
 * scr_net.c / scr_dgram.c already gate Winsock under _WIN32; nothing to
 * shim here beyond ensuring winsock2 inclusion order stays their job. */

/* ── GNU __attribute__ ────────────────────────────────────────────────
 * The library-mode TU (scr_library.c) — linked ONLY into library archives,
 * so the upstream MSVC work never had to compile it — annotates the trap
 * funnels with __attribute__((noinline)). MSVC has no __attribute__; the
 * annotation is an inlining hint and the C11 _Noreturn keyword on the same
 * declarations carries the semantic part, so strip it. */
#ifndef __GNUC__
#define __attribute__(x)
#endif

#endif /* SCR_MSVC_COMPAT_H */
