/* 只给声明：LVGL 的内置内存池不调 malloc，真被引用时会链接报错提示补实现。 */
#ifndef CSHIM_STDLIB_H
#define CSHIM_STDLIB_H
#include <stddef.h>
void *malloc(size_t n);
void free(void *p);
void *realloc(void *p, size_t n);
void *calloc(size_t n, size_t size);
int abs(int v);
long labs(long v);
int atoi(const char *s);
void exit(int code);
void abort(void);
#endif
