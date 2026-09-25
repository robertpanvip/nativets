/* dirent.h for MSVC builds of the scriptc runtime — reuses dirent-shim.h.
 * Sits in the shim/ include dir so `#include <dirent.h>` resolves. */
#ifndef SCR_MSVC_DIRENT_FAKE
#define SCR_MSVC_DIRENT_FAKE
#include "dirent-shim.h"
#endif
