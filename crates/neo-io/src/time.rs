//! RFC 3339 / ISO-8601 UTC timestamps for OSM XML.
//!
//! Internally the OSM data model uses Unix seconds ([`neo_core::Meta::timestamp`]), while OSM XML 0.6
//! attributes are `YYYY-MM-DDTHH:MM:SSZ`. Writing integers straight into XML produces **invalid OSM XML**,
//! and reading real files parses them to 0 (metadata silently lost). This module provides a dependency-free
//! two-way conversion (UTC, second precision, fractional seconds accepted on read).

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_timestamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let seconds_of_day = unix_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `YYYY-MM-DDTHH:MM:SSZ` (with optional fractional seconds) → Unix seconds.
///
/// Strictly validates the range and separators; any invalid input returns `None` (callers should report a parse
/// error rather than silently treating it as 0).
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    // Shortest `1970-01-01T00:00:00Z` = 20 bytes.
    if bytes.len() < 20 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    if !matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    if bytes.get(13) != Some(&b':') || bytes.get(16) != Some(&b':') {
        return None;
    }
    let hour: u32 = text.get(11..13)?.parse().ok()?;
    let minute: u32 = text.get(14..16)?.parse().ok()?;
    let second: u32 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day > max_day {
        return None;
    }
    // Skip fractional seconds.
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(|b| b.is_ascii_digit()) {
            index += 1;
        }
        if index == start {
            return None;
        }
    }
    match bytes.get(index) {
        Some(b'Z' | b'z') => {}
        // Only accept UTC in the form `+00:00` / `-00:00` (OSM always uses Z).
        Some(b'+') | Some(b'-') => {
            let offset = text.get(index + 1..index + 6)?;
            if offset != "00:00" {
                return None;
            }
            index += 6;
        }
        _ => return None,
    }
    if index + 1 != bytes.len() {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second),
    )
}

/// Gregorian date → days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970-01-01 → Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_known_epochs() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2024-02-29T12:34:56Z"), Some(1_709_210_096));
        assert_eq!(format_timestamp(1_709_210_096), "2024-02-29T12:34:56Z");
        assert_eq!(parse_timestamp("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_timestamp("0"), None);
        assert_eq!(parse_timestamp("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_timestamp("2024-02-30T00:00:00Z"), None);
        assert_eq!(parse_timestamp("2024-01-01 00:00:00Z"), Some(1_704_067_200));
        assert_eq!(parse_timestamp("2024-01-01T00:00:00+05:00"), None);
    }
}
