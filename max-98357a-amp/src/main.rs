#![no_std]
#![no_main]

use defmt::info;
use esp_hal::{
    Blocking,
    clock::CpuClock,
    dma::DmaTxBuf,
    dma_tx_buffer,
    i2s::master::{Channels, DataFormat, I2s, I2sTx, TdmConfig, UnitConfig},
    time::Rate,
};
use panic_rtt_target as _;
use threepm::easy_mode::EasyMode;

esp_bootloader_esp_idf::esp_app_desc!();

/// 内嵌的测试音频：44.1 kHz / 单声道 / 16 bit PCM（8.7 s，750 KB）。
const WAV: &[u8] = include_bytes!("../../assets/videos/test.wav");
/// 内嵌的测试音频：44.1 kHz / 单声道 / 64 kbps CBR MP3（4 分 11 秒，2.0 MB，带封面）。
const MP3: &[u8] = include_bytes!("../../assets/videos/test.mp3");
/// DMA 单次搬运的字节数，64 KB ≈ 0.74 s 音频。
const CHUNK: usize = 64 * 1024;
/// 喂给 ThreePM 的分片大小，与它内部 2048 字节的环形缓冲对齐。
const FEED: usize = 512;
/// 一个 MP3 帧最多 1152 样本/声道 × 2 声道 × 2 字节。
const MAX_FRAME_BYTES: usize = 1152 * 2 * 2;

/// 从 WAV 头里取出（采样率、声道数、PCM 数据）；只支持 16 bit PCM。
fn parse_wav(wav: &[u8]) -> (u32, u16, &[u8]) {
    assert!(
        &wav[0..4] == b"RIFF" && &wav[8..12] == b"WAVE",
        "not a RIFF/WAVE file"
    );

    let mut fmt = None;
    let mut at = 12;
    while at + 8 <= wav.len() {
        let id = &wav[at..at + 4];
        let size = u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &wav[at + 8..(at + 8 + size).min(wav.len())];

        if id == b"fmt " {
            let channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            assert!(bits == 16, "only 16-bit PCM is supported");
            fmt = Some((rate, channels));
        } else if id == b"data" {
            let (rate, channels) = fmt.expect("data chunk before fmt chunk");
            return (rate, channels, body);
        }

        at += 8 + size + (size & 1);
    }

    panic!("WAV has no data chunk");
}

/// 跳过 ID3v2 标签，返回音频数据的起始偏移。
///
/// ffmpeg 写出来的标签能有几十 KB（`test.mp3` 里塞了歌词，69 686 字节），比 ThreePM 内部
/// 2048 字节的环缓冲大得多。不先跳掉，`mp3_decode_ready` 会为了慢慢吃掉标签反复返回
/// false，调用方很容易误判成「数据耗尽」。
fn skip_id3(mp3: &[u8]) -> usize {
    if mp3.len() < 10 || &mp3[0..3] != b"ID3" {
        return 0;
    }

    let size = ((mp3[6] as usize) << 21)
        | ((mp3[7] as usize) << 14)
        | ((mp3[8] as usize) << 7)
        | (mp3[9] as usize);
    let footer = if mp3[5] & 0x10 != 0 { 10 } else { 0 };
    (10 + size + footer).min(mp3.len())
}

/// 解出第一帧的格式，供 I2S 配置采样率与声道数。
fn mp3_format() -> (u32, u8) {
    let audio = &MP3[skip_id3(MP3)..];
    let mut decoder = EasyMode::new();
    let mut pos = 0;

    loop {
        while decoder.buffer_free() >= FEED && pos < audio.len() {
            let end = (pos + FEED).min(audio.len());
            let n = decoder.add_data(&audio[pos..end]);
            if n == 0 {
                break;
            }
            pos += n;
        }
        if decoder.mp3_decode_ready() {
            break;
        }
        assert!(
            pos < audio.len() || decoder.buffer_used() > 0,
            "mp3 has no decodable frame"
        );
    }

    let frame = decoder.mp3_info().expect("mp3 frame header parse failed");
    info!(
        "mp3: {} Hz, {} ch, {} kbps, {} bytes after {} bytes of ID3",
        frame.samprate,
        frame.nChans,
        frame.bitrate,
        audio.len(),
        skip_id3(MP3)
    );
    (frame.samprate as u32, frame.nChans as u8)
}

