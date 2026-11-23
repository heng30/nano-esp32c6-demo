/* LVGL 自己会 include <assert.h>；断言钩子由 src/libc.rs 的 __assert_func 提供。 */
#ifndef CSHIM_ASSERT_H
#define CSHIM_ASSERT_H
void __assert_func(const char *file, int line, const char *func, const char *expr);
#ifdef NDEBUG
#define assert(expr) ((void)0)
#else
#define assert(expr) ((expr) ? (void)0 : __assert_func(__FILE__, __LINE__, __func__, #expr))
#endif
#endif
