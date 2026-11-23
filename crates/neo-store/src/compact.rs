//! 阶段 6 的内存压缩原语：坐标差分、引用差分 varint、有序 ID 列压缩。
//!
//! 字符串驻留已经在 [`crate::DataSet::intern`] 里。这里补的是可以落盘或换出的
//! 紧凑编码，不替换在线 SoA（在线编辑仍用 `i32` 坐标和句柄）。

use neo_core::NeoError;

/// ZigZag + unsigned LEB128，适合小差分。
pub fn encode_delta_varints(values: &[i64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len());
    let mut prev = 0i64;
    for value in values {
        let delta = value.wrapping_sub(prev);
        prev = *value;
        write_leb(&mut out, zigzag(delta));
    }
    out
}

/// [`encode_delta_varints`] 的逆变换。
pub fn decode_delta_varints(bytes: &[u8]) -> Result<Vec<i64>, NeoError> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut prev = 0i64;
    while i < bytes.len() {
        let (n, used) = read_leb(&bytes[i..]).map_err(|e| NeoError::Parse(e.to_string()))?;
        i += used;
        let delta = unzigzag(n);
        let value = prev.wrapping_add(delta);
        prev = value;
        out.push(value);
    }
    Ok(out)
}

/// 经纬度（1e-7 度）按点差分压缩。纬度和经度各自相对上一点差分，避免交错后出现数亿的假差分。
pub fn encode_coords(coords: &[(i32, i32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(coords.len());
    let mut prev_lat = 0i64;
    let mut prev_lon = 0i64;
    for &(lat, lon) in coords {
        let lat = i64::from(lat);
        let lon = i64::from(lon);
        write_leb(&mut out, zigzag(lat.wrapping_sub(prev_lat)));
        write_leb(&mut out, zigzag(lon.wrapping_sub(prev_lon)));
        prev_lat = lat;
        prev_lon = lon;
    }
    out
}

/// [`encode_coords`] 的逆变换。奇数个 varint 视为损坏。
pub fn decode_coords(bytes: &[u8]) -> Result<Vec<(i32, i32)>, NeoError> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut prev_lat = 0i64;
    let mut prev_lon = 0i64;
    while i < bytes.len() {
        let (lat_n, lat_used) = read_leb(&bytes[i..]).map_err(NeoError::Parse)?;
        i += lat_used;
        if i >= bytes.len() {
            return Err(NeoError::Parse("coord stream has an odd length".into()));
        }
        let (lon_n, lon_used) = read_leb(&bytes[i..]).map_err(NeoError::Parse)?;
        i += lon_used;
        let lat = prev_lat.wrapping_add(unzigzag(lat_n));
        let lon = prev_lon.wrapping_add(unzigzag(lon_n));
        prev_lat = lat;
        prev_lon = lon;
        out.push((lat as i32, lon as i32));
    }
    Ok(out)
}

/// 升序 `u32` ID 列的差分 varint。
pub fn encode_sorted_ids(ids: &[u32]) -> Vec<u8> {
    let values: Vec<i64> = ids.iter().map(|id| i64::from(*id)).collect();
    encode_delta_varints(&values)
}

/// [`encode_sorted_ids`] 的逆变换。
pub fn decode_sorted_ids(bytes: &[u8]) -> Result<Vec<u32>, NeoError> {
    Ok(decode_delta_varints(bytes)?
        .into_iter()
        .map(|v| v as u32)
        .collect())
}

fn zigzag(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

fn unzigzag(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}

fn write_leb(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            byte |= 0x80;
            out.push(byte);
        } else {
            out.push(byte);
            break;
        }
    }
}

fn read_leb(bytes: &[u8]) -> Result<(u64, usize), String> {
    let mut n = 0u64;
    let mut shift = 0;
    for (i, byte) in bytes.iter().enumerate() {
        if shift >= 64 {
            return Err("varint too long".into());
        }
        n |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((n, i + 1));
        }
        shift += 7;
    }
    Err("truncated varint".into())
}
