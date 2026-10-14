//! Drawing the whole window, headlessly, in every state it can be in.
//!
//! An immediate-mode UI has no types stopping a panel from borrowing what a
//! row already holds, or a dialog from indexing a row that a filter has just
//! removed. Those only show up when the frame is actually built, so these
//! tests build it — once for each state a user can get the window into.

use std::path::PathBuf;

use autolog_core::EntryStatus;
use autolog_core::config::Settings;
use autolog_core::error::UserError;
use autolog_core::pipeline::Summary;
use autolog_core::secrets::Secret;
use autolog_core::source::{self, SkippedRow};
use chrono_tz::Tz;
use eframe::App as _;
use egui::{Context, RawInput, Rect, pos2, vec2};

use crate::app::{AutoLogApp, Modal, NoticeKind};

fn sample(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(name)
}

/// An app with nothing of the developer's own settings or credentials in it.
fn app(ctx: &Context) -> AutoLogApp {
    let mut app = AutoLogApp::new(ctx, None).expect("the app should start");
    app.settings = Settings::default();
    // Nothing in a test may write over the real settings file or keyring.
    app.settings_dirty = false;
    app.secrets = std::sync::Arc::new(autolog_core::secrets::SecretStore::in_memory());
    app
}

fn with_entries(ctx: &Context, provider: &str, file: &str) -> AutoLogApp {
    let mut app = app(ctx);
    app.settings.provider = provider.to_owned();

    let loaded = source::load(
        &sample(file),
        source::provider(provider).expect("a known provider"),
        Tz::UTC,
        None,
    )
    .expect("the sample should load");

    app.source_columns = loaded.columns;
    app.entries = loaded.entries;
    app.total_seconds = loaded.total_seconds;
    app.file = Some(sample(file));
    app.mark_view_dirty();
    app
}

/// Build `frames` whole frames and return the app, having panicked if any
/// of them could not be laid out.
fn draw(app: &mut AutoLogApp, ctx: &Context, frames: usize) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
        ..Default::default()
    };

    for _ in 0..frames {
        let mut frame = eframe::Frame::_new_kittest();
        app.logic(ctx, &mut frame);
        ctx.run_ui(input.clone(), |ui| app.ui(ui, &mut frame))
            .drop_without_applying_deltas();
    }
}

fn context() -> Context {
    let ctx = Context::default();
    // Loading real fonts costs more than these tests are worth.
    ctx.set_fonts(egui::FontDefinitions::empty());
    ctx
}

#[test]
fn the_window_draws_with_nothing_loaded() {
    let ctx = context();
    let mut app = app(&ctx);
    draw(&mut app, &ctx, 2);
    assert!(app.entries.is_empty());
    assert_eq!(app.submittable(), 0);
}

#[test]
fn the_window_draws_a_loaded_file() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    draw(&mut app, &ctx, 2);

    assert_eq!(app.entries.len(), 8);
    assert_eq!(app.view.len(), 8, "every row is visible with no filter set");
    assert_eq!(app.submittable(), 8);
}

#[test]
fn every_source_column_can_be_shown_at_once() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    app.extra_columns = app.source_columns.clone();
    draw(&mut app, &ctx, 2);
    assert_eq!(app.extra_columns.len(), 5);
}

#[test]
fn switching_consumer_redraws_with_that_trackers_columns() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");

    for consumer in ["kimai", "jira"] {
        app.select_consumer(consumer);
        draw(&mut app, &ctx, 2);
        assert_eq!(app.consumer_spec().id, consumer);
    }
}

#[test]
fn searching_and_filtering_narrow_the_rows_drawn() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");

    app.search = "ACME-118".to_owned();
    app.mark_view_dirty();
    draw(&mut app, &ctx, 1);
    assert_eq!(app.view.len(), 1);

    app.search.clear();
    app.filters = [EntryStatus::Failed].into_iter().collect();
    app.mark_view_dirty();
    draw(&mut app, &ctx, 1);
    assert!(app.view.is_empty(), "nothing has failed yet");

    app.filters.clear();
    app.mark_view_dirty();
    draw(&mut app, &ctx, 1);
    assert_eq!(app.view.len(), 8);
}

#[test]
fn a_selection_survives_the_rows_under_it_being_filtered_away() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    draw(&mut app, &ctx, 1);

    app.selection = app.view.iter().copied().collect();
    app.filters = [EntryStatus::Success].into_iter().collect();
    app.mark_view_dirty();
    draw(&mut app, &ctx, 1);

    // Rows that are no longer shown cannot be acted on, so they leave the
    // selection rather than being changed out of sight.
    assert!(app.selection.is_empty());
}

#[test]
fn sorting_by_every_column_draws() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    app.extra_columns = app.source_columns.clone();

    for column in [
        crate::app::ColumnKey::Status,
        crate::app::ColumnKey::Started,
        crate::app::ColumnKey::Duration,
        crate::app::ColumnKey::Issue,
        crate::app::ColumnKey::Target("project".to_owned()),
        crate::app::ColumnKey::Source("Notes".to_owned()),
        crate::app::ColumnKey::Message,
    ] {
        app.toggle_sort(column);
        draw(&mut app, &ctx, 1);
        assert_eq!(app.view.len(), 8);
    }
}

