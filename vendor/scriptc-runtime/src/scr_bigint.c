/* Engine-free ECMAScript bigint values: arbitrary-precision signed
 * integers over little-endian base-2^32 limbs. This unit is linked only
 * when typed IR contains bigint values. */
#include "scr_runtime.h"

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct ScrBigInt {
  size_t rc;
  size_t len;
  int sign; /* -1, 0, +1; zero always has len == 0 */
  uint32_t limb[];
};

/* Node 24's V8 accepts widths above this when the input already fits, but
 * refuses any operation that would materialize a wider BigInt. */
#define BI_MAX_BITS ((size_t)1 << 30)

static void bi_oom(void) { scr_trap("scriptc: out of memory\n"); }

static bool bi_max_size_exceeded(void) {
  static const char msg[] = "Maximum BigInt size exceeded";
  scr_throw_error_msg(SCR_ERR_RANGE, msg, sizeof msg - 1);
  return false;
}

static ScrBigInt *bi_alloc(size_t cap) {
  if (cap > (SIZE_MAX - sizeof(ScrBigInt)) / sizeof(uint32_t)) bi_oom();
  ScrBigInt *v = calloc(1, sizeof(ScrBigInt) + cap * sizeof(uint32_t));
  if (!v) bi_oom();
  v->rc = 1;
  return v;
}

static void bi_normalize(ScrBigInt *v) {
  while (v->len && v->limb[v->len - 1] == 0) v->len--;
  if (!v->len) v->sign = 0;
}

static ScrBigInt *bi_zero(void) { return bi_alloc(0); }

static ScrBigInt *bi_u64(uint64_t n) {
  if (!n) return bi_zero();
  ScrBigInt *v = bi_alloc(n >> 32 ? 2 : 1);
  v->sign = 1;
  v->limb[0] = (uint32_t)n;
  v->len = 1;
  if (n >> 32) {
    v->limb[1] = (uint32_t)(n >> 32);
    v->len = 2;
  }
  return v;
}

static ScrBigInt *bi_copy(const ScrBigInt *a) {
  ScrBigInt *v = bi_alloc(a->len);
  v->len = a->len;
  v->sign = a->sign;
  if (a->len) memcpy(v->limb, a->limb, a->len * sizeof(uint32_t));
  return v;
}

ScrBigInt *scr_bigint_retain(ScrBigInt *v) {
  if (v && v->rc != SIZE_MAX) v->rc++;
  return v;
}

void scr_bigint_release(ScrBigInt *v) {
  if (v && v->rc != SIZE_MAX && --v->rc == 0) free(v);
}

void *scr_bigint_retain_v(void *v) { return scr_bigint_retain(v); }
void scr_bigint_release_v(void *v) { scr_bigint_release(v); }

static int bi_cmp_abs(const ScrBigInt *a, const ScrBigInt *b) {
  if (a->len != b->len) return a->len < b->len ? -1 : 1;
  for (size_t i = a->len; i-- > 0;) {
    if (a->limb[i] != b->limb[i]) return a->limb[i] < b->limb[i] ? -1 : 1;
  }
  return 0;
}

static int bi_cmp(const ScrBigInt *a, const ScrBigInt *b) {
  if (a->sign != b->sign) return a->sign < b->sign ? -1 : 1;
  if (!a->sign) return 0;
  int c = bi_cmp_abs(a, b);
  return a->sign < 0 ? -c : c;
}

bool scr_bigint_eq(ScrBigInt *a, ScrBigInt *b) { return bi_cmp(a, b) == 0; }
double scr_bigint_cmp_f64(ScrBigInt *a, ScrBigInt *b) { return (double)bi_cmp(a, b); }
bool scr_bigint_truthy(ScrBigInt *v) { return v->sign != 0; }

static ScrBigInt *bi_add_abs(const ScrBigInt *a, const ScrBigInt *b) {
  size_t n = a->len > b->len ? a->len : b->len;
  ScrBigInt *r = bi_alloc(n + 1);
  uint64_t carry = 0;
  for (size_t i = 0; i < n; i++) {
    uint64_t x = i < a->len ? a->limb[i] : 0;
    uint64_t y = i < b->len ? b->limb[i] : 0;
    uint64_t sum = x + y + carry;
    r->limb[i] = (uint32_t)sum;
    carry = sum >> 32;
  }
  r->limb[n] = (uint32_t)carry;
  r->len = n + (carry != 0);
  r->sign = r->len ? 1 : 0;
  return r;
}

