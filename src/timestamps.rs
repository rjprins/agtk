//! UTC timestamps written by agent logs and Azure DevOps.

/// Parses an ISO UTC timestamp into Unix milliseconds, accepting Z or +00:00.
pub(crate) fn parse_utc_millis(value: &[u8]) -> Option<u64> {
    let value = value
        .strip_suffix(b"Z")
        .or_else(|| value.strip_suffix(b"+00:00"))?;
    if value.len() < 19
        || value[4] != b'-'
        || value[7] != b'-'
        || value[10] != b'T'
        || value[13] != b':'
        || value[16] != b':'
    {
        return None;
    }
    let year = digits(&value[..4])?;
    let month = digits(&value[5..7])?;
    let day = digits(&value[8..10])?;
    let hour = digits(&value[11..13])?;
    let minute = digits(&value[14..16])?;
    let second = digits(&value[17..19])?;
    let days_in_month = match month {
        4 | 6 | 9 | 11 => 30,
        2 => 28 + i64::from(year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)),
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let fraction = &value[19..];
    let millis = if fraction.is_empty() {
        0
    } else {
        let fraction = fraction.strip_prefix(b".")?;
        if fraction.is_empty() || !fraction.iter().all(u8::is_ascii_digit) {
            return None;
        }
        fraction
            .iter()
            .take(3)
            .zip([100, 10, 1])
            .map(|(digit, scale)| i64::from(digit - b'0') * scale)
            .sum()
    };
    // Days from the civil date, after Howard Hinnant's algorithm.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds * 1_000 + millis).ok()
}

fn digits(bytes: &[u8]) -> Option<i64> {
    bytes.iter().try_fold(0, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + i64::from(byte - b'0'))
    })
}

#[cfg(test)]
mod tests {
    use super::parse_utc_millis;

    #[test]
    fn utc_suffixes_and_fractions_agree_to_millisecond_precision() {
        for (fraction, millis) in [("", 0), (".1", 100), (".12", 120), (".1234567", 123)] {
            for suffix in ["Z", "+00:00"] {
                let value = format!("1970-01-01T00:00:00{fraction}{suffix}");
                assert_eq!(parse_utc_millis(value.as_bytes()), Some(millis));
            }
        }
        assert_eq!(
            parse_utc_millis(b"2000-02-29T00:00:00Z"),
            Some(951_782_400_000)
        );
        for invalid in [
            "1900-02-29T00:00:00Z",
            "1970-01-01T00:00:00.Z",
            "1969-12-31T23:59:59Z",
            "1970-01-01T00:00:00+01:00",
        ] {
            assert_eq!(parse_utc_millis(invalid.as_bytes()), None);
        }
    }
}
