/* LVGL 的 lv_printf.h 会 include <inttypes.h> 取 PRI* 宏。 */
#ifndef CSHIM_INTTYPES_H
#define CSHIM_INTTYPES_H
#include <stdint.h>
#define PRId8 "d"
#define PRIu8 "u"
#define PRIx8 "x"
#define PRId16 "d"
#define PRIu16 "u"
#define PRIx16 "x"
#define PRId32 "d"
#define PRIi32 "i"
#define PRIu32 "u"
#define PRIx32 "x"
#define PRIX32 "X"
#define PRId64 "lld"
#define PRIi64 "lld"
#define PRIu64 "llu"
#define PRIx64 "llx"
#define PRIdPTR "d"
#define PRIuPTR "u"
#endif
