//! The sample exports in `samples/`, read end to end.
//!
//! These are the real files the Python version shipped, so a change that
//! breaks a vendor's actual format fails here rather than in front of a user.

use std::path::PathBuf;

use autolog_core::consumer::kimai;
use autolog_core::consumer::{Consumer as _, Strategies};
use autolog_core::matching::MatchStrategy;
use autolog_core::source::{self, provider};
use chrono_tz::Tz;

fn sample(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(name)
}

#[test]
fn the_grindstone_sample_loads_whole() {
    let loaded = source::load(
        &sample("grindstone.csv"),
        provider("grindstone").unwrap(),
        Tz::Asia__Damascus,
        None,
    )
    .expect("the sample should load");

    assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
    assert_eq!(loaded.entries.len(), 6);
    assert_eq!(
        loaded.columns,
        ["Work Item", "End", "Start", "Duration", "Notes"]
    );

    let first = &loaded.entries[0];
    assert_eq!(first.issue_key.as_deref(), Some("ACME-6055"));
    assert_eq!(first.duration, 3604);
    assert_eq!(first.started.to_string(), "2026-10-12 10:00:25 +03");
    assert_eq!(first.description, "rename upload-queue to upload-worker");

    // The meeting has no issue key, and is left for the user rather than guessed.
    assert_eq!(loaded.entries[1].issue_key, None);
    assert_eq!(loaded.entries[1].activity, "Meetings");

    // `WIDGET-BaseImage` is not an issue key either: the half after the dash
    // has to be a number for a key to be read out of it.
    assert_eq!(loaded.entries[4].issue_key, None);
    assert_eq!(loaded.entries[4].duration, 2 * 3600 + 38 * 60 + 5);
}

#[test]
fn the_kimai_sample_loads_whole() {
    let loaded = source::load(
        &sample("kimai.csv"),
        provider("kimai").unwrap(),
        Tz::UTC,
        None,
    )
    .expect("the sample should load");

    assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
    assert_eq!(loaded.entries.len(), 20);
    assert_eq!(
        loaded.entries[0].started.to_string(),
        "2025-05-04 08:00:00 UTC"
    );
    assert_eq!(loaded.entries[0].duration, 29_800);
    // `5-2-2024` is read month-first, as the Python version read it.
    assert_eq!(
        loaded.entries[3].started.to_string(),
        "2024-05-02 11:00:00 UTC"
    );
}

#[test]
fn the_odoo_spreadsheet_loads_whole() {
    let loaded = source::load(
        &sample("odoo.xlsx"),
        provider("odoo").unwrap(),
        Tz::UTC,
        None,
    )
    .expect("the sample should load");

    assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
    assert_eq!(loaded.entries.len(), 20);
    assert_eq!(loaded.columns, ["Date", "Description", "Quantity", "Task"]);

    let first = &loaded.entries[0];
    assert_eq!(first.issue_key.as_deref(), Some("AUTOLOG-1"));
    // 1.5 hours, started at Odoo's assumed 08:00.
    assert_eq!(first.duration, 5400);
    assert_eq!(first.started.to_string(), "2025-05-04 08:00:00 UTC");
}

#[test]
fn a_file_read_with_the_wrong_provider_says_which_columns_it_wanted() {
    let error = source::load(
        &sample("kimai.csv"),
        provider("grindstone").unwrap(),
        Tz::UTC,
        None,
    )
    .expect_err("a Kimai export is not a Grindstone one");

    let message = autolog_core::UserFacing::to_user_error(&error);
    assert!(message.message.contains("Work Item"), "{}", message.message);
}

/// The whole path a user walks: load a file, then let Kimai match it.
#[test]
fn loaded_entries_can_be_matched_against_a_trackers_names() {
    let loaded = source::load(
        &sample("grindstone.csv"),
        provider("grindstone").unwrap(),
        Tz::UTC,
        None,
    )
    .unwrap();

    let mut entries = loaded.entries;
    let consumer = kimai::Kimai::new(&test_credentials(), true).unwrap();

    // With no names fetched there is nothing to match against, and every row
    // is left for the user rather than filled in with a guess.
    let strategies: Strategies = kimai::SPEC
        .targets
        .iter()
        .map(|field| (field.key.to_owned(), field.default_strategy))
        .collect();
    consumer.assign(&mut entries, &strategies);

    assert!(
        entries
            .iter()
            .all(|entry| entry.target(kimai::PROJECT).is_none())
    );
    assert_eq!(
        strategies.get(kimai::PROJECT),
        Some(&MatchStrategy::Exact),
        "a Kimai project matches exactly unless the user says otherwise"
    );
}

fn test_credentials() -> autolog_core::consumer::Credentials {
    use autolog_core::secrets::Secret;
    let mut credentials = autolog_core::consumer::Credentials::default();
    credentials.set("base_url", Secret::new("https://kimai.example.com"));
    credentials.set("api_token", Secret::new("token"));
    credentials
}
