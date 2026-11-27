//! 够用的 AVI（RIFF）解析：只取视频帧、画面尺寸与帧间隔。
//!
//! 只认 ffmpeg 默认写出的这种结构：`LIST hdrl` 里的 `avih` 给尺寸与每帧微秒数，
//! `LIST movi` 里的 `<流号>dc` 按拍摄顺序逐帧放 MJPEG。`idx1` 索引块不读。

use embassy_time::Duration;

/// RIFF chunk 迭代器：`[ID; 4][长度; 4][内容][奇数长度补 1 字节]`。
struct Chunks<'a> {
    /// 尚未消费的剩余字节。
    rest: &'a [u8],
}

impl<'a> Iterator for Chunks<'a> {
    type Item = ([u8; 4], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let id: [u8; 4] = self.rest.get(..4)?.try_into().ok()?;
        let len = u32::from_le_bytes(self.rest.get(4..8)?.try_into().ok()?) as usize;
        let body = self.rest.get(8..8 + len)?;
        self.rest = self.rest.get(8 + len + (len & 1)..).unwrap_or_default();
        Some((id, body))
    }
}

/// 找第一个 `LIST <want>`（返回列表类型之后的内容）或裸 `<want>`，都只看本层。
fn find(data: &[u8], want: [u8; 4]) -> Option<&[u8]> {
    for (id, body) in (Chunks { rest: data }) {
        if id == *b"LIST" {
            if let Some((kind, rest)) = body.split_at_checked(4)
                && kind == want
            {
                return Some(rest);
            }
        } else if id == want {
            return Some(body);
        }
    }
    None
}

/// 逐帧的视频数据切片。
pub struct Frames<'a> {
    /// 正在走的 `movi` 内容区。
    chunks: Chunks<'a>,
}

impl<'a> Iterator for Frames<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        // 视频帧块名形如 `00dc`；音频块是 `01wb`，按后缀排除
        self.chunks
            .by_ref()
            .find(|(id, _)| id[0].is_ascii_digit() && id[1].is_ascii_digit() && &id[2..] == b"dc")
            .map(|(_, body)| body)
    }
}

/// 解析结果，固件里只读。
pub struct Avi<'a> {
    /// `movi` 列表的内容区。
    movi: &'a [u8],
    /// 画面宽（像素）。
    width: u16,
    /// 画面高（像素）。
    height: u16,
    /// 每帧应占的时间（来自 `avih` 的 `dwMicroSecPerFrame`）。
    frame_delay: Duration,
}

impl<'a> Avi<'a> {
    /// 解析头部；不是 AVI 或缺少 `avih` / `movi` 时返回 `None`。
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.get(..4)? != b"RIFF" || data.get(8..12)? != b"AVI " {
            return None;
        }

        let avih = find(find(data.get(12..)?, *b"hdrl")?, *b"avih")?;
        let dword = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(avih.get(at..at + 4)?.try_into().ok()?))
        };

        Some(Self {
            movi: find(data.get(12..)?, *b"movi")?,
            width: u16::try_from(dword(32)?).ok()?,
            height: u16::try_from(dword(36)?).ok()?,
            frame_delay: Duration::from_micros(u64::from(dword(0)?)),
        })
    }

    /// 画面宽（像素）。
    pub fn width(&self) -> u16 {
        self.width
    }

    /// 画面高（像素）。
    pub fn height(&self) -> u16 {
        self.height
    }

    /// 每帧应占的时间。
    pub fn frame_delay(&self) -> Duration {
        self.frame_delay
    }

    /// 从头开始逐帧迭代，不占内存。
    pub fn frames(&self) -> Frames<'a> {
        Frames {
            chunks: Chunks { rest: self.movi },
        }
    }
}
