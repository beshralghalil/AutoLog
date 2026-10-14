//! Finding an issue key in whatever the time tracker was told to call the task.
//!
//! People write the key in about three ways, and the tracker keeps whatever
//! they typed: `ABC-123`, `[ABC-123] Some title`, `[ABC] 123 Some title`.

use std::sync::LazyLock;

use regex::Regex;

static PATTERNS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        // [ABC-123]
        Regex::new(r"\[([A-Za-z][A-Za-z0-9]*)\s*-\s*(\d+)\]").expect("valid pattern"),
        // [ABC] 123
        Regex::new(r"\[([A-Za-z][A-Za-z0-9]*)\]\s*(\d+)").expect("valid pattern"),
        // ABC-123
        Regex::new(r"\b([A-Za-z][A-Za-z0-9]*)\s*-\s*(\d+)\b").expect("valid pattern"),
    ]
});

/// The issue key in `text`, upper-cased as trackers write it.
///
/// A key starts with a letter, which keeps a date like `2025-05` and a
/// duration like `1-30` from being read as one.
pub fn parse(text: &str) -> Option<String> {
    PATTERNS.iter().find_map(|pattern| {
        pattern
            .captures(text)
            .map(|captures| format!("{}-{}", captures[1].to_uppercase(), &captures[2]))
    })
}

/// Just the project side of the key, e.g. `ACME` out of `ACME-6055`.
pub fn parse_project(text: &str) -> Option<String> {
    parse(text).and_then(|key| key.split('-').next().map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shapes_people_actually_write() {
        assert_eq!(parse("ACME-6055"), Some("ACME-6055".to_owned()));
        assert_eq!(
            parse("[AUTOLOG-1] Series Edit Page"),
            Some("AUTOLOG-1".to_owned())
        );
        assert_eq!(parse("[ABC] 123 some title"), Some("ABC-123".to_owned()));
        assert_eq!(parse("fixing abc - 42 today"), Some("ABC-42".to_owned()));
    }

    #[test]
    fn keys_come_back_upper_cased() {
        assert_eq!(parse("autolog-7"), Some("AUTOLOG-7".to_owned()));
    }

    #[test]
    fn text_with_no_key_yields_none() {
        assert_eq!(parse("Meetings"), None);
        assert_eq!(parse("SVP3 FW Weekly"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn a_leading_letter_keeps_dates_and_clocks_out() {
        assert_eq!(parse("2025-05"), None);
        assert_eq!(parse("1-30"), None);
    }

    #[test]
    fn the_project_is_the_half_before_the_dash() {
        assert_eq!(parse_project("[ACME-6055] work"), Some("ACME".to_owned()));
        assert_eq!(parse_project("Meetings"), None);
    }
}
