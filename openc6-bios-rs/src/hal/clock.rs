//! CPU 频率控制，直接改写 PCR 分频寄存器。

use super::regs::{modify, read};

/// PCR 里的 CPU 频率配置寄存器（TRM 8.4）。
const PCR_CPU_FREQ_CONF: usize = 0x6009_6118;
/// `div_num` 字段（bit 8..16）。
const DIV_MASK: u32 = 0x1FF << 8;
/// `div_num` 的位偏移。
const HS_DIV_SHIFT: u32 = 8;
/// 强制 120 MHz 的开关位。
const FORCE_120M: u32 = 1 << 16;

/// 低档频率。
pub const LOW_MHZ: u32 = 80;
/// 中档频率。
pub const MID_MHZ: u32 = 120;
/// 高档频率。
pub const HIGH_MHZ: u32 = 160;

/// 从硅片寄存器直接读出当前 HP CPU 频率。
pub fn get_cpu_mhz() -> u32 {
    let pcr = read(PCR_CPU_FREQ_CONF);
    if pcr & FORCE_120M != 0 {
        MID_MHZ
    } else if (pcr >> HS_DIV_SHIFT) & 0xFF == 1 {
        LOW_MHZ
    } else {
        HIGH_MHZ
    }
}

/// 直接改写分频器。
///
/// 160 MHz 是 div=0、force=0；120 MHz 靠 force 位；80 MHz 是 div=1。
pub fn set_cpu_mhz_raw(freq_mhz: u32) {
    modify(PCR_CPU_FREQ_CONF, |reg| {
        let mut reg = reg & !DIV_MASK;
        if freq_mhz >= HIGH_MHZ {
            // 160 MHz：div_num = 0，force_120m = 0
        } else if freq_mhz >= MID_MHZ {
            reg |= FORCE_120M;
        } else {
            reg |= 1 << HS_DIV_SHIFT;
        }
        reg
    });
}
