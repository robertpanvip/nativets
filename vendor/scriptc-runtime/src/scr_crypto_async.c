/* Callback-style node:crypto wrappers. Kept in a separate executable-only
 * translation unit: library-mode archives and minimal runtime tests link the
 * synchronous digest core without acquiring event-loop or bytes-I/O edges. */
#include "scr_runtime.h"

void scr_crypto_random_bytes_async(double size, ScrClosure *cb, ScrCryptoBytesFn fn) {
  ScrBytes *value = scr_crypto_random_bytes(size);
  if (scr_exc_pending()) {
    scr_closure_release(cb);
    return;
  }
  scr_crypto_defer_bytes(value, cb, fn);
}

void scr_crypto_pbkdf2_async(ScrBytes *password, ScrBytes *salt,
                             double iterations, double keylen, ScrStr *digest,
                             ScrClosure *cb, ScrCryptoBytesFn fn) {
  ScrBytes *value = scr_crypto_pbkdf2(password, salt, iterations, keylen, digest);
  if (scr_exc_pending()) {
    scr_closure_release(cb);
    return;
  }
  scr_crypto_defer_bytes(value, cb, fn);
}