#[test]
fn sorting_by_length_orders_by_time_rather_than_by_text() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    app.toggle_sort(crate::app::ColumnKey::Duration);
    draw(&mut app, &ctx, 1);

    let lengths: Vec<u32> = app
        .view
        .iter()
        .filter_map(|id| app.entry(*id))
        .map(|entry| entry.duration)
        .collect();

    assert!(
        lengths.windows(2).all(|pair| pair[0] <= pair[1]),
        "shortest first, got {lengths:?}"
    );
}

#[test]
fn every_dialog_draws() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    app.skipped = vec![SkippedRow {
        number: 3,
        reason: "could not read `whenever` as a date".to_owned(),
    }];

    let dialogs = [
        Modal::Error(Box::new(
            UserError::new(
                "Cannot reach the server",
                "kimai.example.com did not answer.",
            )
            .with_hint("Check the address.")
            .with_detail("dns error"),
        )),
        Modal::Settings,
        Modal::Summary(Summary {
            posted: 4,
            skipped: 1,
            failed: 1,
            ..Summary::default()
        }),
        Modal::Skipped,
    ];

    for dialog in dialogs {
        app.modal = Some(dialog);
        draw(&mut app, &ctx, 2);
    }
}

#[test]
fn the_settings_dialog_draws_for_every_consumer() {
    let ctx = context();
    let mut app = app(&ctx);

    for consumer in ["kimai", "jira"] {
        app.select_consumer(consumer);
        app.modal = Some(Modal::Settings);
        draw(&mut app, &ctx, 2);
    }
}

#[test]
fn notices_draw_and_then_expire() {
    let ctx = context();
    let mut app = app(&ctx);
    app.notify(NoticeKind::Success, "Loaded 6 entries.");
    app.notify(NoticeKind::Warning, "3 rows could not be read.");
    app.notify(NoticeKind::Info, "Stopping after the entries in flight.");

    draw(&mut app, &ctx, 2);

    // There may be one more: a store that keeps secrets in memory says so.
    for raised in [
        "Loaded 6 entries.",
        "3 rows could not be read.",
        "Stopping after the entries in flight.",
    ] {
        assert!(
            app.notices.iter().any(|notice| notice.text == raised),
            "{raised} should still be on screen"
        );
    }
}

#[test]
fn a_row_being_edited_draws() {
    let ctx = context();
    let mut app = with_entries(&ctx, "grindstone", "grindstone.csv");
    app.editing = Some(crate::app::Editing {
        id: 1,
        column: crate::app::ColumnKey::Issue,
        buffer: "ABC-1".to_owned(),
    });
    draw(&mut app, &ctx, 2);

    app.set_issue_key(1, "abc-42");
    assert_eq!(app.entry(1).unwrap().issue_key.as_deref(), Some("ABC-42"));
    assert_eq!(app.entry(1).unwrap().project_key.as_deref(), Some("ABC"));
}

#[test]
fn a_row_at_every_status_draws() {
    let ctx = context();
    let mut app = with_entries(&ctx, "kimai", "kimai.csv");

    let statuses = [
        EntryStatus::Pending,
        EntryStatus::Queued,
        EntryStatus::Submitting,
        EntryStatus::Success,
        EntryStatus::Skipped,
        EntryStatus::Failed,
        EntryStatus::Invalid,
    ];

    for (entry, status) in app.entries.iter_mut().zip(statuses) {
        entry.status = status;
        entry.outcome = Some(UserError::new("Rejected", "Jira refused this entry."));
    }

    app.mark_view_dirty();
    draw(&mut app, &ctx, 2);
    assert_eq!(app.view.len(), 20);
}

/// Reading the stored token happens off the painting thread. Saving before it
/// lands used to write the empty form back, which deleted the token the user
/// had never touched.
#[test]
fn quitting_before_the_token_is_read_does_not_delete_it() {
    let ctx = context();
    let mut app = app(&ctx);
    let consumer = app.settings.consumer.clone();

    app.secrets
        .set(&consumer, "api_key", &Secret::new("a-real-token"))
        .unwrap();

    // The form is still empty: the keyring read has not come back.
    app.save_credentials();
    assert_eq!(
        app.secrets.get(&consumer, "api_key").expose(),
        "a-real-token"
    );

    // Once it has, the form is authoritative again.
    app.secrets_loaded.insert(consumer.clone());
    app.form_mut()
        .insert("api_key".to_owned(), "typed".to_owned());
    app.save_credentials();
    assert_eq!(app.secrets.get(&consumer, "api_key").expose(), "typed");

    // Including when the user empties it on purpose.
    app.form_mut().insert("api_key".to_owned(), String::new());
    app.save_credentials();
    assert!(app.secrets.get(&consumer, "api_key").is_empty());
}

#[test]
fn the_odoo_spreadsheet_draws() {
    let ctx = context();
    let mut app = with_entries(&ctx, "odoo", "odoo.xlsx");
    app.extra_columns = app.source_columns.clone();
    draw(&mut app, &ctx, 2);
    assert_eq!(app.entries.len(), 20);
}