/* |a| >= |b|. */
static ScrBigInt *bi_sub_abs(const ScrBigInt *a, const ScrBigInt *b) {
  ScrBigInt *r = bi_alloc(a->len);
  uint64_t borrow = 0;
  for (size_t i = 0; i < a->len; i++) {
    uint64_t x = a->limb[i];
    uint64_t y = (i < b->len ? b->limb[i] : 0) + borrow;
    r->limb[i] = (uint32_t)(x - y);
    borrow = x < y;
  }
  r->len = a->len;
  r->sign = 1;
  bi_normalize(r);
  return r;
}

ScrBigInt *scr_bigint_add(ScrBigInt *a, ScrBigInt *b) {
  if (!a->sign) return bi_copy(b);
  if (!b->sign) return bi_copy(a);
  if (a->sign == b->sign) {
    ScrBigInt *r = bi_add_abs(a, b);
    r->sign = a->sign;
    return r;
  }
  int c = bi_cmp_abs(a, b);
  if (!c) return bi_zero();
  ScrBigInt *r = c > 0 ? bi_sub_abs(a, b) : bi_sub_abs(b, a);
  r->sign = c > 0 ? a->sign : b->sign;
  return r;
}

ScrBigInt *scr_bigint_neg(ScrBigInt *a) {
  ScrBigInt *r = bi_copy(a);
  r->sign = -r->sign;
  return r;
}

ScrBigInt *scr_bigint_sub(ScrBigInt *a, ScrBigInt *b) {
  if (!b->sign) return bi_copy(a);
  ScrBigInt *negated = bi_copy(b);
  negated->sign = -negated->sign;
  ScrBigInt *result = scr_bigint_add(a, negated);
  scr_bigint_release(negated);
  return result;
}

ScrBigInt *scr_bigint_mul(ScrBigInt *a, ScrBigInt *b) {
  if (!a->sign || !b->sign) return bi_zero();
  ScrBigInt *r = bi_alloc(a->len + b->len);
  for (size_t i = 0; i < a->len; i++) {
    uint64_t carry = 0;
    for (size_t j = 0; j < b->len; j++) {
      size_t k = i + j;
      uint64_t p = (uint64_t)a->limb[i] * b->limb[j] + r->limb[k] + carry;
      r->limb[k] = (uint32_t)p;
      carry = p >> 32;
    }
    r->limb[i + b->len] = (uint32_t)carry;
  }
  r->len = a->len + b->len;
  r->sign = a->sign * b->sign;
  bi_normalize(r);
  return r;
}

static size_t bi_bits(const ScrBigInt *a) {
  if (!a->len) return 0;
  uint32_t top = a->limb[a->len - 1];
  size_t high = 0;
  while (top) {
    high++;
    top >>= 1;
  }
  size_t words = a->len - 1;
  if (words > (SIZE_MAX - high) / 32) bi_oom();
  return words * 32 + high;
}

static unsigned bi_bit(const ScrBigInt *a, size_t bit) {
  size_t i = bit / 32;
  return i < a->len ? (a->limb[i] >> (bit % 32)) & 1u : 0;
}

/* Mutable positive remainder with capacity cap >= denominator length + 1. */
static void bi_mut_shl1(ScrBigInt *a, size_t cap, unsigned bit) {
  uint64_t carry = bit;
  for (size_t i = 0; i < a->len; i++) {
    uint64_t v = ((uint64_t)a->limb[i] << 1) | carry;
    a->limb[i] = (uint32_t)v;
    carry = v >> 32;
  }
  if (carry) a->limb[a->len++] = (uint32_t)carry;
  else if (!a->len && bit && cap) {
    a->limb[0] = 1;
    a->len = 1;
  }
  a->sign = a->len ? 1 : 0;
}

static void bi_mut_sub_abs(ScrBigInt *a, const ScrBigInt *b) {
  uint64_t borrow = 0;
  for (size_t i = 0; i < a->len; i++) {
    uint64_t x = a->limb[i];
    uint64_t y = (i < b->len ? b->limb[i] : 0) + borrow;
    a->limb[i] = (uint32_t)(x - y);
    borrow = x < y;
  }
  bi_normalize(a);
}

