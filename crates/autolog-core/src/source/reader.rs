//! Getting rows out of a file, whatever the file happens to be.
//!
//! Both readers hand back the same thing: every column the file has, under the
//! header the file wrote, as text. Nothing is dropped here — the table shows
//! the lot, and a provider picks out only the handful it needs.

use std::path::Path;

use calamine::{Data, Reader as _};

use crate::model::Row;
use crate::source::SourceError;

/// Extensions the file picker offers, for the dialog filter.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["csv", "tsv", "xlsx", "xlsm", "xls", "ods"];

/// Delimiters sniffed in a text export, in the order they are tried.
const DELIMITERS: [u8; 3] = *b",;\t";

/// A file's contents: its columns in the order it wrote them, and its rows.
///
/// The columns are carried separately so that a file with headers and no rows
/// can still be checked against what the chosen provider expects.
#[derive(Debug, Clone, Default)]
pub struct Sheet {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

/// Read every row of `path`, keyed by the file's own column headers.
pub fn read_rows(path: &Path) -> Result<Sheet, SourceError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "csv" | "tsv" | "txt" => read_separated(path),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => read_spreadsheet(path),
        "" => Err(SourceError::UnsupportedFile {
            extension: "no extension".to_owned(),
        }),
        other => Err(SourceError::UnsupportedFile {
            extension: format!(".{other}"),
        }),
    }
}

fn read_separated(path: &Path) -> Result<Sheet, SourceError> {
    let bytes = std::fs::read(path).map_err(|error| SourceError::Unreadable {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;

    // A spreadsheet writing UTF-8 often leads with a byte-order mark, which
    // would otherwise become part of the first column's name.
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let delimiter = sniff_delimiter(text, path);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(text.as_bytes());

    let headers = reader
        .headers()
        .map_err(|error| SourceError::Malformed {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?
        .iter()
        .map(clean_header)
        .collect::<Vec<_>>();
    let headers = disambiguate(headers);

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| SourceError::Malformed {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;

        let mut row = Row::with_capacity(headers.len());
        for (index, header) in headers.iter().enumerate() {
            let value = record.get(index).unwrap_or_default().trim().to_owned();
            row.insert(header.clone(), value);
        }

        if row.values().any(|value| !value.is_empty()) {
            rows.push(row);
        }
    }

    Ok(Sheet {
        columns: headers,
        rows,
    })
}

/// Pick the delimiter that splits the header line into the most columns, so a
/// semicolon-separated European export loads without being asked about.
fn sniff_delimiter(text: &str, path: &Path) -> u8 {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tsv"))
    {
        return b'\t';
    }

    let header = text.lines().next().unwrap_or_default();
    DELIMITERS
        .into_iter()
        .max_by_key(|delimiter| header.bytes().filter(|byte| byte == delimiter).count())
        .unwrap_or(b',')
}

fn read_spreadsheet(path: &Path) -> Result<Sheet, SourceError> {
    let mut workbook =
        calamine::open_workbook_auto(path).map_err(|error| SourceError::Unreadable {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;

    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| SourceError::Empty {
            path: path.display().to_string(),
        })?;

    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|error| SourceError::Malformed {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;

    let mut records = range.rows();
    let headers: Vec<String> = records
        .next()
        .ok_or_else(|| SourceError::Empty {
            path: path.display().to_string(),
        })?
        .iter()
        .map(|cell| clean_header(&cell_text(cell)))
        .collect();
    let headers = disambiguate(headers);

    let mut rows = Vec::new();
    for record in records {
        let mut row = Row::with_capacity(headers.len());
        for (index, header) in headers.iter().enumerate() {
            let value = record.get(index).map(cell_text).unwrap_or_default();
            row.insert(header.clone(), value);
        }

        if row.values().any(|value| !value.is_empty()) {
            rows.push(row);
        }
    }

    Ok(Sheet {
        columns: headers,
        rows,
    })
}

/// Render a cell the way a person would read it: a whole number keeps no
/// decimal point, and a date cell comes out in a shape the date reader knows.
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(text) | Data::DateTimeIso(text) | Data::DurationIso(text) => {
            text.trim().to_owned()
        }
        Data::Int(value) => value.to_string(),
        Data::Float(value) => {
            if value.fract() == 0.0 && value.abs() < 1e15 {
                #[allow(clippy::cast_possible_truncation)]
                let whole = *value as i64;
                whole.to_string()
            } else {
                value.to_string()
            }
        }
        Data::Bool(value) => value.to_string().to_uppercase(),
        Data::DateTime(value) => value.as_datetime().map_or_else(
            || value.to_string(),
            |dt| dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        ),
        Data::Error(error) => {
            tracing::debug!(?error, "spreadsheet cell holds an error value");
            String::new()
        }
    }
}

/// Two columns sharing a name would collapse into one, silently losing a
/// column the user can see in their spreadsheet, so repeats are numbered.
fn disambiguate(headers: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    headers
        .into_iter()
        .map(|header| {
            let count = seen.entry(header.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                header
            } else {
                format!("{header} ({count})")
            }
        })
        .collect()
}

/// Headers arrive with a stray byte-order mark, trailing spaces, or blank, in
/// which case the column still needs a name to be addressable.
fn clean_header(header: &str) -> String {
    let cleaned = header.trim_start_matches('\u{feff}').trim();
    if cleaned.is_empty() {
        "(unnamed)".to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write(name: &str, contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn a_byte_order_mark_does_not_become_part_of_the_first_header() {
        let (_dir, path) = write("export.csv", "\u{feff}Work Item,Duration\nABC-1,1:00:00\n");
        let sheet = read_rows(&path).unwrap();
        assert_eq!(sheet.rows.len(), 1);
        assert_eq!(sheet.columns[0], "Work Item");
        assert_eq!(
            sheet.rows[0].get("Work Item").map(String::as_str),
            Some("ABC-1")
        );
    }

    #[test]
    fn semicolon_separated_exports_load_without_being_asked_about() {
        let (_dir, path) = write("export.csv", "Date;Duration;Task\n2025-05-04;3600;ABC-1\n");
        let sheet = read_rows(&path).unwrap();
        assert_eq!(sheet.rows[0].get("Task").map(String::as_str), Some("ABC-1"));
    }

    #[test]
    fn every_column_survives_the_read_in_the_order_the_file_wrote_them() {
        let (_dir, path) = write("export.csv", "B,A,Extra\n1,2,3\n");
        let sheet = read_rows(&path).unwrap();
        assert_eq!(sheet.columns, ["B", "A", "Extra"]);
    }

    #[test]
    fn blank_lines_are_not_rows() {
        let (_dir, path) = write("export.csv", "A,B\n1,2\n,\n3,4\n");
        assert_eq!(read_rows(&path).unwrap().rows.len(), 2);
    }

    #[test]
    fn repeated_headers_stay_separate_columns() {
        let (_dir, path) = write("export.csv", "Note,Note\na,b\n");
        let sheet = read_rows(&path).unwrap();
        assert_eq!(sheet.columns, ["Note", "Note (2)"]);
        assert_eq!(sheet.rows[0].get("Note (2)").map(String::as_str), Some("b"));
    }

    #[test]
    fn an_unknown_extension_says_so() {
        let (_dir, path) = write("export.pdf", "nope");
        assert!(matches!(
            read_rows(&path),
            Err(SourceError::UnsupportedFile { .. })
        ));
    }
}
