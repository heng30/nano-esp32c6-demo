//! NMEA 0183 子集解析：GGA / RMC，对应上游 `nmea_parser` 组件。
//! 逐行无状态解析，UART 任务拼好一行后调 `parse_*`。

/// GGA 定位结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gga {
    /// UTC 时分秒
    pub hms: (u8, u8, u8),
    /// 纬度，十进制度，北正南负
    pub lat: f64,
    /// 经度，十进制度，东正西负
    pub lon: f64,
    /// 参与定位的卫星数
    pub sats: u8,
    /// 海拔，米
    pub alt_m: f32,
    /// quality 字段 > 0 才算有效定位
    pub valid: bool,
}

/// RMC 推荐最小定位信息
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rmc {
    /// UTC 时分秒
    pub hms: (u8, u8, u8),
    /// 状态位 A=有效 V=警告
    pub active: bool,
    /// 纬度，十进制度
    pub lat: f64,
    /// 经度，十进制度
    pub lon: f64,
    /// 地面速度，节
    pub speed_knots: f32,
    /// UTC 日期 ddmmyy
    pub date: u32,
}

/// 校验和：`$` 与 `*` 之间按字节异或；返回十六进制两位（小写）
fn checksum(payload: &str) -> u8 {
    payload.bytes().fold(0u8, |a, b| a ^ b)
}

/// 拆一行 NMEA：返回 (payload, 校验字节)；格式不对返回 None
fn split_line(line: &str) -> Option<(&str, Option<u8>)> {
    let body = line.strip_prefix('$')?;
    match body.split_once('*') {
        Some((payload, cs)) => Some((payload, Some(u8::from_str_radix(cs, 16).ok()?))),
        None => Some((body, None)),
    }
}

/// 解析 hhmmss(.sss) 时间字段
fn parse_hms(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() < 6 {
        return None;
    }
    Some((s[0..2].parse().ok()?, s[2..4].parse().ok()?, s[4..6].parse().ok()?))
}

/// 解析 ddmm.mmmm + 半球字母 → 带符号十进制度
fn parse_coord(value: &str, hemi: &str) -> Option<f64> {
    let dot = value.find('.').unwrap_or(value.len());
    // 度位数 = 小数点位置 - 2（分固定两位）
    if dot < 3 {
        return None;
    }
    let deg: f64 = value[..dot - 2].parse().ok()?;
    let min: f64 = value[dot - 2..].parse().ok()?;
    let mut out = deg + min / 60.0;
    match hemi {
        "S" | "W" => out = -out,
        "N" | "E" => {}
        _ => return None,
    }
    Some(out)
}

/// 解析 $xxGGA 行；校验和错误或字段缺失返回 None
pub fn parse_gga(line: &str) -> Option<Gga> {
    let (payload, cs) = split_line(line)?;
    if let Some(cs) = cs {
        if checksum(payload) != cs {
            return None;
        }
    }
    let mut f = payload.split(',');
    if !f.next()?.ends_with("GGA") {
        return None;
    }
    let hms = parse_hms(f.next()?)?;
    let lat = parse_coord(f.next()?, f.next()?)?;
    let lon = parse_coord(f.next()?, f.next()?)?;
    // quality 字段存在但可能为空串：空串按无定位处理，不算解析失败
    let quality: u8 = f.next()?.parse().unwrap_or(0);
    let sats: u8 = f.next()?.parse().unwrap_or(0);
    // hdop 不用，跳过
    let _ = f.next()?;
    let alt_m: f32 = f.next()?.parse().unwrap_or(0.0);
    Some(Gga { hms, lat, lon, sats, alt_m, valid: quality > 0 })
}

/// 解析 $xxRMC 行；校验和错误返回 None
pub fn parse_rmc(line: &str) -> Option<Rmc> {
    let (payload, cs) = split_line(line)?;
    if let Some(cs) = cs {
        if checksum(payload) != cs {
            return None;
        }
    }
    let mut f = payload.split(',');
    if !f.next()?.ends_with("RMC") {
        return None;
    }
    let hms = parse_hms(f.next()?)?;
    let active = f.next()? == "A";
    let lat = parse_coord(f.next()?, f.next()?)?;
    let lon = parse_coord(f.next()?, f.next()?)?;
    let speed_knots: f32 = f.next()?.parse().unwrap_or(0.0);
    // course 不用，跳过
    let _ = f.next()?;
    let date: u32 = f.next()?.parse().unwrap_or(0);
    Some(Rmc { hms, active, lat, lon, speed_knots, date })
}

#[cfg(test)]
mod tests {
    // 测试用 format!/to_string，宿主构建下引入 std
    extern crate std;

    use crate::nmea::tests::std::string::ToString;
    use crate::nmea::tests::std::format;
    use super::*;

    /// 维基百科标准示例句
    const GGA_OK: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
    const RMC_OK: &str = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,074.2,230394,003.1,W*63";

    #[test]
    fn gga_known_sentence() {
        let g = parse_gga(GGA_OK).unwrap();
        assert_eq!(g.hms, (12, 35, 19));
        assert!((g.lat - 48.1173).abs() < 1e-3, "lat {}", g.lat);
        assert!((g.lon - 11.5166).abs() < 1e-3, "lon {}", g.lon);
        assert_eq!(g.sats, 8);
        assert!((g.alt_m - 545.4).abs() < 0.1);
        assert!(g.valid);
    }

    #[test]
    fn gga_no_fix_invalid() {
        // quality=0：能解析但 valid=false
        let nofix = "$GPGGA,123519,4807.038,N,01131.000,E,0,00,,,,M,,*5f";
        // 该句校验和是编的，先验一下函数是否吃：应因校验失败返回 None
        assert!(parse_gga(nofix).is_none());
        // 构造 quality=0 且校验正确的句子
        let payload = "GPGGA,123519,4807.038,N,01131.000,E,0,00,,,,M,,";
        let cs = checksum(payload);
        let line = format!("${}*{:02x}", payload, cs);
        let g = parse_gga(&line).unwrap();
        assert!(!g.valid);
    }

    #[test]
    fn bad_checksum_rejected() {
        let mut bad = GGA_OK.to_string();
        // 把校验字节改错
        bad.pop();
        bad.push('0');
        assert!(parse_gga(&bad).is_none());
        assert!(parse_rmc(RMC_OK.replace("*63", "*00").as_str()).is_none());
    }

    #[test]
    fn rmc_known_sentence() {
        let r = parse_rmc(RMC_OK).unwrap();
        assert_eq!(r.hms, (12, 35, 19));
        assert!(r.active);
        assert!((r.lat - 48.1173).abs() < 1e-3);
        assert!((r.lon - 11.5166).abs() < 1e-3);
        assert!((r.speed_knots - 22.4).abs() < 0.1);
        assert_eq!(r.date, 230394);
    }

    #[test]
    fn southern_western_negative() {
        let payload = "GPGGA,123519,4807.038,S,01131.000,W,1,08,0.9,545.4,M,46.9,M,,";
        let cs = checksum(payload);
        let line = format!("${}*{:02x}", payload, cs);
        let g = parse_gga(&line).unwrap();
        assert!(g.lat < 0.0);
        assert!(g.lon < 0.0);
    }

    #[test]
    fn non_nmea_line_rejected() {
        assert!(parse_gga("hello world").is_none());
        assert!(parse_rmc("$GPGGA,123519,4807.038,N*00").is_none());
    }
}