static void bi_divmod_abs(const ScrBigInt *a, const ScrBigInt *b,
                          ScrBigInt **qout, ScrBigInt **rout) {
  ScrBigInt *q = bi_alloc(a->len);
  ScrBigInt *r = bi_alloc(b->len + 1);
  size_t bits = bi_bits(a);
  for (size_t p = bits; p-- > 0;) {
    bi_mut_shl1(r, b->len + 1, bi_bit(a, p));
    if (bi_cmp_abs(r, b) >= 0) {
      bi_mut_sub_abs(r, b);
      q->limb[p / 32] |= (uint32_t)1u << (p % 32);
    }
  }
  q->len = a->len;
  q->sign = 1;
  bi_normalize(q);
  *qout = q;
  *rout = r;
}

static bool bi_div_zero(ScrBigInt *b) {
  if (b->sign) return false;
  static const char msg[] = "Division by zero";
  scr_throw_error_msg(SCR_ERR_RANGE, msg, sizeof msg - 1);
  return true;
}

ScrBigInt *scr_bigint_div(ScrBigInt *a, ScrBigInt *b) {
  if (bi_div_zero(b)) return NULL;
  if (!a->sign) return bi_zero();
  ScrBigInt *q, *r;
  bi_divmod_abs(a, b, &q, &r);
  scr_bigint_release(r);
  if (q->sign) q->sign = a->sign * b->sign;
  return q;
}

ScrBigInt *scr_bigint_mod(ScrBigInt *a, ScrBigInt *b) {
  if (bi_div_zero(b)) return NULL;
  if (!a->sign) return bi_zero();
  ScrBigInt *q, *r;
  bi_divmod_abs(a, b, &q, &r);
  scr_bigint_release(q);
  if (r->sign) r->sign = a->sign;
  return r;
}

static ScrBigInt *bi_shl_abs(const ScrBigInt *a, size_t count) {
  if (!a->sign) return bi_zero();
  size_t words = count / 32;
  unsigned shift = (unsigned)(count % 32);
  if (words > SIZE_MAX - a->len - 1) bi_oom();
  ScrBigInt *r = bi_alloc(a->len + words + 1);
  uint64_t carry = 0;
  for (size_t i = 0; i < a->len; i++) {
    uint64_t v = ((uint64_t)a->limb[i] << shift) | carry;
    r->limb[i + words] = (uint32_t)v;
    carry = shift ? v >> 32 : 0;
  }
  r->limb[a->len + words] = (uint32_t)carry;
  r->len = a->len + words + (carry != 0);
  r->sign = a->sign;
  return r;
}

static ScrBigInt *bi_shr_abs(const ScrBigInt *a, size_t count, bool *discarded) {
  size_t words = count / 32;
  unsigned shift = (unsigned)(count % 32);
  *discarded = false;
  if (words >= a->len) {
    *discarded = a->sign != 0;
    return bi_zero();
  }
  for (size_t i = 0; i < words; i++) if (a->limb[i]) *discarded = true;
  if (shift && (a->limb[words] & (((uint32_t)1u << shift) - 1u))) *discarded = true;
  size_t n = a->len - words;
  ScrBigInt *r = bi_alloc(n);
  uint32_t carry = 0;
  for (size_t i = n; i-- > 0;) {
    uint32_t cur = a->limb[i + words];
    r->limb[i] = shift ? (cur >> shift) | carry : cur;
    carry = shift ? cur << (32 - shift) : 0;
  }
  r->len = n;
  r->sign = r->len ? 1 : 0;
  bi_normalize(r);
  return r;
}

static bool bi_count(const ScrBigInt *v, size_t *out) {
  if (!v->sign) { *out = 0; return true; }
  if (v->len > 2) return false;
  uint64_t n = 0;
  for (size_t i = v->len; i-- > 0;) {
    n = (n << 32) | v->limb[i];
  }
  if (n > SIZE_MAX) return false;
  *out = (size_t)n;
  return true;
}

static ScrBigInt *bi_shift(ScrBigInt *a, ScrBigInt *count, bool left) {
  bool direction = left ^ (count->sign < 0);
  size_t n;
  if (!bi_count(count, &n)) {
    if (!direction) {
      ScrBigInt *r = bi_zero();
      if (a->sign < 0) { r->len = 1; r->sign = -1; r->limb[0] = 1; }
      return r;
    }
    bi_max_size_exceeded();
    return NULL;
  }
  if (direction) return bi_shl_abs(a, n);
  bool discarded;
  ScrBigInt *r = bi_shr_abs(a, n, &discarded);
  if (a->sign >= 0) return r;
  r->sign = r->len ? -1 : 0;
  if (discarded) {
    ScrBigInt *one = bi_u64(1);
    ScrBigInt *rounded = bi_add_abs(r, one);
    rounded->sign = -1;
    scr_bigint_release(one);
    scr_bigint_release(r);
    return rounded;
  }
  return r;
}

