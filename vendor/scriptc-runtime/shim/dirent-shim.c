/* dirent shim implementation — FindFirstFileA-based (the runtime's sync fs
 * arm is byte/ANSI in this area; names longer than the buffer trap loudly
 * through the runtime's own error paths, same as MinGW's dirent). */
#include "dirent-shim.h"

#ifndef _CRT_SECURE_NO_WARNINGS
#define _CRT_SECURE_NO_WARNINGS
#endif

#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

struct DIR {
  HANDLE find;
  WIN32_FIND_DATAA data;
  struct dirent ent;
  int first;
  int err;
};

static void dirent_seterrno(DWORD e) {
  switch (e) {
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
      errno = ENOENT;
      break;
    case ERROR_ACCESS_DENIED:
      errno = EACCES;
      break;
    case ERROR_NOT_ENOUGH_MEMORY:
    case ERROR_OUTOFMEMORY:
      errno = ENOMEM;
      break;
    default:
      errno = EIO;
      break;
  }
}

DIR *opendir(const char *name) {
  if (name == NULL) {
    errno = EINVAL;
    return NULL;
  }
  size_t len = strlen(name);
  while (len > 0 && (name[len - 1] == '\\' || name[len - 1] == '/')) len--;
  if (len == 0) {
    errno = ENOENT;
    return NULL;
  }
  /* pattern = name + "\\*" */
  size_t cap = len + 3;
  char *pattern = (char *)malloc(cap);
  if (!pattern) {
    errno = ENOMEM;
    return NULL;
  }
  memcpy(pattern, name, len);
  pattern[len] = '\\';
  pattern[len + 1] = '*';
  pattern[len + 2] = '\0';

  DIR *d = (DIR *)calloc(1, sizeof(DIR));
  if (!d) {
    free(pattern);
    errno = ENOMEM;
    return NULL;
  }
  d->find = FindFirstFileA(pattern, &d->data);
  free(pattern);
  if (d->find == INVALID_HANDLE_VALUE) {
    DWORD e = GetLastError();
    free(d);
    dirent_seterrno(e);
    return NULL;
  }
  d->first = 1;
  return d;
}

struct dirent *readdir(DIR *dir) {
  if (dir == NULL || dir->find == INVALID_HANDLE_VALUE) {
    errno = EBADF;
    return NULL;
  }
  for (;;) {
    if (!dir->first) {
      if (!FindNextFileA(dir->find, &dir->data)) {
        DWORD e = GetLastError();
        dirent_seterrno(e);
        return NULL; /* end of dir (ERROR_NO_MORE_FILES) or error */
      }
    }
    dir->first = 0;
    /* skip "." and ".." like every dirent consumer expects */
    const char *n = dir->data.cFileName;
    if (n[0] == '.' && (n[1] == '\0' || (n[1] == '.' && n[2] == '\0'))) continue;

    size_t l = strlen(n);
    if (l >= sizeof(dir->ent.d_name)) {
      errno = ENAMETOOLONG;
      return NULL;
    }
    memcpy(dir->ent.d_name, n, l + 1);
    /* d_type: unknown — the runtime's MinGW arm already treats dirent
     * entries without a usable d_type via the stat fallback. */
    dir->ent.d_type = DT_UNKNOWN;
    return &dir->ent;
  }
}

int closedir(DIR *dir) {
  if (dir == NULL || dir->find == INVALID_HANDLE_VALUE) {
    errno = EBADF;
    return -1;
  }
  BOOL ok = FindClose(dir->find);
  dir->find = INVALID_HANDLE_VALUE;
  free(dir);
  return ok ? 0 : -1;
}
