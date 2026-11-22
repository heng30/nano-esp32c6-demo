# unifont.bdf

仓库公共资源：GNU Unifont 17.0.04 的 16×16 点阵字库，`ili9341-lcd/build.rs`
从里面抠 GB2312 汉字字模。放根目录是为了以后别的示例也能复用。

- 上游：<https://unifoundry.com/unifont/>
- 许可：SIL Open Font License 1.1 与 GPLv2+（附 GNU Font Embedding Exception）双许可，
  完整声明见文件头 `COPYRIGHT` 属性。
- 文件原样拷贝，未做裁剪；改字库请连同版本号一起更新本文件。

# DejaVuSans.ttf

构建期必需资源：`ili9341-lcd-slint/build.rs` 调用 Slint 编译器时，裸机没有系统字体可找，
必须在 `.slint` 里 `import` 一个 TTF，由它在构建期栅格化成位图字形。

- 上游：DejaVu 字体 2.37（取自 `dejavu-fonts-minimal`）
- 许可：Bitstream Vera 字体许可（允许再分发与修改），完整声明见 TTF 内的 `name` 表
- 只覆盖拉丁/希腊/西里尔，**不含汉字**；中文要走点阵位图，见 `ili9341-lcd-slint/README.md`
- 文件原样拷贝，未做子集化（741 KB；子集化后约几十 KB，但会绑死界面里用到的字符集）
