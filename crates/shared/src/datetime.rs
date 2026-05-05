//! Minimal ISO 8601 (`YYYY-MM-DDTHH:MM:SSZ`) parsing and date arithmetic for
//! the 14-dimension vectorization formulas.
//!
//! The Rinha 2026 payload contract pins the timestamp shape to a 20-byte
//! UTC representation, so this module avoids dragging in `chrono`/`time`.

use core::fmt;

#[derive(Debug, Clone, Copy)]
pub struct Utc {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

#[derive(Debug, Clone)]
pub enum ParseError {
    Shape,
    Numeric(&'static str),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shape => f.write_str("expected 20-byte ISO 8601 timestamp YYYY-MM-DDTHH:MM:SSZ"),
            Self::Numeric(c) => write!(f, "non-numeric value in {c}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl Utc {
    /// Parse `YYYY-MM-DDTHH:MM:SSZ`. Fractional seconds and offsets other than
    /// `Z` are rejected — the Rinha 2026 contract pins this exact shape.
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        let bytes = s.as_bytes();
        if bytes.len() != 20
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || bytes[10] != b'T'
            || bytes[13] != b':'
            || bytes[16] != b':'
            || bytes[19] != b'Z'
        {
            return Err(ParseError::Shape);
        }
        Ok(Self {
            year: parse_uint(&bytes[0..4], "year")?,
            month: parse_uint(&bytes[5..7], "month")?,
            day: parse_uint(&bytes[8..10], "day")?,
            hour: parse_uint(&bytes[11..13], "hour")?,
            minute: parse_uint(&bytes[14..16], "minute")?,
            second: parse_uint(&bytes[17..19], "second")?,
        })
    }

    /// Seconds since `1970-01-01T00:00:00Z`, derived from the Julian Day Number
    /// so the difference between two timestamps is exact across month/year
    /// boundaries.
    #[must_use]
    pub fn unix_seconds(&self) -> i64 {
        const UNIX_EPOCH_JDN: i64 = 2_440_588;
        let jdn = julian_day_number(self.year as i32, self.month, self.day);
        (jdn - UNIX_EPOCH_JDN) * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }

    /// Day of week with Monday=0..Sunday=6, matching the spec.
    #[must_use]
    pub fn weekday_monday0(&self) -> u32 {
        // Zeller's congruence (Gregorian) returns 0=Saturday..6=Friday.
        let (y, m) = if self.month < 3 {
            (self.year - 1, self.month + 12)
        } else {
            (self.year, self.month)
        };
        let k = y % 100;
        let j = y / 100;
        let h = (self.day + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
        // Map Zeller's 0=Sat..6=Fri to 0=Mon..6=Sun.
        (h + 5) % 7
    }
}

fn parse_uint(bytes: &[u8], component: &'static str) -> Result<u32, ParseError> {
    let mut acc: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return Err(ParseError::Numeric(component));
        }
        acc = acc * 10 + u32::from(b - b'0');
    }
    Ok(acc)
}

fn julian_day_number(y: i32, m: u32, d: u32) -> i64 {
    // Fliegel & Van Flandern Gregorian-to-JDN.
    let m = m as i32;
    let d = d as i32;
    let a = (14 - m) / 12;
    let y = y + 4800 - a;
    let m = m + 12 * a - 3;
    i64::from(d + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32_045)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_timestamp() {
        let t = Utc::parse("2026-03-11T18:45:53Z").unwrap();
        assert_eq!(t.year, 2026);
        assert_eq!(t.month, 3);
        assert_eq!(t.day, 11);
        assert_eq!(t.hour, 18);
        assert_eq!(t.minute, 45);
        assert_eq!(t.second, 53);
    }

    #[test]
    fn rejects_bad_shape() {
        assert!(matches!(Utc::parse("2026-03-11"), Err(ParseError::Shape)));
        assert!(matches!(
            Utc::parse("2026-03-11T18:45:53+00:00"),
            Err(ParseError::Shape)
        ));
    }

    #[test]
    fn weekday_known_dates() {
        // 2026-01-01 is a Thursday → Monday-zero index 3.
        assert_eq!(Utc::parse("2026-01-01T00:00:00Z").unwrap().weekday_monday0(), 3);
        // 2026-03-11 is a Wednesday → 2.
        assert_eq!(Utc::parse("2026-03-11T18:45:53Z").unwrap().weekday_monday0(), 2);
        // 2026-03-15 is a Sunday → 6.
        assert_eq!(Utc::parse("2026-03-15T00:00:00Z").unwrap().weekday_monday0(), 6);
    }

    #[test]
    fn unix_seconds_matches_known_anchor() {
        // Unix epoch.
        assert_eq!(Utc::parse("1970-01-01T00:00:00Z").unwrap().unix_seconds(), 0);
        // 2000-01-01T00:00:00Z = 946684800.
        assert_eq!(
            Utc::parse("2000-01-01T00:00:00Z").unwrap().unix_seconds(),
            946_684_800
        );
    }

    #[test]
    fn unix_seconds_difference_is_minutes() {
        let a = Utc::parse("2026-03-11T18:45:53Z").unwrap();
        let b = Utc::parse("2026-03-11T14:58:35Z").unwrap();
        let delta = (a.unix_seconds() - b.unix_seconds()) / 60;
        // 18:45:53 - 14:58:35 = 3h47m18s = 227.3 min → 227 floor.
        assert_eq!(delta, 227);
    }
}
