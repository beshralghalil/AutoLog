//! The one row of work that travels from a file to a tracker.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Timelike as _, Utc};
use chrono_tz::Tz;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::UserError;
use crate::time::{self, ParsedDateTime};

/// Position of an entry in the loaded file; stable for the life of the load,
/// so the table and the submission pipeline can refer to the same row.
pub type EntryId = usize;

/// One record exactly as the file wrote it, column name to cell text, in the
/// file's own column order.
pub type Row = IndexMap<String, String>;

/// Where a row stands. Anything other than [`EntryStatus::Success`] may be
/// submitted again, which is what makes retrying a fixed row work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryStatus {
    /// Loaded and not yet sent.
    #[default]
    Pending,
    /// Accepted for this run, waiting its turn.
    Queued,
    /// In flight.
    Submitting,
    /// The tracker took it.
    Success,
    /// The tracker already had it.
    Skipped,
    /// The tracker refused it, or the network did.
    Failed,
    /// A dry run found it incomplete; nothing was sent.
    Invalid,
}

impl EntryStatus {
    /// Whether a run should pick this row up.
    pub fn is_submittable(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Failed | Self::Skipped | Self::Invalid
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Queued => "Queued",
            Self::Submitting => "Sending",
            Self::Success => "Posted",
            Self::Skipped => "Duplicate",
            Self::Failed => "Failed",
            Self::Invalid => "Incomplete",
        }
    }
}

/// A single unit of work read from a provider export.
#[derive(Debug, Clone)]
pub struct WorklogEntry {
    pub id: EntryId,
    /// The start as the file wrote it, kept so that changing the time zone
    /// re-reads the loaded rows instead of re-reading the file.
    pub source_start: ParsedDateTime,
    pub started: DateTime<Tz>,
    /// Length of the work, in seconds.
    pub duration: u32,
    pub description: String,
    /// The provider's free text for the work: a Grindstone work item, a Kimai
    /// activity. Both the issue key and the target guesses come from it.
    pub activity: String,
    /// The untouched text the issue key was looked for in.
    pub raw_issue_key: String,
    pub issue_key: Option<String>,
    pub project_key: Option<String>,
    /// Consumer destinations by field key, e.g. `project` and `activity` for
    /// Kimai. Kept generic so a new consumer needs no change here.
    pub targets: BTreeMap<String, String>,
    pub status: EntryStatus,
    /// Why the row is in its current state, when that needs saying.
    pub outcome: Option<UserError>,
    pub raw: Row,
}

impl WorklogEntry {
    /// The start converted to UTC, which is the only footing on which two
    /// entries from different sources can be compared.
    pub fn start_utc(&self) -> DateTime<Utc> {
        self.started.with_timezone(&Utc)
    }

    pub fn end(&self) -> DateTime<Tz> {
        self.started + Duration::seconds(i64::from(self.duration))
    }

    /// Re-read the stored start in a different zone.
    pub fn relocalize(&mut self, tz: Tz) -> Result<(), time::TimeError> {
        self.started = time::localize(self.source_start, tz)?;
        Ok(())
    }

    pub fn target(&self, field: &str) -> Option<&str> {
        self.targets
            .get(field)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn set_target(&mut self, field: &str, value: impl Into<String>) {
        let value = value.into();
        if value.is_empty() {
            self.targets.remove(field);
        } else {
            self.targets.insert(field.to_owned(), value);
        }
    }

    /// Put the row back in line to be sent, after the user has changed it.
    pub fn reset(&mut self) {
        if self.status != EntryStatus::Success {
            self.status = EntryStatus::Pending;
            self.outcome = None;
        }
    }

    pub fn duration_label(&self) -> String {
        format_duration(self.duration)
    }
}

/// An entry already sitting in the tracker, reduced to the fields that decide
/// whether sending ours again would duplicate it.
#[derive(Debug, Clone)]
pub struct LoggedEntry {
    pub started: DateTime<Utc>,
    pub duration: u32,
    pub description: String,
}

impl LoggedEntry {
    /// Same minute, same length, same words: the tracker already has this.
    ///
    /// Seconds are dropped deliberately. A tracker rounds or restates the
    /// start it was given, and nobody logs two entries of equal length and
    /// equal text within one minute by accident.
    pub fn duplicates(&self, entry: &WorklogEntry) -> bool {
        to_the_minute(self.started) == to_the_minute(entry.start_utc())
            && self.duration == entry.duration
            && collapse_whitespace(&self.description) == collapse_whitespace(&entry.description)
    }
}

fn to_the_minute(value: DateTime<Utc>) -> DateTime<Utc> {
    value
        .with_second(0)
        .unwrap_or(value)
        .with_nanosecond(0)
        .unwrap_or(value)
}

/// Lower-case with runs of whitespace flattened, so that a comment reformatted
/// by the tracker still reads as the same comment.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `1h 05m`, or `45m` below the hour.
pub fn format_duration(seconds: u32) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours == 0 {
        format!("{minutes}m")
    } else {
        format!("{hours}h {minutes:02}m")
    }
}

