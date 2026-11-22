//! 把 `ui/main.slint` 编译成 Rust 代码。界面资源在构建期展开，运行时不读文件。

fn main() {
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new()
            .with_edition((1, 19))
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .expect("slint ui compile failed");

    slint_build::print_rustc_flags().expect("slint rustc flags failed");
}
