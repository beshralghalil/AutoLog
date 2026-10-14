//! Providers, described as data.
//!
//! Everything that differs between a Grindstone export and an Odoo one is a
//! column name or a unit, so a provider is a value rather than a type. Adding
//! one means adding a [`ProviderSpec`] to [`PROVIDERS`]; no trait to implement
//! and nothing else in the crate to touch.

use std::sync::LazyLock;

use crate::time::DateOrder;

/// How a provider writes the length of a piece of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationFormat {
    /// A whole number of seconds, as Kimai exports.
    Seconds,
    /// A whole number of minutes.
    Minutes,
    /// `H:MM:SS`, or `D.HH:MM:SS` once the work passes a day, as Grindstone
    /// exports.
    Clock,
    /// Hours as a decimal, e.g. `1.75`, as Odoo exports.
    DecimalHours,
}

/// Everything needed to read one vendor's export.
#[derive(Debug, Clone)]
pub struct ProviderSpec {
    /// Stable key, used in the config file.
    pub id: &'static str,
    /// Name shown in the picker.
    pub label: &'static str,
    /// One line in the UI about which export this expects.
    pub about: &'static str,
    /// Columns holding the start, joined with a space when there is more than
    /// one — Kimai splits it into `Date` and `From`.
    pub started: &'static [&'static str],
    /// Clock time to assume when the start column carries only a date.
    pub default_time: Option<(u32, u32)>,
    pub duration: &'static str,
    pub duration_format: DurationFormat,
    /// Free-text comment. Optional: a blank cell is fine.
    pub description: &'static str,
    /// The provider's name for the work, shown in the table and used as the
    /// weakest hint when guessing a destination.
    pub activity: &'static str,
    /// Column the issue key is read out of; usually the same as `activity`.
    pub issue_source: &'static str,
    /// Date convention this vendor writes, where it is known.
    pub date_order: DateOrder,
}

impl ProviderSpec {
    /// Columns a file must have before it is worth reading.
    pub fn required_columns(&self) -> Vec<&'static str> {
        let mut columns = self.started.to_vec();
        columns.push(self.duration);
        columns.push(self.activity);
        columns.sort_unstable();
        columns.dedup();
        columns
    }
}

pub static PROVIDERS: LazyLock<Vec<ProviderSpec>> = LazyLock::new(|| {
    vec![
        ProviderSpec {
            id: "grindstone",
            label: "Grindstone",
            about: "CSV export with Work Item, Start, Duration and Notes columns.",
            started: &["Start"],
            default_time: None,
            duration: "Duration",
            duration_format: DurationFormat::Clock,
            description: "Notes",
            activity: "Work Item",
            issue_source: "Work Item",
            date_order: DateOrder::MonthFirst,
        },
        ProviderSpec {
            id: "kimai",
            label: "Kimai",
            about: "CSV export with Date, From, Duration, Activity and Description columns.",
            started: &["Date", "From"],
            default_time: None,
            duration: "Duration",
            duration_format: DurationFormat::Seconds,
            description: "Description",
            activity: "Activity",
            issue_source: "Activity",
            date_order: DateOrder::Auto,
        },
        ProviderSpec {
            id: "odoo",
            label: "Odoo",
            about: "Spreadsheet export with Date, Quantity (hours), Task and Description columns.",
            started: &["Date"],
            default_time: Some((8, 0)),
            duration: "Quantity",
            duration_format: DurationFormat::DecimalHours,
            description: "Description",
            activity: "Task",
            issue_source: "Task",
            date_order: DateOrder::Auto,
        },
    ]
});

pub fn provider(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS
        .iter()
        .find(|spec| spec.id.eq_ignore_ascii_case(id))
}

/// Read a duration written in `format` into seconds.
pub fn parse_duration(text: &str, format: DurationFormat) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    let seconds = match format {
        DurationFormat::Seconds => text.parse::<f64>().ok()?,
        DurationFormat::Minutes => text.parse::<f64>().ok()? * 60.0,
        DurationFormat::DecimalHours => text.parse::<f64>().ok()? * 3600.0,
        DurationFormat::Clock => parse_clock(text)?,
    };

    if seconds.is_finite() && seconds >= 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(seconds.round() as u32)
    } else {
        None
    }
}

/// `H:MM:SS`, with an optional `D.` in front once the work passes a day.
fn parse_clock(text: &str) -> Option<f64> {
    let (days, clock) = match text.rsplit_once('.') {
        Some((days, clock)) if clock.contains(':') => (days.trim().parse::<u64>().ok()?, clock),
        _ => (0, text),
    };

    let mut seconds = 0_u64;
    let mut parts = 0;
    for part in clock.split(':') {
        seconds = seconds
            .checked_mul(60)?
            .checked_add(part.trim().parse::<u64>().ok()?)?;
        parts += 1;
    }

    // `1:30` is an hour and a half; a bare number is not a clock at all.
    match parts {
        2 => seconds = seconds.checked_mul(60)?,
        3 => {}
        _ => return None,
    }

    #[allow(clippy::cast_precision_loss)]
    Some((days * 86_400 + seconds) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grindstone_clocks_read_as_seconds() {
        assert_eq!(parse_duration("1:00:04", DurationFormat::Clock), Some(3604));
        assert_eq!(parse_duration("0:41:56", DurationFormat::Clock), Some(2516));
        assert_eq!(
            parse_duration("2.03:00:00", DurationFormat::Clock),
            Some(183_600)
        );
        assert_eq!(parse_duration("1:30", DurationFormat::Clock), Some(5400));
    }

    #[test]
    fn odoo_decimal_hours_do_not_wrap_past_a_day() {
        assert_eq!(
            parse_duration("1.75", DurationFormat::DecimalHours),
            Some(6300)
        );
        assert_eq!(
            parse_duration("1", DurationFormat::DecimalHours),
            Some(3600)
        );
        // The Python tool read this as 1 hour, having taken `timedelta.seconds`.
        assert_eq!(
            parse_duration("25", DurationFormat::DecimalHours),
            Some(90_000)
        );
    }

    #[test]
    fn nonsense_durations_are_refused_rather_than_guessed() {
        assert_eq!(parse_duration("", DurationFormat::Seconds), None);
        assert_eq!(parse_duration("a while", DurationFormat::Clock), None);
        assert_eq!(parse_duration("90", DurationFormat::Clock), None);
        assert_eq!(parse_duration("-5", DurationFormat::Seconds), None);
    }

    #[test]
    fn every_provider_is_addressable_by_its_id() {
        for spec in PROVIDERS.iter() {
            assert_eq!(provider(spec.id).map(|found| found.id), Some(spec.id));
        }
        assert!(provider("nope").is_none());
    }
}