/// `7:45` for a total across many entries.
pub fn format_total(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 3600, (seconds % 3600) / 60)
}

/// Entries built for tests in this crate, so each module's tests do not each
/// hand-roll one.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::time::{DateOrder, parse_datetime};

    /// A one-hour entry whose work item is `work_item`.
    pub fn entry(work_item: &str) -> WorklogEntry {
        at(work_item, "2025-05-04 08:00", 3600)
    }

    pub fn at(work_item: &str, start: &str, duration: u32) -> WorklogEntry {
        let source_start = parse_datetime(start, DateOrder::Auto).expect("valid fixture start");
        WorklogEntry {
            id: 0,
            source_start,
            started: time::localize(source_start, Tz::UTC).expect("UTC is never ambiguous"),
            duration,
            description: String::new(),
            activity: work_item.to_owned(),
            raw_issue_key: work_item.to_owned(),
            issue_key: crate::source::issue_key::parse(work_item),
            project_key: crate::source::issue_key::parse_project(work_item),
            targets: BTreeMap::new(),
            status: EntryStatus::Pending,
            outcome: None,
            raw: Row::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{DateOrder, parse_datetime};

    fn entry(start: &str, duration: u32, description: &str) -> WorklogEntry {
        let source_start = parse_datetime(start, DateOrder::Auto).unwrap();
        WorklogEntry {
            id: 0,
            source_start,
            started: time::localize(source_start, Tz::UTC).unwrap(),
            duration,
            description: description.to_owned(),
            activity: String::new(),
            raw_issue_key: String::new(),
            issue_key: None,
            project_key: None,
            targets: BTreeMap::new(),
            status: EntryStatus::Pending,
            outcome: None,
            raw: Row::new(),
        }
    }

    fn logged(start: &str, duration: u32, description: &str) -> LoggedEntry {
        LoggedEntry {
            started: entry(start, duration, description).start_utc(),
            duration,
            description: description.to_owned(),
        }
    }

    #[test]
    fn a_duplicate_survives_restated_seconds_and_reflowed_text() {
        let ours = entry("2025-05-04 08:00:41", 3600, "  Fixed   the\nbug ");
        assert!(logged("2025-05-04 08:00:12", 3600, "Fixed the bug").duplicates(&ours));
    }

    #[test]
    fn a_different_length_or_minute_is_not_a_duplicate() {
        let ours = entry("2025-05-04 08:00", 3600, "Fixed the bug");
        assert!(!logged("2025-05-04 08:00", 1800, "Fixed the bug").duplicates(&ours));
        assert!(!logged("2025-05-04 08:01", 3600, "Fixed the bug").duplicates(&ours));
        assert!(!logged("2025-05-04 08:00", 3600, "Fixed the typo").duplicates(&ours));
    }

    #[test]
    fn durations_read_as_hours_and_minutes() {
        assert_eq!(format_duration(3604), "1h 00m");
        assert_eq!(format_duration(2516), "41m");
        assert_eq!(format_total(27_000), "7:30");
    }

    #[test]
    fn an_emptied_target_is_dropped_rather_than_stored_blank() {
        let mut entry = entry("2025-05-04 08:00", 60, "");
        entry.set_target("project", "ACME");
        assert_eq!(entry.target("project"), Some("ACME"));
        entry.set_target("project", "");
        assert_eq!(entry.target("project"), None);
    }
}
