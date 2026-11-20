//! 电源微调与调频策略（AI Tweaker）。

use crate::{
    hal::clock,
    nvram::{self, CpuGovernor, CpuFreq},
};

/// 动态调频的采样周期。
const SAMPLE_MS: u64 = 250;
/// 降频前必须持续空闲的时间。
const DOWNSCALE_HOLD_MS: u64 = 1000;
/// 判定为「忙」的空闲堆阈值（字节），低于它说明有活干。
const BUSY_FREE_HEAP: usize = 8 * 1024;

/// 把 NVRAM 里的 AI Tweaker 设置应用到 SoC。
pub fn apply_bios_settings() {
    let cfg = nvram::get();

    // BOD 档位：上游通过 brownout_hal 配阈值，这里只记录选择，
    // 因为 esp-hal 的欠压检测由 bootloader 阶段固定，运行期改不了。
    defmt::info!("[TWEAKER] BOD 档位: {:?}", cfg.bod_level);

    let target = match cfg.cpu_freq {
        CpuFreq::Mhz80 => clock::LOW_MHZ,
        CpuFreq::Mhz120 => clock::MID_MHZ,
        CpuFreq::Mhz160 => clock::HIGH_MHZ,
    };
    clock::set_cpu_mhz_raw(target);
    defmt::info!("[TWEAKER] CPU 锁定在 {} MHz", target);
}

/// 动态调频任务：按堆占用粗估负载，滞回升降频。
#[embassy_executor::task]
pub async fn governor_task() {
    let mut last_busy = esp_hal::time::Instant::now();
    let mut current = clock::HIGH_MHZ;

    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_millis(SAMPLE_MS)).await;

        if nvram::get().cpu_governor != CpuGovernor::Dynamic {
            continue;
        }

        let busy = esp_alloc::HEAP.free() < BUSY_FREE_HEAP;
        let max = match nvram::get().cpu_freq {
            CpuFreq::Mhz80 => clock::LOW_MHZ,
            CpuFreq::Mhz120 => clock::MID_MHZ,
            CpuFreq::Mhz160 => clock::HIGH_MHZ,
        };

        if busy {
            last_busy = esp_hal::time::Instant::now();
            if current < max {
                // 负载尖峰立即升频
                current = max;
                clock::set_cpu_mhz_raw(current);
            }
        } else if current > clock::LOW_MHZ
            && esp_hal::time::Instant::now() - last_busy
                > esp_hal::time::Duration::from_millis(DOWNSCALE_HOLD_MS)
        {
            current = clock::LOW_MHZ;
            clock::set_cpu_mhz_raw(current);
        }
    }
}
