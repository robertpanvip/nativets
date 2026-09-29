/* White-box memory contract: small suspended fibers must not commit their
 * whole stack, but large live frames must still survive awaits and yields. */
#undef NDEBUG
#include "scr_async.c"
#include <assert.h>

static void assert_sparse(ScrFiber *f, const void *anchor) {
  size_t used = 0;
#ifdef _WIN32
  (void)f;
  MEMORY_BASIC_INFORMATION region;
  assert(VirtualQuery(anchor, &region, sizeof region) == sizeof region);
  void *base = region.AllocationBase;
  char *cursor = base;
  while (VirtualQuery(cursor, &region, sizeof region) == sizeof region &&
         region.AllocationBase == base) {
    if (region.State == MEM_COMMIT) used += region.RegionSize;
    cursor += region.RegionSize;
  }
#else
  (void)anchor;
  long page = sysconf(_SC_PAGESIZE);
  assert(page > 0);
  size_t pages = SCR_FIBER_STACK / (size_t)page;
#ifdef __APPLE__
  char *resident = calloc(pages, 1);
#else
  unsigned char *resident = calloc(pages, 1);
#endif
  assert(resident != NULL);
  assert(mincore(f->stack, SCR_FIBER_STACK, resident) == 0);
  for (size_t i = 0; i < pages; i++) {
    if (resident[i] & 1) used += (size_t)page;
  }
  free(resident);
#endif
  /* Wide margin for OS/sanitizer frames; the original eager initialization
   * commits the entire reservation and fails this bound. */
  if (used >= SCR_FIBER_STACK / 2) {
    fprintf(stderr, "shallow fiber committed %zu of %d stack bytes\n",
            used, SCR_FIBER_STACK);
    abort();
  }
}

static void shallow_async(ScrFiber *f, void *gate) {
  char anchor = 0;
  assert_sparse(f, &anchor);
  scr_await_void(gate);
  scr_promise_fulfill_void(scr_fiber_promise(f));
}

static void shallow_gen(ScrFiber *f, void *unused) {
  (void)unused;
  char anchor = 0;
  assert_sparse(f, &anchor);
  scr_gen_yield_f64(17);
  scr_gen_out_f64(scr_gen_of_fiber(f), 23);
}

/* Volatile and noinline ensure this actually uses more than the small
 * initialization window. Check every byte again after suspension. */
__attribute__((noinline))
static void large_frame(ScrPromise *gate, bool generator, unsigned char seed) {
  volatile unsigned char bytes[128 * 1024];
  for (size_t i = 0; i < sizeof bytes; i++) bytes[i] = (unsigned char)(seed + i);
  if (generator) scr_gen_yield_f64(seed);
  else scr_await_void(gate);
  for (size_t i = 0; i < sizeof bytes; i++) assert(bytes[i] == (unsigned char)(seed + i));
}

static void deep_async(ScrFiber *f, void *gate) {
  large_frame(gate, false, 37);
  scr_promise_fulfill_void(scr_fiber_promise(f));
}

static void deep_gen(ScrFiber *f, void *unused) {
  (void)unused;
  large_frame(NULL, true, 91);
  scr_gen_out_f64(scr_gen_of_fiber(f), 92);
}

int main(void) {
  scr_init();
  enum { COUNT = 128 };
  ScrPromise *jobs[COUNT];
  ScrGen *gens[COUNT];
  ScrPromise *gate = scr_promise_new();
  for (int i = 0; i < COUNT; i++) {
    jobs[i] = scr_async_spawn(shallow_async, gate);
    gens[i] = scr_gen_new(shallow_gen, NULL, NULL);
#ifndef _WIN32
    assert_sparse(gens[i]->fiber, NULL);
#endif
    scr_gen_resume(gens[i]);
    assert(scr_gen_take_out_f64(gens[i]) == 17);
  }
  scr_promise_fulfill_void(gate);
  assert(!scr_loop_run(NULL));
  assert(!scr_exc_pending());
  scr_promise_release(gate);
  for (int i = 0; i < COUNT; i++) {
    scr_promise_release(jobs[i]);
    scr_gen_resume(gens[i]);
    assert(scr_gen_done(gens[i]));
    assert(scr_gen_take_out_f64(gens[i]) == 23);
    scr_gen_release(gens[i]);
  }

  /* Exercise full stack capacity on fresh and reused mappings, and keep a
   * large generator frame parked while other fibers complete. */
  for (int pass = 0; pass < 3; pass++) {
    gate = scr_promise_new();
    ScrGen *gen = scr_gen_new(deep_gen, NULL, NULL);
    scr_gen_resume(gen);
    assert(scr_gen_take_out_f64(gen) == 91);
    for (int i = 0; i < 8; i++) jobs[i] = scr_async_spawn(deep_async, gate);
    scr_promise_fulfill_void(gate);
    assert(!scr_loop_run(NULL));
    assert(!scr_exc_pending());
    scr_promise_release(gate);
    for (int i = 0; i < 8; i++) scr_promise_release(jobs[i]);
    scr_gen_resume(gen);
    assert(scr_gen_done(gen));
    assert(scr_gen_take_out_f64(gen) == 92);
    scr_gen_release(gen);
  }
  /* The last loop turn ran with a deliberately parked generator. Refresh
   * the exhaustion bookkeeping now that every generator has completed. */
  assert(!scr_loop_run(NULL));
  assert(scr_fibers_live == 0);
  puts("fiber stacks stay lazy and preserve large suspended frames");
  return 0;
}