ScrBigInt *scr_bigint_shl(ScrBigInt *a, ScrBigInt *count) { return bi_shift(a, count, true); }
ScrBigInt *scr_bigint_shr(ScrBigInt *a, ScrBigInt *count) { return bi_shift(a, count, false); }

ScrBigInt *scr_bigint_pow(ScrBigInt *a, ScrBigInt *b) {
  if (b->sign < 0) {
    static const char msg[] = "undefined must be positive";
    scr_throw_error_msg(SCR_ERR_RANGE, msg, sizeof msg - 1);
    return NULL;
  }
  ScrBigInt *result = bi_u64(1);
  ScrBigInt *base = bi_copy(a);
  size_t bits = bi_bits(b);
  for (size_t i = 0; i < bits; i++) {
    if (bi_bit(b, i)) {
      ScrBigInt *next = scr_bigint_mul(result, base);
      scr_bigint_release(result);
      result = next;
    }
    if (i + 1 < bits) {
      ScrBigInt *next = scr_bigint_mul(base, base);
      scr_bigint_release(base);
      base = next;
    }
  }
  scr_bigint_release(base);
  return result;
}

static void bi_to_twos(const ScrBigInt *a, uint32_t *out, size_t n) {
  memset(out, 0, n * sizeof(uint32_t));
  size_t copy = a->len < n ? a->len : n;
  if (copy) memcpy(out, a->limb, copy * sizeof(uint32_t));
  if (a->sign >= 0) return;
  uint64_t carry = 1;
  for (size_t i = 0; i < n; i++) {
    uint64_t v = (uint64_t)(~out[i]) + carry;
    out[i] = (uint32_t)v;
    carry = v >> 32;
  }
}

static ScrBigInt *bi_from_twos(uint32_t *v, size_t n, bool negative) {
  ScrBigInt *r = bi_alloc(n);
  if (!negative) {
    memcpy(r->limb, v, n * sizeof(uint32_t));
    r->len = n;
    r->sign = 1;
    bi_normalize(r);
    return r;
  }
  uint64_t carry = 1;
  for (size_t i = 0; i < n; i++) {
    uint64_t x = (uint64_t)(~v[i]) + carry;
    r->limb[i] = (uint32_t)x;
    carry = x >> 32;
  }
  r->len = n;
  r->sign = -1;
  bi_normalize(r);
  return r;
}

static ScrBigInt *bi_bitop(ScrBigInt *a, ScrBigInt *b, char op) {
  size_t n = (a->len > b->len ? a->len : b->len) + 1;
  uint32_t *x = calloc(n * 2, sizeof(uint32_t));
  if (!x) bi_oom();
  uint32_t *y = x + n;
  bi_to_twos(a, x, n);
  bi_to_twos(b, y, n);
  for (size_t i = 0; i < n; i++) {
    x[i] = op == '&' ? x[i] & y[i] : op == '|' ? x[i] | y[i] : x[i] ^ y[i];
  }
  bool neg = (x[n - 1] & 0x80000000u) != 0;
  ScrBigInt *r = bi_from_twos(x, n, neg);
  free(x);
  return r;
}

ScrBigInt *scr_bigint_and(ScrBigInt *a, ScrBigInt *b) { return bi_bitop(a, b, '&'); }
ScrBigInt *scr_bigint_or(ScrBigInt *a, ScrBigInt *b) { return bi_bitop(a, b, '|'); }
ScrBigInt *scr_bigint_xor(ScrBigInt *a, ScrBigInt *b) { return bi_bitop(a, b, '^'); }

ScrBigInt *scr_bigint_not(ScrBigInt *a) {
  size_t n = a->len + 1;
  uint32_t *v = calloc(n, sizeof(uint32_t));
  if (!v) bi_oom();
  bi_to_twos(a, v, n);
  for (size_t i = 0; i < n; i++) v[i] = ~v[i];
  ScrBigInt *r = bi_from_twos(v, n, (v[n - 1] & 0x80000000u) != 0);
  free(v);
  return r;
}

static void bi_mul_small(ScrBigInt **pv, uint32_t m, uint32_t add) {
  ScrBigInt *a = *pv;
  size_t cap = a->len + 1;
  ScrBigInt *r = bi_alloc(cap);
  uint64_t carry = add;
  for (size_t i = 0; i < a->len; i++) {
    uint64_t x = (uint64_t)a->limb[i] * m + carry;
    r->limb[i] = (uint32_t)x;
    carry = x >> 32;
  }
  r->limb[a->len] = (uint32_t)carry;
  r->len = a->len + (carry != 0);
  r->sign = r->len ? 1 : 0;
  scr_bigint_release(a);
  *pv = r;
}

