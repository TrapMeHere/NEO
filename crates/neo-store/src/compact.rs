//! Phase 6 in-memory compression primitives: coordinate deltas, reference-delta varints, sorted ID column compression.
//!
//! String interning already lives in [`crate::DataSet::intern`]. What is added here is a compact encoding
//! that can be persisted or paged out; it does not replace the online SoA (in-memory editing still uses `i32` coordinates and handles).

use neo_core::NeoError;

/// ZigZag + unsigned LEB128, suitable for small deltas.
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

/// Inverse of [`encode_delta_varints`].
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

/// Delta-compress lat/lon (1e-7 degree) per point. Latitude and longitude are each differenced against the
/// previous point, avoiding hundreds-of-millions false deltas from interleaving.
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

/// Inverse of [`encode_coords`]. An odd number of varints is treated as corruption.
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

/// Delta varints for an ascending `u32` ID column.
pub fn encode_sorted_ids(ids: &[u32]) -> Vec<u8> {
    let values: Vec<i64> = ids.iter().map(|id| i64::from(*id)).collect();
    encode_delta_varints(&values)
}

/// Inverse of [`encode_sorted_ids`].
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
