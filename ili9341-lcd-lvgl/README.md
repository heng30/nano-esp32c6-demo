# ili9341-lcd-lvgl

同一块野火 2.8 寸 ILI9341（240×320，16 位 8080 并口）上跑 **LVGL 9.6**，做
`../ili9341-lcd-slint/` 那页音乐播放器：唱片旋转、唱针由盘心移向盘边、进度条、上一首/暂停/下一首、
歌词滚动。区别在于这里的控件都是 LVGL 自己的，不是 Rust 逐像素画完再贴一张图。

屏幕接线与 `../ili9341-lcd/` 完全相同，**接线表以 [../ili9341-lcd/README.md](../ili9341-lcd/README.md) 为准**。

- 栈：`esp-hal 1.2` + `esp-rtos` + `embassy`（`#[esp_rtos::start]`），`#![no_std]`，不用 alloc
- 界面：`lvgl-rust 0.3.1`（安全包装）+ `lvgl-rust-sys 0.3.1`（LVGL 9.6.0 的 C 源码构建期编译），
  配置用本仓自己的 `lvgl-config/lv_conf.h`
- 渲染：LVGL 软件渲染 → `Flush` 驱动逐行写并口，绘制缓冲 320×40 像素

## 各元素用什么画的

| 屏幕上的东西 | LVGL 元素 | 做法 |
|---|---|---|
| 唱片（盘面 / 纹路 / 四象限标签） | `lv_image` | 启动时把 132×132 的 RGB565 位图画进静态缓冲，`lv_image_set_rotation` 每帧改角度整块旋转，`lv_image_set_pivot` 定在盘心；盘外像素是纯黑，由 `image_colorkey` 抠成透明 |
| 唱片机基座 | `lv_obj` ×3 | (6,6) 起的 160×146 圆角板，留出上/左边距；竖向渐变 + 顶部高光条 + 底部暗边做厚度；唱片的盘外像素是透明的，基座的渐变能透上来 |
| 唱臂 + 支点 + 唱针头 | `lv_line` + 两个 `lv_obj` | 线段两个点每帧按播放进度改写（标签外沿的内环 → 盘边；盘心那段是标签，不是纹路），支点是 10×10 的圆，针头是 9×13 的方块跟着线段末端走 |
| 进度条 | `lv_bar` | 本首已播千分比，0–1000 |
| 歌词 | `lv_label` | 15 行一次建好，逐帧改 y 做滚动（一阶低通，不跳行）、改文字色做高亮，离当前行太远的直接 `lv_obj_set_hidden` |
| 上一首 / 暂停 / 下一首 | `lv_button` + `lv_label` | 图标用 LVGL 内置的 `LV_SYMBOL_PREV/PAUSE/NEXT`。屏没有触摸（XPT2046 没接线），纯装饰 |

播放列表、歌词、时间轴都在 `src/music.rs` 里，3 首歌共 36 秒循环。

## 第二页：客厅控制面板

一轮音乐（36 秒）播完插播 30 秒控制面板，然后切回音乐页循环。这页在 `src/home.rs`，是独立
一块 LVGL 屏幕（`Lvgl::create_screen` + `load_screen`），纯展示、值写死：

| 屏幕上的东西 | LVGL 元素 | 做法 |
|---|---|---|
| 卡片 | `lv_obj` | (6,6) 起 308×228 圆角深灰板，边框 1 px |
| 恒温圆弧 + 旋钮 | `lv_arc` | `set_bg_angles(135, 405)`、行程 62%，底轨深灰、行程亮蓝、线宽 10；旋钮是 `LV_PART_KNOB`（默认全透明，要自己给 `bg_opa` 与圆的半径） |
| 21.5° / Heating | `lv_label` | 28 号 montserrat（`LV_FONT_MONTSERRAT_28`）+ 14 号灰色副标题 |
| 灯光开关 | `lv_switch` | `add_state(LV_STATE_CHECKED)` 置为开，指示器亮蓝、旋钮白 |
| 亮度滑条 | `lv_slider` | 0–100 取 70，轨道深灰、指示器与旋钮亮蓝 |
| Evening scene | `lv_button` + `lv_label` | 亮蓝圆角按钮，文字居中 |

