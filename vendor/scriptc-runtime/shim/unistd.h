/* unistd.h for MSVC builds of the scriptc runtime.
 * Only the names the runtime's Win32 arm actually consumes; everything
 * maps to UCRT's underscore-prefixed spellings (see scr_msvc_compat.h). */
#ifndef SCR_MSVC_UNISTD_FAKE
#define SCR_MSVC_UNISTD_FAKE

#include "scr_msvc_compat.h"

/* POSIX stdio/pipe helpers some TUs call */
#include <stdio.h>
#define popen  _popen
#define pclose _pclose

/* Path maximal from POSIX */
#ifndef PATH_MAX
#define PATH_MAX 260
#endif

/* symlinks don't exist as POSIX links here */
#ifndef ssize_t
typedef intptr_t ssize_t;
#endif

#endif /* SCR_MSVC_UNISTD_FAKE */