#[esp_hal::main]
fn main() -> ! {
    rtt_target::rtt_init_defmt!();

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    let (wav_rate, wav_channels, pcm) = parse_wav(WAV);
    let (mp3_rate, mp3_channels) = mp3_format();
    info!(
        "wav: {} Hz, {} ch, {} bytes PCM",
        wav_rate,
        wav_channels,
        pcm.len()
    );
    // 采样率写死在 I2S 构造时的时钟分频里，两段必须一致；声道数可以每段重配
    assert!(
        wav_rate == mp3_rate,
        "wav and mp3 sample rates differ, I2S would have to be rebuilt"
    );
    info!("mp3: {} Hz, {} ch", mp3_rate, mp3_channels);

    let mut buf = dma_tx_buffer!(CHUNK).expect("dma tx buffer allocation failed");
    let mut spare = dma_tx_buffer!(CHUNK).expect("dma tx buffer allocation failed");

    let i2s = I2s::new(
        peripherals.I2S0,
        peripherals.DMA_CH0,
        TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(wav_rate))
            .with_channels(if wav_channels == 1 {
                Channels::MONO
            } else {
                Channels::STEREO
            })
            .with_data_format(DataFormat::Data16Channel16),
    )
    .expect("i2s init failed");

    // MAX98357A: BCLK=GPIO4, LRC=GPIO5, DIN=GPIO6
    let mut tx = i2s
        .i2s_tx
        .with_bclk(peripherals.GPIO4)
        .with_ws(peripherals.GPIO5)
        .with_dout(peripherals.GPIO6)
        .build();

    let mut round = 0u32;
    loop {
        configure(&mut tx, wav_rate, wav_channels);
        info!("round {}: wav", round);
        (tx, buf) = play_wav(tx, buf, pcm);

        configure(&mut tx, mp3_rate, mp3_channels as u16);
        info!("round {}: mp3", round);
        (tx, buf, spare) = play_mp3(tx, buf, spare);

        round += 1;
    }
}

/// 按这一段音频的格式重配 I2S TX 单元（单声道与立体声的 slot 排布不同）。
fn configure(tx: &mut I2sTx<'_, Blocking>, rate: u32, channels: u16) {
    let config = UnitConfig::new_tdm_philips()
        .with_sample_rate(Rate::from_hz(rate))
        .with_channels(if channels == 1 {
            Channels::MONO
        } else {
            Channels::STEREO
        })
        .with_data_format(DataFormat::Data16Channel16);
    tx.apply_config(&config).expect("i2s apply_config failed");
}

/// 把 WAV 的 PCM 数据按块送进 I2S，播完一轮返回。
fn play_wav<'d>(
    mut tx: I2sTx<'d, Blocking>,
    mut buf: DmaTxBuf,
    pcm: &[u8],
) -> (I2sTx<'d, Blocking>, DmaTxBuf) {
    for chunk in pcm.chunks(CHUNK) {
        buf.fill(chunk);
        let (result, back, restored) = tx.write(buf).expect("i2s write failed").wait();
        result.expect("i2s dma transfer failed");
        tx = back;
        buf = restored;
    }

    (tx, buf)
}

/// 解一块 MP3 到 `buf`，返回写入的字节数；没有更多数据时返回 `None`。
fn decode_chunk(
    decoder: &mut EasyMode,
    pcm: &mut [i16],
    audio: &[u8],
    pos: &mut usize,
    buf: &mut DmaTxBuf,
) -> Option<usize> {
    let mut filled = 0;
    while filled + MAX_FRAME_BYTES <= CHUNK {
        while decoder.buffer_free() >= FEED && *pos < audio.len() {
            let end = (*pos + FEED).min(audio.len());
            let n = decoder.add_data(&audio[*pos..end]);
            if n == 0 {
                break;
            }
            *pos += n;
        }

        // 对帧头阶段也会返回 false，只要还有数据就继续喂
        if !decoder.mp3_decode_ready() {
            if *pos >= audio.len() && decoder.buffer_used() == 0 {
                break;
            }
            continue;
        }

        let Ok(samples) = decoder.decode(pcm) else {
            break;
        };

        let target = &mut buf.as_mut_slice()[filled..filled + samples * 2];
        for (dst, src) in target
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(pcm[..samples].iter())
        {
            *dst = src.to_le_bytes();
        }
        filled += samples * 2;
    }

    if filled == 0 { None } else { Some(filled) }
}

/// 双缓冲播 MP3：DMA 播一块的同时解下一块，避免解码时间变成静音空档。
fn play_mp3<'d>(
    mut tx: I2sTx<'d, Blocking>,
    mut playing: DmaTxBuf,
    mut spare: DmaTxBuf,
) -> (I2sTx<'d, Blocking>, DmaTxBuf, DmaTxBuf) {
    let audio = &MP3[skip_id3(MP3)..];
    let mut decoder = EasyMode::new();
    let mut pcm = [0i16; MAX_FRAME_BYTES / 2];
    let mut pos = 0usize;

    let Some(filled) = decode_chunk(&mut decoder, &mut pcm, audio, &mut pos, &mut playing) else {
        return (tx, playing, spare);
    };
    playing.set_length(filled);
    let mut transfer = tx.write(playing).expect("i2s write failed");

    loop {
        // 正在播 playing，同时把下一块解进空闲的 spare
        let next = decode_chunk(&mut decoder, &mut pcm, audio, &mut pos, &mut spare);

        let (result, back, done) = transfer.wait();
        result.expect("i2s dma transfer failed");
        tx = back;
        playing = done;

        let Some(filled) = next else {
            return (tx, playing, spare);
        };

        spare.set_length(filled);
        core::mem::swap(&mut playing, &mut spare);
        transfer = tx.write(playing).expect("i2s write failed");
    }
}