static int bi_digit(unsigned char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'z') return c - 'a' + 10;
  if (c >= 'A' && c <= 'Z') return c - 'A' + 10;
  return -1;
}

/* ECMAScript WhiteSpace + LineTerminator code points, matched over the
 * runtime's UTF-8 strings. libc isspace is locale-dependent and only sees
 * one byte, while StringToBigInt trims the full spec set. */
static size_t bi_space_prefix(const char *s, size_t n) {
  if (n == 0) return 0;
  unsigned char a = (unsigned char)s[0];
  if (a == 0x09 || a == 0x0a || a == 0x0b || a == 0x0c ||
      a == 0x0d || a == 0x20) return 1;
  if (n >= 2 && a == 0xc2 && (unsigned char)s[1] == 0xa0) return 2;
  if (n < 3) return 0;
  unsigned char b = (unsigned char)s[1];
  unsigned char c = (unsigned char)s[2];
  if (a == 0xe1 && b == 0x9a && c == 0x80) return 3; /* U+1680 */
  if (a == 0xe2 && b == 0x80 &&
      ((c >= 0x80 && c <= 0x8a) || c == 0xa8 || c == 0xa9 || c == 0xaf)) return 3;
  if (a == 0xe2 && b == 0x81 && c == 0x9f) return 3; /* U+205F */
  if (a == 0xe3 && b == 0x80 && c == 0x80) return 3; /* U+3000 */
  if (a == 0xef && b == 0xbb && c == 0xbf) return 3; /* U+FEFF */
  return 0;
}

static size_t bi_space_suffix(const char *s, size_t n) {
  if (n == 0) return 0;
  size_t start = n - 1;
  while (start > 0 && ((unsigned char)s[start] & 0xc0u) == 0x80u) start--;
  size_t width = bi_space_prefix(s + start, n - start);
  return width == n - start ? width : 0;
}

static ScrBigInt *bi_parse_fail(const ScrStr *s) {
  static const char pre[] = "Cannot convert ";
  static const char post[] = " to a BigInt";
  size_t n = sizeof pre - 1 + s->len + sizeof post - 1;
  char *msg = malloc(n);
  if (!msg) bi_oom();
  memcpy(msg, pre, sizeof pre - 1);
  memcpy(msg + sizeof pre - 1, s->data, s->len);
  memcpy(msg + sizeof pre - 1 + s->len, post, sizeof post - 1);
  scr_throw_error_msg(SCR_ERR_SYNTAX, msg, n);
  free(msg);
  return NULL;
}

static ScrBigInt *bi_parse_invalid(const ScrStr *s, bool report_error) {
  return report_error ? bi_parse_fail(s) : NULL;
}

static ScrBigInt *bi_parse_string(ScrStr *s, bool report_error) {
  size_t lo = 0, hi = s->len;
  size_t width;
  while (lo < hi && (width = bi_space_prefix(s->data + lo, hi - lo)) != 0) lo += width;
  while (hi > lo && (width = bi_space_suffix(s->data + lo, hi - lo)) != 0) hi -= width;
  if (lo == hi) return bi_zero();
  int sign = 1;
  bool signed_input = false;
  if (s->data[lo] == '+' || s->data[lo] == '-') {
    signed_input = true;
    if (s->data[lo++] == '-') sign = -1;
    if (lo == hi) return bi_parse_invalid(s, report_error);
  }
  unsigned base = 10;
  if (hi - lo >= 2 && s->data[lo] == '0') {
    char p = s->data[lo + 1];
    if (p == 'x' || p == 'X') base = 16;
    else if (p == 'o' || p == 'O') base = 8;
    else if (p == 'b' || p == 'B') base = 2;
    if (base != 10) {
      if (signed_input) return bi_parse_invalid(s, report_error);
      lo += 2;
      if (lo == hi) return bi_parse_invalid(s, report_error);
    }
  }
  ScrBigInt *v = bi_zero();
  bool any = false;
  for (; lo < hi; lo++) {
    int d = bi_digit((unsigned char)s->data[lo]);
    if (d < 0 || (unsigned)d >= base) {
      scr_bigint_release(v);
      return bi_parse_invalid(s, report_error);
    }
    bi_mul_small(&v, base, (uint32_t)d);
    any = true;
  }
  if (!any) { scr_bigint_release(v); return bi_parse_invalid(s, report_error); }
  if (v->sign) v->sign = sign;
  return v;
}

