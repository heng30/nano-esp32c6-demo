{
  pkgs ? import <nixpkgs> { },
}:

# LVGL 是 C 库：这里给交叉 C 编译器（nix 的 riscv32 交叉 GCC 是 nolibc 的，libc 头
# 由 src/cshim/include 顶替）、binutils，以及 bindgen 需要的 libclang。
pkgs.mkShell {
  buildInputs = [
    pkgs.llvmPackages.libclang
    pkgs.pkgsCross.riscv32-embedded.buildPackages.gcc
    pkgs.pkgsCross.riscv32-embedded.buildPackages.binutils
  ];

  shellHook = ''
    export LIBCLANG_PATH=${pkgs.llvmPackages.libclang.lib}/lib
  '';
}
