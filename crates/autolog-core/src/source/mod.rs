//! Reading a provider export into entries.

pub mod issue_key;
pub mod reader;
pub mod spec;

use std::collections::HashMap;
use std::path::Path;

use chrono_tz::Tz;

use crate::error::{UserError, UserFacing};
use crate::model::{EntryStatus, Row, WorklogEntry};
use crate::time::{self, DateOrder};

pub use reader::{SUPPORTED_EXTENSIONS, Sheet};
pub use spec::{DurationFormat, PROVIDERS, ProviderSpec, provider};

#[derive(Debug, Clone, thiserror::Error)]
pub enum SourceError {
    #[error("unsupported file type: {extension}")]
    UnsupportedFile { extension: String },
    #[error("could not open {path}: {reason}")]
    Unreadable { path: String, reason: String },
    #[error("could not read {path}: {reason}")]
    Malformed { path: String, reason: String },
    #[error("{path} has no rows")]
    Empty { path: String },
    #[error("{provider} needs the column(s) {}", missing.join(", "))]
    MissingColumns {
        provider: String,
        missing: Vec<String>,
        found: Vec<String>,
    },
}

impl UserFacing for SourceError {
    fn to_user_error(&self) -> UserError {
        match self {
            Self::UnsupportedFile { extension } => UserError::new(
                "Unsupported file",
                format!("AutoLog cannot read {extension} files."),
            )
            .with_hint(format!(
                "Export your worklog as one of: {}.",
                SUPPORTED_EXTENSIONS.join(", ")
            )),
            Self::Unreadable { path, reason } => UserError::new(
                "Could not open the file",
                format!("{path} could not be opened."),
            )
            .with_hint("Check that the file still exists and is not open in another program.")
            .with_detail(reason.clone()),
            Self::Malformed { path, reason } => UserError::new(
                "Could not read the file",
                format!("{path} is not laid out as expected."),
            )
            .with_hint("Re-export it from your time tracker and try again.")
            .with_detail(reason.clone()),
            Self::Empty { path } => UserError::new(
                "Nothing to load",
                format!("{path} has a header but no rows."),
            ),
            Self::MissingColumns {
                provider,
                missing,
                found,
            } => UserError::new(
                "Wrong provider for this file",
                format!(
                    "This file has no {} column{}, which {provider} exports need.",
                    missing.join(" or "),
                    if missing.len() == 1 { "" } else { "s" }
                ),
            )
            .with_hint("Pick the provider that matches the file, or re-export it.")
            .with_detail(format!("Columns in the file: {}", found.join(", "))),
        }
    }
}

/// A row the file had but no entry could be made from.
#[derive(Debug, Clone)]
pub struct SkippedRow {
    /// 1-based position among the data rows, as a spreadsheet counts them.
    pub number: usize,
    pub reason: String,
}

/// Everything one file turned into.
#[derive(Debug, Clone, Default)]
pub struct LoadedFile {
    pub entries: Vec<WorklogEntry>,
    /// Rows that could not be read, kept so the user is told rather than left
    /// wondering why the count is short.
    pub skipped: Vec<SkippedRow>,
    /// Every column the file has, in its own order.
    pub columns: Vec<String>,
    pub total_seconds: u64,
}

/// Read `path` as a `spec` export, with starts pinned to `tz`.
///
/// `order` overrides the provider's own date convention; pass `None` to use it.
pub fn load(
    path: &Path,
    spec: &ProviderSpec,
    tz: Tz,
    order: Option<DateOrder>,
) -> Result<LoadedFile, SourceError> {
    let sheet = reader::read_rows(path)?;
    if sheet.columns.is_empty() {
        return Err(SourceError::Empty {
            path: path.display().to_string(),
        });
    }

    let columns = ColumnIndex::new(&sheet.columns);
    let missing: Vec<String> = spec
        .required_columns()
        .into_iter()
        .filter(|name| columns.resolve(name).is_none())
        .map(ToOwned::to_owned)
        .collect();

    if !missing.is_empty() {
        return Err(SourceError::MissingColumns {
            provider: spec.label.to_owned(),
            missing,
            found: sheet.columns.clone(),
        });
    }

    let order = order.unwrap_or(spec.date_order);
    let mut entries = Vec::with_capacity(sheet.rows.len());
    let mut skipped = Vec::new();
    let mut total_seconds = 0_u64;

    for (index, row) in sheet.rows.into_iter().enumerate() {
        match build_entry(entries.len(), &row, spec, &columns, tz, order) {
            Ok(entry) => {
                total_seconds += u64::from(entry.duration);
                entries.push(entry);
            }
            Err(reason) => skipped.push(SkippedRow {
                number: index + 1,
                reason,
            }),
        }
    }

    Ok(LoadedFile {
        entries,
        skipped,
        columns: sheet.columns,
        total_seconds,
    })
}

