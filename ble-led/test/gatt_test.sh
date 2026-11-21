#!/usr/bin/env bash
# ble-led 的 BLE 控制闭环测试：发现设备、连上、往 0x2BE2 写 on/off、读回校验。
#
# 全部走 busctl 直接调 BlueZ 的 D-Bus 接口：bluetoothctl 的扫描/连接跨进程不可靠
# （扫描进程退出后设备对象会被清掉，再 connect 报 "not available"）。
#
# 依赖主机 BlueZ 5.x 的 busctl，板子已烧好 ble-led 并在广播。
# 用法:
#   ./test/gatt_test.sh              # 按广播名找设备
#   ./test/gatt_test.sh 42:4C:CA:5F:CA:15
#
# 退出码 0 表示四项断言全过；任何一项失败立刻退出非 0。

set -euo pipefail

# 广播名，也是默认的查找依据
TARGET="${1:-ble-led}"
# 灯输出特性 UUID（蓝牙 SIG 的 Light Output）的短号
CHAR_UUID_MATCH="2be2"

die() { echo "✗ $*" >&2; exit 1; }

ADAPTER=$(busctl tree org.bluez 2>/dev/null | grep -oE '/org/bluez/hci[0-9]+' | head -1) ||
    die "找不到蓝牙适配器，主机蓝牙没开？"

stop_discovery() {
    busctl call org.bluez "$ADAPTER" org.bluez.Adapter1 StopDiscovery 2>/dev/null || true
    busctl call org.bluez "$ADAPTER" org.bluez.Adapter1 RemoveDiscoveryFilter 2>/dev/null || true
}
trap stop_discovery EXIT

# 必须显式限定 LE：默认发现是 BR/EDR + LE 混合，抓不到只广播 LE 的外设
start_discovery() {
    busctl call org.bluez "$ADAPTER" org.bluez.Adapter1 SetDiscoveryFilter 'a{sv}' 1 Transport s "le"
    busctl call org.bluez "$ADAPTER" org.bluez.Adapter1 StartDiscovery
}

# 列出所有已发现设备的 D-Bus 路径
dev_paths() {
    busctl tree org.bluez 2>/dev/null | grep -oE '/org/bluez/hci[0-9]+/dev_[^ ]*' | sort -u
}

# 按广播名找设备路径；找不到就扫一轮
find_dev() {
    local path name
    for path in $(dev_paths); do
        name=$(busctl get-property org.bluez "$path" org.bluez.Device1 Name 2>/dev/null) || continue
        if [[ $name == *"\"$TARGET\""* ]]; then
            echo "$path"
            return 0
        fi
    done
    return 1
}

# 入参本身就是 MAC 时直接拼路径
mac_to_path() {
    echo "/org/bluez/hci0/dev_${1//:/_}"
}

DEV=""
if [[ -n $(find_dev || true) ]]; then
    DEV=$(find_dev)
else
    echo "扫描 $TARGET（最多 30 秒）…" >&2
    start_discovery
    for _ in $(seq 1 60); do
        if DEV=$(find_dev); then break; fi
        sleep 0.5
    done
    stop_discovery
    if [[ -z $DEV ]] && [[ $TARGET =~ ^([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}$ ]]; then
        DEV=$(mac_to_path "$TARGET")
    fi
fi
[[ -n $DEV ]] || die "扫描不到名为 \"$TARGET\" 的设备，先确认板子已烧录并在广播"
echo "设备 $DEV"

# 在 bluez 的 GATT 树里找 UUID 为 0x2BE2 的特性路径
find_char() {
    local path uuid
    for path in $(busctl tree org.bluez 2>/dev/null | grep -oE "${DEV}/service[0-9]+/char[0-9]+" | sort -u); do
        uuid=$(busctl get-property org.bluez "$path" org.bluez.GattCharacteristic1 UUID 2>/dev/null) || continue
        if [[ $uuid == *"$CHAR_UUID_MATCH"* ]]; then
            echo "$path"
            return 0
        fi
    done
    return 1
}

is_connected() { [[ $(busctl get-property org.bluez "$DEV" org.bluez.Device1 Connected) == "b true" ]]; }

# 把 ASCII 字符串逐字节写进特性
write_text() {
    local -a bytes=()
    local ch
    for ((i = 0; i < ${#1}; i++)); do
        ch=${1:i:1}
        bytes+=("$(printf '%d' "'$ch")")
    done
    busctl call org.bluez "$CHAR" org.bluez.GattCharacteristic1 WriteValue \
        aya{sv} "${#bytes[@]}" "${bytes[@]}" 0
}

# 读特性值，把 ay 的字节还原成文本
read_text() {
    busctl call org.bluez "$CHAR" org.bluez.GattCharacteristic1 ReadValue a{sv} 0 |
        awk '{ for (i = 3; i <= NF; i++) printf "%c", $i }'
}

expect() {
    local want=$1 got=$2 what=$3
    if [[ $got != "$want" ]]; then
        die "$what：期望 \"$want\"，实际 \"$got\""
    fi
    echo "✓ $what = \"$got\""
}

if is_connected; then
    echo "已连接，跳过 connect"
else
    busctl call org.bluez "$DEV" org.bluez.Device1 Connect >/dev/null || true
    for _ in $(seq 1 20); do
        is_connected && break
        sleep 0.5
    done
    is_connected || die "连接超时（板子被别的中心占着？CONNECTIONS_MAX = 1）"
fi

CHAR=""
for _ in $(seq 1 20); do
    if CHAR=$(find_char); then break; fi
    sleep 0.5
done
[[ -n $CHAR ]] || die "设备上没有 0x2BE2 特性，检查服务是否起全"
echo "特性 $CHAR"

write_text on
expect on "$(read_text)" "写 on 后读回"

write_text off
expect off "$(read_text)" "写 off 后读回"

# 非法值必须被拒（服务端 reject），且不污染特性值
if write_text xyz 2>/dev/null; then
    die "非法值 xyz 竟然被接受"
fi
echo "✓ 非法值 xyz 被拒绝"
expect off "$(read_text)" "被拒后状态未变"

busctl call org.bluez "$DEV" org.bluez.Device1 Disconnect >/dev/null 2>&1 || true
echo "全部通过"