ScrBigInt *scr_bigint_parse(ScrStr *s) { return bi_parse_string(s, true); }

/* Abstract Equality's String/BigInt arm uses StringToBigInt without
 * surfacing its parse failure: an invalid string compares false instead
 * of throwing. BigInt(string) keeps the public throwing parser above. */
bool scr_bigint_eq_string(ScrBigInt *a, ScrStr *b) {
  ScrBigInt *parsed = bi_parse_string(b, false);
  if (!parsed) return false;
  bool equal = scr_bigint_eq(a, parsed);
  scr_bigint_release(parsed);
  return equal;
}

ScrBigInt *scr_bigint_from_f64(double value) {
  if (!isfinite(value) || trunc(value) != value) {
    char num[64], msg[192];
    if (isnan(value)) strcpy(num, "NaN");
    else if (isinf(value)) strcpy(num, value < 0 ? "-Infinity" : "Infinity");
    else snprintf(num, sizeof num, "%.17g", value);
    int n = snprintf(msg, sizeof msg,
      "The number %s cannot be converted to a BigInt because it is not an integer", num);
    scr_throw_error_msg(SCR_ERR_RANGE, msg, (size_t)n);
    return NULL;
  }
  if (value == 0) return bi_zero();
  int sign = value < 0 ? -1 : 1;
  double mag = fabs(value);
  int exp;
  double frac = frexp(mag, &exp);
  uint64_t mant = (uint64_t)ldexp(frac, 53);
  ScrBigInt *v = bi_u64(mant);
  if (exp > 53) {
    ScrBigInt *shifted = bi_shl_abs(v, (size_t)(exp - 53));
    scr_bigint_release(v);
    v = shifted;
  } else if (exp < 53) {
    bool discarded;
    ScrBigInt *shifted = bi_shr_abs(v, (size_t)(53 - exp), &discarded);
    scr_bigint_release(v);
    v = shifted;
  }
  v->sign = sign;
  return v;
}

double scr_bigint_cmp_number(ScrBigInt *a, double b) {
  if (isnan(b)) return 2;
  if (isinf(b)) return b > 0 ? -1 : 1;
  double integral = trunc(b);
  ScrBigInt *other = scr_bigint_from_f64(integral);
  int cmp = bi_cmp(a, other);
  scr_bigint_release(other);
  if (cmp != 0 || integral == b) return (double)cmp;
  return b > 0 ? -1 : 1;
}

static uint32_t bi_div_small(uint32_t *limb, size_t n, uint32_t d) {
  uint64_t rem = 0;
  for (size_t i = n; i-- > 0;) {
    uint64_t cur = (rem << 32) | limb[i];
    limb[i] = (uint32_t)(cur / d);
    rem = cur % d;
  }
  return (uint32_t)rem;
}

ScrStr *scr_bigint_to_string(ScrBigInt *v, double radix_value) {
  if (!isfinite(radix_value) || trunc(radix_value) != radix_value || radix_value < 2 || radix_value > 36) {
    static const char msg[] = "toString() radix argument must be between 2 and 36";
    scr_throw_error_msg(SCR_ERR_RANGE, msg, sizeof msg - 1);
    return NULL;
  }
  unsigned radix = (unsigned)radix_value;
  if (!v->sign) return scr_str_new("0", 1);
  size_t bits = bi_bits(v);
  if (bits > SIZE_MAX - 2 || v->len > SIZE_MAX / sizeof(uint32_t)) bi_oom();
  size_t cap = bits + 2; /* base 2 is longest, plus sign */
  char *buf = malloc(cap);
  uint32_t *tmp = malloc(v->len * sizeof(uint32_t));
  if (!buf || !tmp) bi_oom();
  memcpy(tmp, v->limb, v->len * sizeof(uint32_t));
  size_t nlimbs = v->len, at = cap;
  static const char digits[] = "0123456789abcdefghijklmnopqrstuvwxyz";
  while (nlimbs) {
    uint32_t rem = bi_div_small(tmp, nlimbs, radix);
    buf[--at] = digits[rem];
    while (nlimbs && tmp[nlimbs - 1] == 0) nlimbs--;
  }
  if (v->sign < 0) buf[--at] = '-';
  ScrStr *out = scr_str_new(buf + at, cap - at);
  free(tmp);
  free(buf);
  return out;
}