fn build_entry(
    id: usize,
    row: &Row,
    spec: &ProviderSpec,
    columns: &ColumnIndex,
    tz: Tz,
    order: DateOrder,
) -> Result<WorklogEntry, String> {
    let started_text = started_text(row, spec, columns);
    if started_text.is_empty() {
        return Err("no start time".to_owned());
    }

    let source_start =
        time::parse_datetime(&started_text, order).map_err(|error| error.to_string())?;
    let started = time::localize(source_start, tz).map_err(|error| error.to_string())?;

    let duration_text = columns.value(row, spec.duration);
    let duration = spec::parse_duration(duration_text, spec.duration_format)
        .ok_or_else(|| format!("could not read `{duration_text}` as a duration"))?;

    let activity = columns.value(row, spec.activity).to_owned();
    let raw_issue_key = columns.value(row, spec.issue_source).to_owned();

    Ok(WorklogEntry {
        id,
        source_start,
        started,
        duration,
        description: columns.value(row, spec.description).to_owned(),
        issue_key: issue_key::parse(&raw_issue_key),
        project_key: issue_key::parse_project(&raw_issue_key),
        activity,
        raw_issue_key,
        targets: std::collections::BTreeMap::new(),
        status: EntryStatus::Pending,
        outcome: None,
        raw: row.clone(),
    })
}

/// Join the columns holding the start, adding the provider's assumed clock
/// time when the file carries only a date.
fn started_text(row: &Row, spec: &ProviderSpec, columns: &ColumnIndex) -> String {
    let mut text = spec
        .started
        .iter()
        .map(|name| columns.value(row, name))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    if let Some((hour, minute)) = spec.default_time
        && !text.is_empty()
        && !text.contains(':')
    {
        text = format!("{text} {hour}:{minute:02}");
    }

    text
}

/// Column names, matched without minding case or surrounding space, because
/// an export's capitalisation drifts between versions.
struct ColumnIndex {
    by_lowercase: HashMap<String, String>,
}

impl ColumnIndex {
    fn new(columns: &[String]) -> Self {
        Self {
            by_lowercase: columns
                .iter()
                .map(|name| (name.to_lowercase(), name.clone()))
                .collect(),
        }
    }

    fn resolve(&self, name: &str) -> Option<&str> {
        self.by_lowercase
            .get(&name.to_lowercase())
            .map(String::as_str)
    }

