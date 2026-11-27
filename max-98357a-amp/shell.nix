{
  pkgs ? import <nixpkgs> { },
}:

pkgs.mkShell {
  buildInputs = [
    pkgs.pkgsCross.riscv32-embedded.buildPackages.gcc
    pkgs.pkgsCross.riscv32-embedded.buildPackages.binutils
  ];
}
