/* LVGL 8.3 的数学运算都在 C 里自己实现，只有极少数分支会用到 libm。 */
#ifndef CSHIM_MATH_H
#define CSHIM_MATH_H
double sqrt(double x);
float sqrtf(float x);
double sin(double x);
float sinf(float x);
double cos(double x);
float cosf(float x);
double pow(double x, double y);
float powf(float x, float y);
float fabsf(float x);
double fabs(double x);
float floorf(float x);
float ceilf(float x);
double fmod(double x, double y);
float fmodf(float x, float y);
#endif