    fn value<'a>(&self, row: &'a Row, name: &str) -> &'a str {
        self.resolve(name)
            .and_then(|actual| row.get(actual))
            .map_or("", |value| value.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write(name: &str, contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
        (dir, path)
    }

    #[test]
    fn loads_a_grindstone_export() {
        let (_dir, path) = write(
            "grindstone.csv",
            "\u{feff}Work Item,End,Start,Duration,Notes\n\
             ACME-6055,10/12/2026 11:00:29 AM,10/12/2026 10:00:25 AM,1:00:04,rename upload-queue\n\
             Meetings,10/12/2026 11:41:58 AM,10/12/2026 11:00:02 AM,0:41:56,\n",
        );

        let loaded = load(&path, provider("grindstone").unwrap(), Tz::UTC, None).unwrap();

        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.skipped.is_empty());
        assert_eq!(loaded.total_seconds, 3604 + 2516);

        let first = &loaded.entries[0];
        assert_eq!(first.started.to_string(), "2026-10-12 10:00:25 UTC");
        assert_eq!(first.duration, 3604);
        assert_eq!(first.issue_key.as_deref(), Some("ACME-6055"));
        assert_eq!(first.project_key.as_deref(), Some("ACME"));
        assert_eq!(first.description, "rename upload-queue");

        // A row with no issue key still loads; the user fills it in.
        assert_eq!(loaded.entries[1].issue_key, None);
        assert_eq!(loaded.entries[1].activity, "Meetings");
    }

    /// The window reads an entry by indexing on its id rather than searching
    /// for it, which only holds while an id is a position.
    #[test]
    fn an_entrys_id_is_its_position() {
        let (_dir, path) = write(
            "grindstone.csv",
            "Work Item,Start,Duration,Notes\n\
             ABC-1,2025-05-04 08:00,1:00:00,fine\n\
             ABC-2,whenever,1:00:00,this row is skipped\n\
             ABC-3,2025-05-04 09:00,1:00:00,fine\n",
        );

        let loaded = load(&path, provider("grindstone").unwrap(), Tz::UTC, None).unwrap();
        assert_eq!(loaded.skipped.len(), 1);
        for (index, entry) in loaded.entries.iter().enumerate() {
            assert_eq!(
                entry.id, index,
                "a skipped row must not leave a gap in the ids"
            );
        }
    }

    #[test]
    fn keeps_every_source_column_on_the_entry() {
        let (_dir, path) = write(
            "grindstone.csv",
            "Work Item,End,Start,Duration,Notes,Billable\n\
             ABC-1,x,2025-05-04 08:00,1:00:00,note,Yes\n",
        );

        let loaded = load(&path, provider("grindstone").unwrap(), Tz::UTC, None).unwrap();
        assert_eq!(loaded.columns.len(), 6);
        assert_eq!(
            loaded.entries[0].raw.get("Billable").map(String::as_str),
            Some("Yes")
        );
    }

    #[test]
    fn odoo_dates_get_the_assumed_start_of_day() {
        let (_dir, path) = write(
            "odoo.csv",
            "Date,Description,Quantity,Task\n2025-05-04,work,1.5,[AUTOLOG-1] Series Edit Page\n",
        );

        let loaded = load(&path, provider("odoo").unwrap(), Tz::UTC, None).unwrap();
        assert_eq!(
            loaded.entries[0].started.to_string(),
            "2025-05-04 08:00:00 UTC"
        );
        assert_eq!(loaded.entries[0].duration, 5400);
    }

    #[test]
    fn kimai_joins_its_split_date_and_time() {
        let (_dir, path) = write(
            "kimai.csv",
            "\"Date\",\"From\",\"Duration\",\"Activity\",\"Description\"\n\
             \"2025-05-04\",\"08:00\",\"29800\",\"[AUTOLOG-1] Edit Page\",\"did a thing\"\n",
        );

        let loaded = load(&path, provider("kimai").unwrap(), Tz::UTC, None).unwrap();
        assert_eq!(
            loaded.entries[0].started.to_string(),
            "2025-05-04 08:00:00 UTC"
        );
        assert_eq!(loaded.entries[0].duration, 29_800);
    }

    #[test]
    fn the_wrong_provider_names_the_columns_it_wanted() {
        let (_dir, path) = write(
            "kimai.csv",
            "Date,From,Duration,Activity\n2025-05-04,08:00,60,x\n",
        );
        let error = load(&path, provider("grindstone").unwrap(), Tz::UTC, None).unwrap_err();

        let SourceError::MissingColumns { missing, .. } = &error else {
            panic!("expected missing columns, got {error:?}");
        };
        assert!(missing.contains(&"Work Item".to_owned()));
        assert!(missing.contains(&"Start".to_owned()));
        assert!(error.to_user_error().message.contains("Work Item"));
    }

    #[test]
    fn unreadable_rows_are_reported_rather_than_dropped_in_silence() {
        let (_dir, path) = write(
            "grindstone.csv",
            "Work Item,Start,Duration,Notes\n\
             ABC-1,2025-05-04 08:00,1:00:00,fine\n\
             ABC-2,whenever,1:00:00,bad date\n\
             ABC-3,2025-05-04 09:00,a while,bad duration\n",
        );

        let loaded = load(&path, provider("grindstone").unwrap(), Tz::UTC, None).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.skipped.len(), 2);
        assert_eq!(loaded.skipped[0].number, 2);
        assert!(loaded.skipped[1].reason.contains("duration"));
    }

    #[test]
    fn column_names_are_matched_without_minding_case() {
        let (_dir, path) = write(
            "grindstone.csv",
            "work item,start,DURATION,notes\nABC-1,2025-05-04 08:00,1:00:00,fine\n",
        );
        assert_eq!(
            load(&path, provider("grindstone").unwrap(), Tz::UTC, None)
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[test]
    fn entries_are_pinned_to_the_chosen_zone() {
        let (_dir, path) = write(
            "grindstone.csv",
            "Work Item,Start,Duration,Notes\nABC-1,2025-05-04 08:00,1:00:00,fine\n",
        );
        let loaded = load(
            &path,
            provider("grindstone").unwrap(),
            Tz::Asia__Damascus,
            None,
        )
        .unwrap();
        assert_eq!(
            loaded.entries[0].start_utc().to_string(),
            "2025-05-04 05:00:00 UTC"
        );
    }
}
