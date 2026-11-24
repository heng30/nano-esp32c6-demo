# kk-ui-rs 专用：按 examples/sim 里用到的字符生成字库，再把画布交给模拟器跑。
#
# 路径一律按本文件所在目录解析，所以 `make -C kk-ui-rs` 或在别处 `make -f` 都能用；
# 包用 `--manifest-path` 指定，不依赖当前目录，也不依赖 `cargo install` 装到哪。
#
#   make                生成字库 + 开窗口跑模拟器
#   make font           只生成 examples/sim/assets/font/{font,width,cp}.bin 与 text.txt
#   make snapshot       无头截图（SNAPSHOT=/tmp/sim.png STEPS=40）
#   make TOOL=kkui-tool 用装好的工具，不现场构建

ROOT := $(patsubst %/,%,$(dir $(abspath $(lastword $(MAKEFILE_LIST)))))
MANIFEST := $(ROOT)/Cargo.toml

DIR ?= examples/sim
OUT ?= $(DIR)/assets/font

TOOL ?= cargo run --quiet --manifest-path $(MANIFEST) --bin kkui-tool --features tool --
SIM := cargo run --quiet --manifest-path $(MANIFEST) --example sim --features sim
SNAPSHOT ?= /tmp/sim.png
STEPS ?= 40

.PHONY: all font run snapshot

all: run

font:
	cd $(ROOT) && $(TOOL) font $(DIR) --out $(OUT)

# 字库是编译期 include_bytes! 进来的，所以必须先有文件。
run: font
	cd $(ROOT) && $(SIM)

snapshot: font
	cd $(ROOT) && $(SIM) -- --snapshot $(SNAPSHOT) --steps $(STEPS)
