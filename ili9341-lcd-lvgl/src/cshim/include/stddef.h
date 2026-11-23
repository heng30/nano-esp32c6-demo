/* 极简 stddef：这个 crate 不带 newlib，只需要够 LVGL 编译的那几个定义。 */
#ifndef CSHIM_STDDEF_H
#define CSHIM_STDDEF_H
typedef __SIZE_TYPE__ size_t;
typedef __PTRDIFF_TYPE__ ptrdiff_t;
typedef __WCHAR_TYPE__ wchar_t;
#ifndef NULL
#define NULL ((void *)0)
#endif
#define offsetof(type, member) __builtin_offsetof(type, member)
#endif
