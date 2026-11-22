# ili9341-lcd-lgvl

同一块野火 2.8 寸 ILI9341（240×320，16 位 8080 并口）上跑 **LVGL 8.3**，做
`../ili9341-lcd-slint/` 那页音乐播放器：唱片旋转、唱针由盘心移向盘边、进度条、上一首/暂停/下一首、
歌词滚动。区别在于这里的控件都是 LVGL 自己的，不是 Rust 逐像素画完再贴一张图。

屏幕接线与 `../ili9341-lcd/` 完全相同，**接线表以 [../ili9341-lcd/README.md](../ili9341-lcd/README.md) 为准**。

- 栈：`esp-hal 1.2` + `esp-rtos` + `embassy`（`#[esp_rtos::start]`），`#![no_std]`，不用 alloc
- 界面：`lvgl-sys 0.6.2`（LVGL 8.3.5 的 C 源码构建期编译，`use-vendored-config` 用它的 `lv_conf.h`）
- 渲染：LVGL 软件渲染 → `flush_cb` 逐行写并口，绘制缓冲 320×40 像素

## 各元素用什么画的

| 屏幕上的东西 | LVGL 元素 | 做法 |
|---|---|---|
| 唱片（盘面 / 纹路 / 四象限标签） | `lv_img` | 启动时把 132×132 的 RGB565 位图画进静态缓冲，`lv_img_set_angle` 每帧改角度整块旋转，`lv_img_set_pivot` 定在盘心 |
| 唱臂 + 支点 + 唱针头 | `lv_line` + 两个 `lv_obj` | 线段两个点每帧按播放进度改写（盘心 → 盘边），支点是 10×10 的圆，针头是 9×13 的方块跟着线段末端走 |
| 进度条 | `lv_bar` | 本首已播千分比，0–1000 |
| 歌词 | `lv_label` | 15 行一次建好，逐帧改 y 做滚动（一阶低通，不跳行）、改文字色做高亮，离当前行太远的直接 `LV_OBJ_FLAG_HIDDEN` |
| 上一首 / 暂停 / 下一首 | `lv_btn` + `lv_label` | 图标用 LVGL 内置的 `LV_SYMBOL_PREV/PAUSE/NEXT`。屏没有触摸（XPT2046 没接线），纯装饰 |

播放列表、歌词、时间轴都在 `src/music.rs` 里，3 首歌共 36 秒循环。

## 构建：LVGL 是 C 库

这是本仓唯一需要 C 工具链的示例。**先起 shell**：

```bash
cd ili9341-lcd-lgvl
nix-shell                      # 用本目录的 shell.nix：riscv32 交叉 GCC + binutils + libclang
cargo build --release
cargo run --release            # 烧录 + RTT 日志
```

`shell.nix` 给的 `pkgsCross.riscv32-embedded.buildPackages.gcc` 是 **nolibc** 的 GCC，而且它的
wrapper 会把宿主 glibc 的头文件塞进搜索路径（编 riscv32 时报 `gnu/stubs-32.h` 缺失）。
本目录的做法是不链接 newlib，自己顶掉那套头：

- `src/cshim/include/`：`stdint.h`、`stddef.h`、`stdarg.h`、`string.h`、`stdlib.h`、`stdio.h`、
  `math.h`、`assert.h`、`inttypes.h`、`limits.h`、`ctype.h`、`stdbool.h` 的极简声明。
  `.cargo/config.toml` 里通过 `CPATH` 注入——`CPATH` 排在 wrapper 的 `-isystem` 之前，glibc 的头就被挡掉了。
- 真正会被链接的 libc 符号只有 `strlen`/`strcmp`/`strcpy`/`strcat`，`lvgl-sys` 自己带着；
  `__assert_func` 由 `esp-rom-sys` 提供；`memcpy` 一类来自 compiler-builtins。所以不需要 libc.a。
- `lvgl-sys` 的 `build.rs` 会把 Rust 三元组直接丢给 libclang，而 clang 只认 `riscv32-unknown-elf`，
  用 `BINDGEN_EXTRA_CLANG_ARGS_riscv32imac_unknown_none_elf` 再补一次 `--target`。

`.cargo/config.toml` 里的 `CPATH` 是**本机仓库的绝对路径**（`[env] relative` 在这里的 cargo 上没生效），
换机器要改这一行。`LIBCLANG_PATH` 由 `shell.nix` 的 `shellHook` 导出。

## 内存

| 用途 | 大小 |
|---|---|
| LVGL 自己的内存池（`lv_conf.h` 的 `LV_MEM_SIZE`） | 48 KB，静态 |
| LVGL 绘制缓冲（320×40，RGB565） | 25 KB，静态 |
| 唱片位图（132×132，RGB565） | 34.8 KB，静态 |

不注册 `#[global_allocator]`：Rust 侧没有任何 `alloc`，LVGL 也只用它自己的池子。

## 注意

- 换字号要改 `lv_conf.h` 的 `LV_FONT_MONTSERRAT_*`，再用 `use-vendored-config` 之外的路径给
  `DEP_LV_CONFIG_PATH` 指一份自己的配置；默认只开了 montserrat 14。
- `lv_obj_set_width` 之后还要 `lv_obj_set_size` 固定行高，否则 label 按内容换行，两行歌词会叠在一起。
- 歌词行号用 Q8 定点做插值时，整数除法必须用 `div_euclid`（向下取整）；用 `/` 向零取整会让相邻
  两行同时算成「当前行」，两行文字重叠。
- LVGL 旋转是逐像素反算，唱片 120×120 每帧重画一遍；再大就要考虑降低帧率。