ScrStr *scr_bigint_inspect(ScrBigInt *v) {
  ScrStr *digits = scr_bigint_to_string(v, 10);
  if (!digits) return NULL;
  char *buf = malloc(digits->len + 1);
  if (!buf) bi_oom();
  memcpy(buf, digits->data, digits->len);
  buf[digits->len] = 'n';
  ScrStr *out = scr_str_new(buf, digits->len + 1);
  free(buf);
  scr_str_release(digits);
  return out;
}

double scr_bigint_to_f64(ScrBigInt *v) {
  ScrStr *s = scr_bigint_to_string(v, 10);
  if (!s) return 0;
  double out = strtod(s->data, NULL);
  scr_str_release(s);
  return out;
}

static bool bi_index(double bits, size_t *out) {
  if (!isfinite(bits) || trunc(bits) != bits || bits < 0 || bits > 9007199254740991.0 || bits > (double)SIZE_MAX) {
    static const char msg[] = "Invalid value: not (convertible to) a safe integer";
    scr_throw_error_msg(SCR_ERR_RANGE, msg, sizeof msg - 1);
    return false;
  }
  *out = (size_t)bits;
  return true;
}

/* Whether |value| < 2^bits, without materializing the power or multiplying
 * the width. This is also the large-width fast path: Node returns a value
 * that already fits without allocating storage proportional to `bits`. */
static bool bi_abs_lt_pow2(const ScrBigInt *value, size_t bits) {
  size_t full = bits / 32;
  unsigned high = (unsigned)(bits % 32);
  if (value->len <= full) return true;
  if (!high || value->len != full + 1) return false;
  return value->limb[full] < ((uint32_t)1u << high);
}

/* Whether |value| == 2^bits. The inclusive signed lower bound
 * (-2^(bits-1)) needs this one extra case beyond bi_abs_lt_pow2. */
static bool bi_abs_eq_pow2(const ScrBigInt *value, size_t bits) {
  size_t word = bits / 32;
  unsigned bit = (unsigned)(bits % 32);
  if (value->len != word + 1 || value->limb[word] != ((uint32_t)1u << bit)) return false;
  for (size_t i = 0; i < word; i++) {
    if (value->limb[i] != 0) return false;
  }
  return true;
}

/* Ceil(bits / 32) without the `(bits + 31)` overflow that is reachable on
 * wasm32. Enforce Node's materialized-value limit before allocation and
 * reject internal sizing failures before any n-1 indexing. */
static bool bi_twos_words(size_t bits, size_t *out, unsigned *high) {
  if (bits > BI_MAX_BITS) return bi_max_size_exceeded();
  *high = (unsigned)(bits % 32);
  size_t n = bits / 32 + (*high != 0);
  if (!n || n > SIZE_MAX / sizeof(uint32_t)) bi_oom();
  *out = n;
  return true;
}

ScrBigInt *scr_bigint_as_uint_n(double bits_value, ScrBigInt *value) {
  size_t bits;
  if (!bi_index(bits_value, &bits)) return NULL;
  if (!bits) return bi_zero();
  if (value->sign >= 0 && bi_abs_lt_pow2(value, bits)) return scr_bigint_retain(value);
  unsigned high;
  size_t n;
  if (!bi_twos_words(bits, &n, &high)) return NULL;
  uint32_t *twos = calloc(n, sizeof(uint32_t));
  if (!twos) bi_oom();
  bi_to_twos(value, twos, n);
  if (high) twos[n - 1] &= ((uint32_t)1u << high) - 1u;
  ScrBigInt *r = bi_from_twos(twos, n, false);
  free(twos);
  return r;
}

ScrBigInt *scr_bigint_as_int_n(double bits_value, ScrBigInt *value) {
  size_t bits;
  if (!bi_index(bits_value, &bits)) return NULL;
  if (!bits) return bi_zero();
  size_t magnitude_bits = bits - 1;
  bool fits = bi_abs_lt_pow2(value, magnitude_bits) ||
    (value->sign < 0 && bi_abs_eq_pow2(value, magnitude_bits));
  if (fits) return scr_bigint_retain(value);
  unsigned high;
  size_t n;
  if (!bi_twos_words(bits, &n, &high)) return NULL;
  uint32_t *twos = calloc(n, sizeof(uint32_t));
  if (!twos) bi_oom();
  bi_to_twos(value, twos, n);
  if (high) twos[n - 1] &= ((uint32_t)1u << high) - 1u;
  size_t sign_bit = (bits - 1) % 32;
  bool neg = (twos[n - 1] & ((uint32_t)1u << sign_bit)) != 0;
  if (neg && high) twos[n - 1] |= ~(((uint32_t)1u << high) - 1u);
  ScrBigInt *r = bi_from_twos(twos, n, neg);
  free(twos);
  return r;
}

