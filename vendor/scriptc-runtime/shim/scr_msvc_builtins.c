/* scr_msvc_builtins.c — GCC/Clang __builtin_* shims for MSVC builds of the
 * scriptc runtime. Only the spellings scr_string.c (and friends) use. */
#include <stdint.h>

#if defined(_MSC_VER) && !defined(__clang__)
#include <intrin.h>

static inline uint64_t msvc_popcountll(uint64_t v) {
#ifdef _WIN64
  return (uint64_t)__popcnt64(v);
#else
  return (uint64_t)(__popcnt((unsigned int)(v)) + __popcnt((unsigned int)(v >> 32)));
#endif
}

static inline unsigned long msvc_clz32(uint32_t v) {
  /* _BitScanReverse gives the index of the highest set bit; clz = 31 - idx.
   * v is guaranteed non-zero at the call sites in the runtime. */
  unsigned long idx;
  _BitScanReverse(&idx, v);
  return 31UL - idx;
}

static inline unsigned long msvc_clzll64(uint64_t v) {
  unsigned long idx;
#ifdef _WIN64
  _BitScanReverse64(&idx, v);
#else
  if ((uint32_t)(v >> 32) != 0) {
    _BitScanReverse(&idx, (uint32_t)(v >> 32));
    return 63UL - (idx + 32UL);
  }
  _BitScanReverse(&idx, (uint32_t)v);
  return 31UL - idx;
#endif
  return 63UL - idx;
}

uint64_t __builtin_popcountll(uint64_t v) { return msvc_popcountll(v); }
unsigned long __builtin_clz(uint32_t v) { return msvc_clz32(v); }
unsigned long __builtin_clzll(uint64_t v) { return msvc_clzll64(v); }
unsigned long __builtin_ctz(uint32_t v) {
  unsigned long idx;
  _BitScanForward(&idx, v);
  return idx;
}
unsigned long __builtin_ctzll(uint64_t v) {
#ifdef _WIN64
  unsigned long idx;
  _BitScanForward64(&idx, v);
  return idx;
#else
  if ((uint32_t)v != 0) {
    unsigned long idx;
    _BitScanForward(&idx, (uint32_t)v);
    return idx;
  }
  return 32UL + __builtin_ctz((uint32_t)(v >> 32));
#endif
}
#endif /* _MSC_VER && !__clang__ */
