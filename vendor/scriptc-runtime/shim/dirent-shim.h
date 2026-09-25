#ifndef DIRENT_SHIM_H
#define DIRENT_SHIM_H

/* Minimal dirent for MSVC — matches MinGW's dirent surface the runtime
 * consumes: DIR*, struct dirent { long d_ino? no; char d_name[...]; unsigned char d_type },
 * opendir/readdir/closedir. Rewinddir/seekdir/telldir are unused by the
 * runtime and left unimplemented on purpose. */

#include <stddef.h>

#define DT_UNKNOWN 0
#define DT_FIFO    1
#define DT_CHR     2
#define DT_DIR     4
#define DT_BLK     6
#define DT_REG     8
#define DT_LNK     10
#define DT_SOCK    12
#define DT_WHT     14

struct dirent {
  char d_name[1024];
  unsigned char d_type;
};

typedef struct DIR DIR;

DIR *opendir(const char *name);
struct dirent *readdir(DIR *dir);
int closedir(DIR *dir);

#endif /* DIRENT_SHIM_H */