static ScrBigInt *bi_from_u64_signed(uint64_t value, bool sign) {
  if (!sign || !(value & UINT64_C(0x8000000000000000))) return bi_u64(value);
  uint64_t magnitude = ~value + 1;
  ScrBigInt *out = bi_u64(magnitude);
  out->sign = out->len ? -1 : 0;
  return out;
}

static uint64_t bi_low_u64(const ScrBigInt *value) {
  uint64_t out = value->len ? value->limb[0] : 0;
  if (value->len > 1) out |= (uint64_t)value->limb[1] << 32;
  return value->sign < 0 ? ~out + 1 : out;
}

static void bi_grouped_decimal(const ScrBigInt *value, char **out, size_t *out_len) {
  ScrStr *plain = scr_bigint_to_string((ScrBigInt *)value, 10);
  size_t start = plain->len && plain->data[0] == '-' ? 1 : 0;
  size_t digits = plain->len - start;
  size_t groups = digits > 3 ? (digits - 1) / 3 : 0;
  char *buf = malloc(plain->len + groups + 2);
  if (!buf) bi_oom();
  size_t src = 0, dst = 0;
  if (start) buf[dst++] = plain->data[src++];
  size_t first = digits % 3;
  if (!first) first = 3;
  memcpy(buf + dst, plain->data + src, first);
  src += first;
  dst += first;
  while (src < plain->len) {
    buf[dst++] = '_';
    memcpy(buf + dst, plain->data + src, 3);
    src += 3;
    dst += 3;
  }
  buf[dst++] = 'n';
  buf[dst] = 0;
  scr_str_release(plain);
  *out = buf;
  *out_len = dst;
}

static bool bi_buffer_value(const ScrBigInt *value, bool sign, uint64_t *out) {
  bool valid;
  if (!sign) {
    valid = value->sign >= 0 && value->len <= 2;
  } else if (value->sign >= 0) {
    valid = value->len < 2 || (value->len == 2 && value->limb[1] <= 0x7fffffffu);
  } else {
    valid = value->len < 2 ||
      (value->len == 2 && (value->limb[1] < 0x80000000u ||
        (value->limb[1] == 0x80000000u && value->limb[0] == 0)));
  }
  if (valid) {
    *out = bi_low_u64(value);
    return true;
  }
  char *received;
  size_t received_len;
  bi_grouped_decimal(value, &received, &received_len);
  const char *range = sign ? ">= -(2n ** 63n) and < 2n ** 63n" : ">= 0n and < 2n ** 64n";
  size_t need = strlen(range) + received_len + 80;
  char *msg = malloc(need);
  if (!msg) bi_oom();
  int n = snprintf(msg, need, "The value of \"value\" is out of range. It must be %s. Received %s", range, received);
  scr_throw_error_msg_code(SCR_ERR_RANGE, msg, (size_t)n, "ERR_OUT_OF_RANGE");
  free(received);
  free(msg);
  return false;
}

ScrBigInt *scr_bigint_buffer_read(ScrBytes *bytes, double offset, bool sign, bool le) {
  uint64_t value;
  if (!scr_bytes_read_u64_raw(bytes, offset, le, &value)) return NULL;
  return bi_from_u64_signed(value, sign);
}

double scr_bigint_buffer_write(ScrBytes *bytes, ScrBigInt *value, double offset, bool sign, bool le) {
  uint64_t raw;
  if (!bi_buffer_value(value, sign, &raw)) return 0;
  if (!scr_bytes_write_u64_raw(bytes, offset, le, raw)) return 0;
  return offset + 8;
}

ScrBigInt *scr_bigint_dataview_get(ScrBytes *bytes, double offset, bool sign, bool le) {
  uint64_t value;
  if (!scr_dataview_read_u64_raw(bytes, offset, le, &value)) return NULL;
  return bi_from_u64_signed(value, sign);
}

void scr_bigint_dataview_set(ScrBytes *bytes, double offset, ScrBigInt *value, bool le) {
  (void)scr_dataview_write_u64_raw(bytes, offset, le, bi_low_u64(value));
}
