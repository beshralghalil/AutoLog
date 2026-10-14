//! Reading the many ways an export writes a date, and pinning it to a zone.
//!
//! Exports are written by whatever tool the user happens to run, so the day
//! can arrive as `2025-05-04`, `10/12/2026 11:00:29 AM` or `5-2-2024 11:00`.
//! Rather than guess with one format, [`parse_datetime`] splits the text into
//! a date and a time and tries the shapes each half is known to take.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Which number comes first when a date is written with separators, as in
/// `5-2-2024`. Only ever consulted when both halves could be a month.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DateOrder {
    /// Decide per value: a leading number above 12 can only be a day.
    /// Falls back to month-first, the convention the old Python tool used.
    #[default]
    Auto,
    /// Always read `5-2-2024` as the 2nd of May.
    MonthFirst,
    /// Always read `5-2-2024` as the 5th of February.
    DayFirst,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum TimeError {
    #[error("the date is empty")]
    Empty,
    #[error("could not read `{0}` as a date and time")]
    Unrecognized(String),
    #[error("`{0}` does not exist in the {1} time zone (clocks moved forward)")]
    Skipped(String, Tz),
}

/// A date and time as the file wrote it: either bare, or already carrying an
/// offset. Kept unresolved so that changing the zone in settings re-reads the
/// loaded entries instead of forcing the file to be parsed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedDateTime {
    Naive(NaiveDateTime),
    Fixed(DateTime<FixedOffset>),
}

/// Date layouts that carry no ambiguity about which half is the month.
const UNAMBIGUOUS_DATES: &[&str] = &[
    "%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d", "%d %b %Y", "%d %B %Y", "%b %d %Y", "%B %d %Y", "%d-%b-%Y",
    "%d-%B-%Y",
];

const MONTH_FIRST_DATES: &[&str] = &["%m/%d/%Y", "%m-%d-%Y", "%m.%d.%Y", "%m/%d/%y", "%m-%d-%y"];
const DAY_FIRST_DATES: &[&str] = &["%d/%m/%Y", "%d-%m-%Y", "%d.%m.%Y", "%d/%m/%y", "%d-%m-%y"];

const TIMES: &[&str] = &[
    "%H:%M:%S%.f",
    "%H:%M:%S",
    "%H:%M",
    "%I:%M:%S %p",
    "%I:%M %p",
    "%I:%M:%S%p",
    "%I:%M%p",
    "%I %p",
];

/// Read a date, and a time when the text carries one.
///
/// A value with an explicit offset keeps it; everything else stays naive until
/// [`localize`] attaches the zone the user selected.
pub fn parse_datetime(input: &str, order: DateOrder) -> Result<ParsedDateTime, TimeError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(TimeError::Empty);
    }

    if let Some(fixed) = parse_offset_aware(text) {
        return Ok(ParsedDateTime::Fixed(fixed));
    }

    split_candidates(text)
        .into_iter()
        .find_map(|(date_part, time_part)| {
            let date = parse_date(date_part, order)?;
            let time = match time_part {
                Some(part) => parse_time(part)?,
                None => NaiveTime::MIN,
            };
            Some(ParsedDateTime::Naive(date.and_time(time)))
        })
        .ok_or_else(|| TimeError::Unrecognized(text.to_owned()))
}

/// Attach `tz` to a parsed value, converting one that already has an offset.
///
/// Where the clock moved back, the earlier of the two readings wins; where it
/// moved forward the time does not exist at all, which is worth telling the
/// user about rather than silently nudging.
pub fn localize(parsed: ParsedDateTime, tz: Tz) -> Result<DateTime<Tz>, TimeError> {
    match parsed {
        ParsedDateTime::Fixed(dt) => Ok(dt.with_timezone(&tz)),
        ParsedDateTime::Naive(naive) => match tz.from_local_datetime(&naive) {
            chrono::offset::LocalResult::Single(dt) => Ok(dt),
            chrono::offset::LocalResult::Ambiguous(earlier, _) => Ok(earlier),
            chrono::offset::LocalResult::None => Err(TimeError::Skipped(naive.to_string(), tz)),
        },
    }
}

/// Read a timestamp an API wrote and pin it to UTC.
///
/// Trackers answer in their own offset — Jira as `+0000`, Kimai as `+0200` —
/// and the only thing worth comparing across them is the instant.
pub fn parse_utc(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let parsed = parse_datetime(text, DateOrder::Auto).ok()?;
    localize(parsed, Tz::UTC)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

fn parse_offset_aware(text: &str) -> Option<DateTime<FixedOffset>> {
    const OFFSET_FORMATS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f%#z",
        "%Y-%m-%d %H:%M:%S%.f%#z",
        "%Y-%m-%dT%H:%M%#z",
        "%Y-%m-%d %H:%M%#z",
    ];

    DateTime::parse_from_rfc3339(text)
        .ok()
        .or_else(|| DateTime::parse_from_rfc2822(text).ok())
        .or_else(|| {
            OFFSET_FORMATS
                .iter()
                .find_map(|format| DateTime::parse_from_str(text, format).ok())
        })
}

/// Every way the text could divide into a date and a time.
///
/// Where the halves part is not obvious — `12 Mar 2024 09:30` has three
/// spaces in the date alone — the only honest answer is to try each boundary
/// and keep the split where both halves read. Splits come first, so a value
/// that is plainly a date and a time is not mistaken for a long date.
fn split_candidates(text: &str) -> Vec<(&str, Option<&str>)> {
    let mut candidates: Vec<(&str, Option<&str>)> = text
        .char_indices()
        .filter(|&(index, ch)| {
            ch.is_whitespace() || (matches!(ch, 'T' | 't') && is_iso_boundary(text, index))
        })
        .filter_map(|(index, ch)| {
            let date = text[..index].trim();
            let time = text[index + ch.len_utf8()..].trim();
            (!date.is_empty() && !time.is_empty()).then_some((date, Some(time)))
        })
        .collect();

    candidates.push((text, None));
    candidates
}

