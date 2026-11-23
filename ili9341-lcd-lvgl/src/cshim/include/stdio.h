/* 同上：只有声明，LVGL 的格式化输出走自己的 lv_snprintf 实现。 */
#ifndef CSHIM_STDIO_H
#define CSHIM_STDIO_H
#include <stddef.h>
#include <stdarg.h>
int snprintf(char *s, size_t n, const char *fmt, ...);
int vsnprintf(char *s, size_t n, const char *fmt, va_list ap);
int sprintf(char *s, const char *fmt, ...);
int printf(const char *fmt, ...);
#endif