`home.rs` 只持有屏幕的所有权（`Owned<Obj>`，drop 时连屏幕一起删）和它的 `Handle`，其余控件
建完就不再引用。

## 构建：LVGL 是 C 库

这是本仓唯一需要 C 工具链的示例。**先起 shell**：

```bash
cd ili9341-lcd-lvgl
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
- 真正会被链接的 libc 符号只有 `strlen`/`strcmp`/`strcpy`/`strcat`，`lvgl-rust` 的
  `use-string-functions` 特性用 Rust 实现它们；
  `__assert_func` 由 `esp-rom-sys` 提供；`memcpy` 一类来自 compiler-builtins。所以不需要 libc.a。
- `lvgl-rust-sys` 的 `build.rs` 会把 Rust 三元组直接丢给 libclang，而 clang 只认 `riscv32-unknown-elf`，
  用 `BINDGEN_EXTRA_CLANG_ARGS_riscv32imac_unknown_none_elf` 再补一次 `--target`。

LVGL 的配置由本目录的 `lvgl-config/lv_conf.h` 提供，`.cargo/config.toml` 里用
`DEP_LV_CONFIG_PATH = { value = "lvgl-config", relative = true }` 指过去。这份配置在官方自带的那份上
改了三行（`lvgl-rust` 的句柄表硬要求，缺了会在构建期 `compile_error!`）：

```c
#define LV_USE_OBJ_ID 1
#define LV_OBJ_ID_AUTO_ASSIGN 1
#define LV_USE_OBJ_ID_BUILTIN 0
```

`.cargo/config.toml` 里的 `CPATH` 是**本机仓库的绝对路径**（`[env] relative` 在这里的 cargo 上没生效），
换机器要改这一行。`LIBCLANG_PATH` 由 `shell.nix` 的 `shellHook` 导出。

## 内存

| 用途 | 大小 |
|---|---|
| LVGL 自己的内存池（`lv_conf.h` 的 `LV_MEM_SIZE`） | 64 KB，静态 |
| LVGL 绘制缓冲（320×40，RGB565） | 25 KB，静态 |
| 唱片位图（132×132，RGB565） | 34 KB，静态 |
| 一行像素的转换缓冲（并口要 u16，LVGL 给字节流） | 640 B |

每个 LVGL 对象多 4 字节（`LV_USE_OBJ_ID` 的 id 字段）。`lvgl-rust` 的句柄表默认 32 槽，
两页一共要 33 个（音乐页 20 + 控制面板 13），所以在 `.cargo/config.toml` 里把
`LVGL_RUST_HANDLE_SLOTS` 提到 64——不改的话第二页建到一半会 `RegistryFull`。

不注册 `#[global_allocator]`：Rust 侧没有任何 `alloc`，LVGL 也只用它自己的池子。

## 注意

- 换字号要改 `lvgl-config/lv_conf.h` 的 `LV_FONT_MONTSERRAT_*`；默认只开了 montserrat 14。
- LVGL 9.6 的 `LV_COLOR_FORMAT_ARGB8565`（RGB565+alpha 三字节）软件渲染器还不支持，带透明的位图
  只能走 `RGB565A8` 这类分离平面格式；这页的圆盘是硬边，直接用 RGB565 + `image_colorkey` 更省
  内存（省 17 KB），透明度也等价。
- `lv_obj_set_width` 之后还要 `lv_obj_set_size` 固定行高，否则 label 按内容换行，两行歌词会叠在一起。
- 歌词行号用 Q8 定点做插值时，整数除法必须用 `div_euclid`（向下取整）；用 `/` 向零取整会让相邻
  两行同时算成「当前行」，两行文字重叠。
- LVGL 旋转是逐像素反算，唱片 132×132 每帧重画一遍；再大就要考虑降低帧率。
- 92 个控件句柄只在建页时拿一次，每帧 `Handle::get_mut(&mut lv)` 重新解析：`lvgl-rust` 的视图借用
  session，存不进 `Music` 结构体。每帧约 25 次解析，开销可以忽略。