/// `T` only separates the halves of an ISO value; in `12 Mar 2024` it is part
/// of a month name.
fn is_iso_boundary(text: &str, index: usize) -> bool {
    let before = text[..index].chars().next_back();
    let after = text[index + 1..].chars().next();
    matches!((before, after), (Some(b), Some(a)) if b.is_ascii_digit() && a.is_ascii_digit())
}

fn parse_date(text: &str, order: DateOrder) -> Option<NaiveDate> {
    for format in UNAMBIGUOUS_DATES {
        if let Ok(date) = NaiveDate::parse_from_str(text, format) {
            return Some(date);
        }
    }

    let (first, second) = match order {
        DateOrder::DayFirst => (DAY_FIRST_DATES, MONTH_FIRST_DATES),
        DateOrder::MonthFirst => (MONTH_FIRST_DATES, DAY_FIRST_DATES),
        DateOrder::Auto => {
            if leads_with_a_day(text) {
                (DAY_FIRST_DATES, MONTH_FIRST_DATES)
            } else {
                (MONTH_FIRST_DATES, DAY_FIRST_DATES)
            }
        }
    };

    first
        .iter()
        .chain(second)
        .find_map(|format| NaiveDate::parse_from_str(text, format).ok())
}

/// A leading number above 12 cannot be a month, whatever the convention.
fn leads_with_a_day(text: &str) -> bool {
    text.split(['/', '-', '.'])
        .next()
        .and_then(|head| head.parse::<u32>().ok())
        .is_some_and(|value| value > 12)
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    let normalized = text.to_uppercase();
    TIMES
        .iter()
        .find_map(|format| NaiveTime::parse_from_str(&normalized, format).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(text: &str, order: DateOrder) -> NaiveDateTime {
        match parse_datetime(text, order).expect("should parse") {
            ParsedDateTime::Naive(dt) => dt,
            ParsedDateTime::Fixed(dt) => dt.naive_local(),
        }
    }

    #[test]
    fn reads_grindstone_twelve_hour_clock() {
        assert_eq!(
            naive("10/12/2026 11:00:29 AM", DateOrder::Auto).to_string(),
            "2026-10-12 11:00:29"
        );
        assert_eq!(
            naive("10/12/2026 1:21:05 PM", DateOrder::Auto).to_string(),
            "2026-10-12 13:21:05"
        );
    }

    #[test]
    fn reads_iso_dates_with_and_without_a_time() {
        assert_eq!(
            naive("2025-05-04 08:00", DateOrder::Auto).to_string(),
            "2025-05-04 08:00:00"
        );
        assert_eq!(
            naive("2025-05-04", DateOrder::Auto).to_string(),
            "2025-05-04 00:00:00"
        );
    }

    #[test]
    fn auto_order_defaults_to_month_first_like_the_python_tool() {
        assert_eq!(
            naive("5-2-2024 11:00", DateOrder::Auto).to_string(),
            "2024-05-02 11:00:00"
        );
    }

    #[test]
    fn auto_order_yields_when_the_lead_cannot_be_a_month() {
        assert_eq!(
            naive("25/12/2024", DateOrder::Auto).to_string(),
            "2024-12-25 00:00:00"
        );
    }

    #[test]
    fn explicit_order_overrides_the_guess() {
        assert_eq!(
            naive("5-2-2024", DateOrder::DayFirst).to_string(),
            "2024-02-05 00:00:00"
        );
    }

    #[test]
    fn keeps_an_explicit_offset() {
        let parsed = parse_datetime("2025-05-04T08:00:00+02:00", DateOrder::Auto).unwrap();
        assert!(matches!(parsed, ParsedDateTime::Fixed(_)));
        let utc = localize(parsed, Tz::UTC).unwrap();
        assert_eq!(utc.to_string(), "2025-05-04 06:00:00 UTC");
    }

    #[test]
    fn month_names_are_not_mistaken_for_an_iso_separator() {
        assert_eq!(
            naive("12 Mar 2024 09:30", DateOrder::Auto).to_string(),
            "2024-03-12 09:30:00"
        );
    }

    #[test]
    fn api_timestamps_land_on_the_same_instant_whatever_offset_they_carry() {
        assert_eq!(
            parse_utc("2025-05-04T08:00:00.000+0000")
                .unwrap()
                .to_string(),
            "2025-05-04 08:00:00 UTC"
        );
        assert_eq!(
            parse_utc("2025-05-04T08:00:00+0200").unwrap().to_string(),
            "2025-05-04 06:00:00 UTC"
        );
        assert!(parse_utc("nonsense").is_none());
    }

    #[test]
    fn rejects_text_that_is_not_a_date() {
        assert!(matches!(
            parse_datetime("not a date", DateOrder::Auto),
            Err(TimeError::Unrecognized(_))
        ));
        assert!(matches!(
            parse_datetime("   ", DateOrder::Auto),
            Err(TimeError::Empty)
        ));
    }

    #[test]
    fn localizing_picks_the_earlier_side_of_a_repeated_hour() {
        // Central European Summer Time ends at 03:00 on 2024-10-27, so 02:30
        // happens twice.
        let parsed = parse_datetime("2024-10-27 02:30", DateOrder::Auto).unwrap();
        let dt = localize(parsed, Tz::Europe__Berlin).unwrap();
        // Summer time, the earlier of the two readings, is 00:30 UTC.
        assert_eq!(
            dt.with_timezone(&chrono::Utc).to_string(),
            "2024-10-27 00:30:00 UTC"
        );
    }
}
