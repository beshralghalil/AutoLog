//! The window's state, and everything that changes it.
//!
//! State is only ever touched on the painting thread: background work reports
//! back as a [`Message`], which [`AutoLogApp::handle`] applies at the top of
//! the frame. That is what keeps a table being painted at sixty frames a
//! second and a run posting entries from needing a lock between them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use autolog_core::config::{ColumnLayout, Settings, Theme};
use autolog_core::consumer::{self, Consumer, ConsumerSpec, Credentials};
use autolog_core::error::{UserError, UserFacing};
use autolog_core::model::{EntryId, EntryStatus, WorklogEntry};
use autolog_core::pipeline::{self, Cancel, Progress, Summary};
use autolog_core::secrets::{Secret, SecretStore};
use autolog_core::source::{self, SkippedRow};
use autolog_core::{MatchStrategy, matching};

use crate::task::{Message, Tasks};
use crate::theme;

/// Which column of the table a cell belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ColumnKey {
    Status,
    Started,
    Duration,
    Issue,
    /// A destination the consumer needs, by its field key.
    Target(String),
    /// A column straight out of the file.
    Source(String),
    /// Why a row is in the state it is in.
    Message,
}

#[derive(Debug, Clone)]
pub struct Sort {
    pub column: ColumnKey,
    pub ascending: bool,
}

/// How things stand with the tracker.
#[derive(Debug, Default)]
pub enum Connection {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed(Box<UserError>),
}

impl Connection {
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Connecting)
    }
}

/// A posting run in progress.
#[derive(Debug)]
pub struct RunState {
    pub cancel: Cancel,
    pub total: usize,
    pub done: usize,
    pub started: Instant,
}

impl RunState {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "counts are far below f32's exact range"
        )]
        let fraction = self.done as f32 / self.total as f32;
        fraction
    }

    /// Roughly how much longer, once enough entries have settled to guess.
    pub fn remaining(&self) -> Option<Duration> {
        if self.done < 3 || self.done >= self.total {
            return None;
        }
        let each = self
            .started
            .elapsed()
            .div_f64(f64::from(u32::try_from(self.done).ok()?));
        Some(each.mul_f64(f64::from(u32::try_from(self.total - self.done).ok()?)))
    }
}

/// A short-lived message in the corner.
#[derive(Debug)]
pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
    pub raised: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
}

/// Something taking over the window until it is dismissed.
#[derive(Debug)]
pub enum Modal {
    Error(Box<UserError>),
    Settings,
    Summary(Summary),
    /// Rows the file had that could not be read.
    Skipped,
}

/// A cell being typed into.
#[derive(Debug)]
pub struct Editing {
    pub id: EntryId,
    pub column: ColumnKey,
    pub buffer: String,
    /// Whether the caret has been put in the box yet.
    ///
    /// Asking for focus once is the whole point: asking again every frame
    /// would take it straight back each time the user clicked away, leaving a
    /// field they could never leave.
    pub focused: bool,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate thing the window is waiting on"
)]
pub struct AutoLogApp {
    pub settings: Settings,
    pub secrets: Arc<SecretStore>,
    pub tasks: Tasks,
    pub log_directory: Option<PathBuf>,

    // The loaded file.
    pub file: Option<PathBuf>,
    pub loading: bool,
    pub entries: Vec<WorklogEntry>,
    pub source_columns: Vec<String>,
    pub skipped: Vec<SkippedRow>,
    pub total_seconds: u64,

    // The tracker.
    /// Credential form values, by consumer then field. Secret fields are read
    /// from the system store into here so they can be edited, and written back
    /// there on save; they never reach the config file.
    pub forms: BTreeMap<String, BTreeMap<String, String>>,
    /// Consumers whose stored secrets have been read back into `forms`. Until
    /// a consumer is in here, its secrets are read-only.
    pub secrets_loaded: BTreeSet<String>,
    pub consumer: Option<Arc<dyn Consumer>>,
    pub connection: Connection,
    /// Every name each destination field can take, by field key. Filled on
    /// connect, because the toolbar's bulk pickers need it on every frame.
    pub bulk_choices: BTreeMap<String, Vec<String>>,

    // The table.
    pub search: String,
    pub filters: BTreeSet<EntryStatus>,
    pub sort: Option<Sort>,
    pub selection: BTreeSet<EntryId>,
    pub editing: Option<Editing>,
    /// Source columns shown beyond the ones AutoLog always shows.
    pub extra_columns: Vec<String>,
    /// Rows to draw, filtered and sorted; rebuilt when something changes.
    pub view: Vec<EntryId>,
    view_dirty: bool,

    // Everything else.
    pub run: Option<RunState>,
    pub notices: Vec<Notice>,
    pub modal: Option<Modal>,
    pub settings_dirty: bool,
    /// The appearance currently painted, so the palette is only rebuilt when
    /// the desktop or the setting actually changes.
    pub applied_theme: Option<egui::Theme>,
    /// A file named on the command line, loaded once the window is up.
    pub opened_with: Option<PathBuf>,
    /// Set once the first frame has run, so startup work happens after the
    /// window is on screen rather than before it.
    started: bool,
}

impl AutoLogApp {
    pub fn new(ctx: &egui::Context, log_directory: Option<PathBuf>) -> anyhow::Result<Self> {
        let settings = Settings::load();
        theme::apply(ctx, theme::resolve(settings.theme, ctx));

        Ok(Self {
            extra_columns: settings.layout(&settings.provider).extra,
            settings,
            secrets: Arc::new(SecretStore::new()),
            tasks: Tasks::new(ctx)?,
            log_directory,
            file: None,
            loading: false,
            entries: Vec::new(),
            source_columns: Vec::new(),
            skipped: Vec::new(),
            total_seconds: 0,
            forms: BTreeMap::new(),
            consumer: None,
            secrets_loaded: BTreeSet::new(),
            connection: Connection::default(),
            bulk_choices: BTreeMap::new(),
            search: String::new(),
            filters: BTreeSet::new(),
            sort: None,
            selection: BTreeSet::new(),
            editing: None,
            view: Vec::new(),
            view_dirty: true,
            run: None,
            notices: Vec::new(),
            modal: None,
            settings_dirty: false,
            applied_theme: None,
            opened_with: None,
            started: false,
        })
    }

    // ----- what is selected -------------------------------------------------

    pub fn consumer_spec(&self) -> &'static ConsumerSpec {
        consumer::spec(&self.settings.consumer).unwrap_or(&consumer::jira::SPEC)
    }

    pub fn provider_spec(&self) -> &'static source::ProviderSpec {
        source::provider(&self.settings.provider).unwrap_or(&source::PROVIDERS[0])
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.connection, Connection::Connected) && self.consumer.is_some()
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// The credential form for the selected consumer, creating it on first use.
    pub fn form_mut(&mut self) -> &mut BTreeMap<String, String> {
        let consumer = self.settings.consumer.clone();
        self.forms.entry(consumer).or_default()
    }

    pub fn form_value(&self, field: &str) -> &str {
        self.forms
            .get(&self.settings.consumer)
            .and_then(|form| form.get(field))
            .map_or("", String::as_str)
    }

    /// Whether every field the tracker needs has something in it.
    pub fn credentials_complete(&self) -> bool {
        self.consumer_spec()
            .credentials
            .iter()
            .all(|field| !self.form_value(field.key).trim().is_empty())
    }

    fn credentials(&self) -> Credentials {
        let mut credentials = Credentials::default();
        for field in self.consumer_spec().credentials {
            credentials.set(field.key, Secret::new(self.form_value(field.key).trim()));
        }
        credentials
    }

    fn strategies(&self) -> consumer::Strategies {
        consumer::strategies(self.consumer_spec(), &self.settings)
    }

    // ----- notices and errors ----------------------------------------------

    pub fn notify(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.notices.push(Notice {
            text: text.into(),
            kind,
            raised: Instant::now(),
        });
    }

    pub fn report(&mut self, error: &impl UserFacing) {
        let error = error.to_user_error();
        tracing::warn!("{error}");
        self.modal = Some(Modal::Error(Box::new(error)));
    }

    /// Drop notices that have had their time on screen.
    pub fn expire_notices(&mut self) {
        self.notices
            .retain(|notice| notice.raised.elapsed() < Duration::from_secs(6));
    }

    // ----- loading a file ---------------------------------------------------

    pub fn pick_file(&mut self) {
        let mut dialog =
            rfd::FileDialog::new().add_filter("Worklog export", source::SUPPORTED_EXTENSIONS);
        if let Some(directory) = &self.settings.last_directory {
            dialog = dialog.set_directory(directory);
        }
        if let Some(path) = dialog.pick_file() {
            self.load(path);
        }
    }

    pub fn load(&mut self, path: PathBuf) {
        let spec = self.provider_spec();
        let timezone = self.settings.timezone;
        let order = self.settings.date_order;
        let reading = path.clone();

        self.loading = true;
        self.settings.remember_directory(&path);
        self.settings_dirty = true;

        self.tasks.spawn_blocking(move || Message::Loaded {
            path: reading.clone(),
            result: Box::new(source::load(&reading, spec, timezone, order)),
        });
        self.file = Some(path);
    }

    /// Read the file again, keeping the provider and zone currently chosen.
    pub fn reload(&mut self) {
        if let Some(path) = self.file.clone() {
            self.load(path);
        }
    }

    // ----- connecting -------------------------------------------------------

    /// Sign in and fetch the tracker's names.
    ///
    /// Also used by Refresh: re-connecting is how the lists are brought up to
    /// date, and it keeps one path rather than two that can drift.
    pub fn connect(&mut self) {
        if !self.credentials_complete() {
            self.notify(NoticeKind::Warning, "Fill in every credential first.");
            return;
        }

        self.save_credentials();
        self.connection = Connection::Connecting;

        let id = self.settings.consumer.clone();
        let credentials = self.credentials();
        let prevent_duplicates = self.settings.prevent_duplicates;
        let reporting_id = id.clone();

        self.tasks.spawn(async move {
            let result = match consumer::build(&id, &credentials, prevent_duplicates) {
                Ok(mut consumer) => match consumer.connect().await {
                    Ok(()) => Ok(consumer),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };

            Message::Connected {
                consumer_id: reporting_id,
                result,
            }
        });
    }

    /// Write the credentials where each half belongs: secrets to the system
    /// store, the rest to the config file.
    pub fn save_credentials(&mut self) {
        let consumer = self.settings.consumer.clone();
        let mut failure = None;

        // Reading the stored token happens off the painting thread, so the
        // form is briefly empty after a consumer is selected. Writing that
        // emptiness back would delete the token the user never touched, so
        // secrets are only saved once what was stored has been read in.
        let readable = self.secrets_loaded.contains(&consumer);

        for field in self.consumer_spec().credentials {
            let value = self.form_value(field.key).trim().to_owned();
            if field.secret {
                if !readable {
                    continue;
                }
                if let Err(error) = self.secrets.set(&consumer, field.key, &Secret::new(value)) {
                    failure = Some(error);
                }
            } else {
                self.settings.set_account_field(&consumer, field.key, value);
            }
        }

        self.settings_dirty = true;
        if let Some(error) = failure {
            self.report(&error);
        }
    }

    /// Read a consumer's saved credentials into its form.
    pub fn load_credentials(&mut self, consumer_id: &str) {
        let Some(spec) = consumer::spec(consumer_id) else {
            return;
        };

        let account = self.settings.account(consumer_id);
        let form = self.forms.entry(consumer_id.to_owned()).or_default();
        for field in spec.credentials {
            if !field.secret {
                form.insert(
                    field.key.to_owned(),
                    account.get(field.key).cloned().unwrap_or_default(),
                );
            }
        }

        // The keyring can take a noticeable moment, so it is not asked on the
        // painting thread.
        let secrets = Arc::clone(&self.secrets);
        let id = consumer_id.to_owned();
        self.tasks.spawn_blocking(move || {
            let result = spec
                .credentials
                .iter()
                .filter(|field| field.secret)
                .map(|field| {
                    secrets
                        .try_get(&id, field.key)
                        .map(|found| (field.key.to_owned(), found.expose().to_owned()))
                })
                .collect();

            Message::CredentialsLoaded {
                consumer_id: id,
                result,
            }
        });
    }

    /// Switch tracker: save what was typed, swap the form, drop the connection.
    pub fn select_consumer(&mut self, id: &str) {
        if self.settings.consumer == id {
            return;
        }

        self.save_credentials();
        id.clone_into(&mut self.settings.consumer);
        self.settings_dirty = true;
        self.consumer = None;
        self.connection = Connection::Disconnected;
        self.bulk_choices.clear();
        self.editing = None;
        self.clear_targets();
        self.load_credentials(id);
        self.mark_view_dirty();
    }

    pub fn select_provider(&mut self, id: &str) {
        if self.settings.provider == id {
            return;
        }

        self.remember_columns();
        id.clone_into(&mut self.settings.provider);
        self.extra_columns = self.settings.layout(id).extra;
        self.settings_dirty = true;

        if self.file.is_some() {
            self.reload();
        }
    }

    /// Forget guesses made for a tracker that is no longer selected.
    fn clear_targets(&mut self) {
        for entry in &mut self.entries {
            entry.targets.clear();
            entry.reset();
        }
    }

    // ----- matching ---------------------------------------------------------

    /// Fill in the destinations that are still blank.
    pub fn assign_targets(&mut self) {
        let Some(consumer) = self.consumer.clone() else {
            return;
        };
        let strategies = self.strategies();
        consumer.assign(&mut self.entries, &strategies);
        self.mark_view_dirty();
    }

    /// Match every row again from scratch, throwing away earlier guesses.
    ///
    /// This is what changing a match strategy means: the rows the old setting
    /// filled in have to be reconsidered, or the setting appears to do nothing.
    pub fn rematch_all(&mut self) {
        let Some(consumer) = self.consumer.clone() else {
            return;
        };

        let strategies = self.strategies();
        for entry in &mut self.entries {
            if entry.status != EntryStatus::Success {
                entry.targets.clear();
            }
        }
        consumer.assign(&mut self.entries, &strategies);
        self.mark_view_dirty();
    }

    /// Set one destination on one entry.
    pub fn retarget(&mut self, id: EntryId, field: &str, value: &str) {
        let Some(consumer) = self.consumer.clone() else {
            return;
        };
        let strategies = self.strategies();
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            consumer.retarget(entry, field, value, &strategies);
        }
        self.mark_view_dirty();
    }

    /// Set one destination on every row the change should reach: the selection
    /// if there is one, otherwise everything on screen.
    pub fn retarget_many(&mut self, field: &str, value: &str) {
        let Some(consumer) = self.consumer.clone() else {
            return;
        };
        let strategies = self.strategies();
        let targets = self.affected_ids();

        for entry in &mut self.entries {
            if targets.contains(&entry.id) && entry.status != EntryStatus::Success {
                consumer.retarget(entry, field, value, &strategies);
            }
        }

        self.notify(
            NoticeKind::Success,
            format!("Set {field} on {} entries.", targets.len()),
        );
        self.mark_view_dirty();
    }

    /// The rows a bulk action applies to.
    pub fn affected_ids(&self) -> BTreeSet<EntryId> {
        if self.selection.is_empty() {
            self.view.iter().copied().collect()
        } else {
            self.selection.clone()
        }
    }

    pub fn set_issue_key(&mut self, id: EntryId, value: &str) {
        let trimmed = value.trim();
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.issue_key = (!trimmed.is_empty()).then(|| trimmed.to_uppercase());
            entry.project_key = entry
                .issue_key
                .as_deref()
                .and_then(|key| key.split('-').next().map(ToOwned::to_owned));
            entry.reset();
        }
        self.mark_view_dirty();
    }

    // ----- running ----------------------------------------------------------

    /// Check every entry without sending anything.
    pub fn dry_run(&mut self) {
        let Some(consumer) = self.consumer.clone() else {
            self.notify(NoticeKind::Warning, "Connect to the tracker first.");
            return;
        };

        let outcomes = pipeline::dry_run(consumer.as_ref(), &self.entries);
        let mut problems = 0;

        for outcome in outcomes {
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == outcome.id) {
                if outcome.status == EntryStatus::Invalid {
                    problems += 1;
                    entry.status = EntryStatus::Invalid;
                    entry.outcome = outcome.error;
                } else if entry.status == EntryStatus::Invalid {
                    entry.status = EntryStatus::Pending;
                    entry.outcome = None;
                }
            }
        }

        if problems == 0 {
            self.notify(
                NoticeKind::Success,
                "Every entry is ready. Nothing was sent.",
            );
        } else {
            self.notify(
                NoticeKind::Warning,
                format!("{problems} entries are not ready. Nothing was sent."),
            );
            self.filters = [EntryStatus::Invalid].into_iter().collect();
        }

        self.mark_view_dirty();
    }

    /// How many rows a run would attempt.
    pub fn submittable(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status.is_submittable())
            .count()
    }

    pub fn start_run(&mut self) {
        if !self.is_connected() {
            self.notify(NoticeKind::Warning, "Connect to the tracker first.");
            return;
        }

        let mut queued: Vec<WorklogEntry> = Vec::new();
        for entry in &mut self.entries {
            if entry.status.is_submittable() {
                entry.status = EntryStatus::Queued;
                entry.outcome = None;
                queued.push(entry.clone());
            }
        }

        if queued.is_empty() {
            self.notify(NoticeKind::Info, "Nothing left to post.");
            return;
        }

        self.save_credentials();

        let cancel = Cancel::new();
        self.run = Some(RunState {
            cancel: cancel.clone(),
            total: queued.len(),
            done: 0,
            started: Instant::now(),
        });

        let id = self.settings.consumer.clone();
        let credentials = self.credentials();
        let prevent_duplicates = self.settings.prevent_duplicates;
        let limit = self.settings.rate_limit;

        self.tasks.spawn_reporting(move |emitter| async move {
            // The run gets its own connection so the window's copy is never
            // borrowed across an await, and so the names are read fresh.
            let mut consumer = match consumer::build(&id, &credentials, prevent_duplicates) {
                Ok(consumer) => consumer,
                Err(error) => return emitter.aborted(error.to_user_error()),
            };

            if let Err(error) = consumer.connect().await {
                return emitter.aborted(error.to_user_error());
            }

            if let Err(error) = consumer.preload(&queued).await {
                // Not fatal: it only means duplicates may go unnoticed.
                tracing::warn!("could not read back what is already logged: {error}");
                emitter.failed(
                    UserError::new(
                        "Could not check for duplicates",
                        "AutoLog could not read what is already logged, so entries may be posted twice.",
                    )
                    .with_detail(error.to_string()),
                );
            }

            let reporter = emitter.clone();
            pipeline::run(Arc::from(consumer), queued, limit, cancel, move |progress| {
                reporter.progress(progress);
            })
            .await;
        });
    }

    pub fn stop_run(&mut self) {
        if let Some(run) = &self.run {
            run.cancel.stop();
            self.notify(NoticeKind::Info, "Stopping after the entries in flight.");
        }
    }

    // ----- the visible rows -------------------------------------------------

    pub fn mark_view_dirty(&mut self) {
        self.view_dirty = true;
    }

    /// Rebuild the filtered, sorted list of rows to draw.
    pub fn rebuild_view(&mut self) {
        if !self.view_dirty {
            return;
        }
        self.view_dirty = false;

        let needle = self.search.trim().to_lowercase();
        let mut visible: Vec<EntryId> = self
            .entries
            .iter()
            .filter(|entry| self.filters.is_empty() || self.filters.contains(&entry.status))
            .filter(|entry| needle.is_empty() || Self::matches_search(entry, &needle))
            .map(|entry| entry.id)
            .collect();

        if let Some(sort) = &self.sort {
            let by = |id: &EntryId| self.sort_key(*id, &sort.column);
            visible.sort_by(|left, right| {
                let ordering = by(left).cmp(&by(right));
                if sort.ascending {
                    ordering
                } else {
                    ordering.reverse()
                }
            });
        }

        // A row that is no longer shown cannot be acted on, so it leaves the
        // selection rather than being changed out of sight. Done against a set
        // because both sides can be the length of the file.
        let shown: std::collections::HashSet<EntryId> = visible.iter().copied().collect();
        self.selection.retain(|id| shown.contains(id));
        self.view = visible;
    }

    /// Search looks at everything on screen, including the source columns, so
    /// a term the user can see is a term they can search for.
    fn matches_search(entry: &WorklogEntry, needle: &str) -> bool {
        entry.activity.to_lowercase().contains(needle)
            || entry.description.to_lowercase().contains(needle)
            || entry
                .issue_key
                .as_deref()
                .is_some_and(|key| key.to_lowercase().contains(needle))
            || entry
                .targets
                .values()
                .any(|value| value.to_lowercase().contains(needle))
            || entry
                .raw
                .values()
                .any(|value| value.to_lowercase().contains(needle))
    }

    fn sort_key(&self, id: EntryId, column: &ColumnKey) -> String {
        let Some(entry) = self.entries.iter().find(|entry| entry.id == id) else {
            return String::new();
        };

        match column {
            ColumnKey::Status => entry.status.label().to_owned(),
            ColumnKey::Started => entry.start_utc().to_rfc3339(),
            // Zero-padded so `9m` sorts below `1h 00m` rather than above it.
            ColumnKey::Duration => format!("{:012}", entry.duration),
            ColumnKey::Issue => entry.issue_key.clone().unwrap_or_default(),
            ColumnKey::Target(field) => entry.target(field).unwrap_or_default().to_owned(),
            ColumnKey::Source(column) => entry.raw.get(column).cloned().unwrap_or_default(),
            ColumnKey::Message => entry
                .outcome
                .as_ref()
                .map(UserError::summary)
                .unwrap_or_default(),
        }
    }

    pub fn toggle_sort(&mut self, column: ColumnKey) {
        self.sort = match self.sort.take() {
            Some(sort) if sort.column == column && sort.ascending => Some(Sort {
                column,
                ascending: false,
            }),
            Some(sort) if sort.column == column => None,
            _ => Some(Sort {
                column,
                ascending: true,
            }),
        };
        self.mark_view_dirty();
    }

    /// The entry with this id.
    ///
    /// An id is the row's position at load and nothing reorders `entries`, so
    /// this is an index rather than a search. The table asks for one of these
    /// per visible cell per frame, which a scan over a long export would not
    /// survive.
    pub fn entry(&self, id: EntryId) -> Option<&WorklogEntry> {
        self.entries.get(id).filter(|entry| entry.id == id)
    }

    /// How many rows sit at each status, for the filter chips.
    pub fn counts(&self) -> BTreeMap<EntryStatus, usize> {
        let mut counts = BTreeMap::new();
        for entry in &self.entries {
            *counts.entry(entry.status).or_insert(0) += 1;
        }
        counts
    }

    // ----- settings ---------------------------------------------------------

    pub fn set_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.settings.theme = theme;
        self.settings_dirty = true;
        theme::apply(ctx, theme::resolve(theme, ctx));
    }

    pub fn set_strategy(&mut self, field: &str, strategy: MatchStrategy) {
        let consumer = self.settings.consumer.clone();
        self.settings.set_strategy(&consumer, field, strategy);
        self.settings_dirty = true;
        self.rematch_all();
    }

    pub fn strategy_for(&self, field: &str) -> MatchStrategy {
        let fallback = self
            .consumer_spec()
            .target(field)
            .map_or(matching::MatchStrategy::Fuzzy, |spec| spec.default_strategy);
        self.settings
            .strategy(&self.settings.consumer, field, fallback)
    }

    fn remember_columns(&mut self) {
        let provider = self.settings.provider.clone();
        self.settings.set_layout(
            &provider,
            ColumnLayout {
                extra: self.extra_columns.clone(),
            },
        );
        self.settings_dirty = true;
    }

    pub fn toggle_column(&mut self, column: &str) {
        if let Some(position) = self.extra_columns.iter().position(|shown| shown == column) {
            self.extra_columns.remove(position);
        } else {
            self.extra_columns.push(column.to_owned());
        }
        self.remember_columns();
    }

    pub fn save_settings(&mut self) {
        if !self.settings_dirty {
            return;
        }
        self.remember_columns();
        self.settings_dirty = false;

        if let Err(error) = self.settings.save() {
            tracing::warn!("{error}");
        }
    }

    // ----- messages from the background -------------------------------------

    /// Apply everything that arrived since the last frame.
    pub fn pump(&mut self) {
        let messages: Vec<Message> = self.tasks.drain().collect();
        for message in messages {
            self.handle(message);
        }
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Loaded { path, result } => self.on_loaded(&path, *result),
            Message::Connected {
                consumer_id,
                result,
            } => self.on_connected(&consumer_id, result),
            Message::Run(progress) => self.on_progress(progress),
            Message::RunFailed(error) => self.modal = Some(Modal::Error(error)),
            Message::CredentialsLoaded {
                consumer_id,
                result,
            } => match result {
                Ok(values) => {
                    let form = self.forms.entry(consumer_id.clone()).or_default();
                    for (field, value) in values {
                        // Anything the user has typed in the meantime wins
                        // over what was stored.
                        form.entry(field).or_insert(value);
                    }
                    self.secrets_loaded.insert(consumer_id);
                }
                Err(error) => {
                    // The consumer is deliberately left out of `secrets_loaded`,
                    // so nothing will be written over a token that is there but
                    // unreadable.
                    tracing::warn!(consumer = %consumer_id, "{error}");
                    let shown = error.to_user_error();
                    self.notify(
                        NoticeKind::Warning,
                        format!("{} {}", shown.message, shown.hint.unwrap_or_default()),
                    );
                }
            },
        }
    }

    fn on_loaded(
        &mut self,
        path: &std::path::Path,
        result: Result<source::LoadedFile, source::SourceError>,
    ) {
        self.loading = false;

        match result {
            Ok(loaded) => {
                self.entries = loaded.entries;
                self.source_columns = loaded.columns;
                self.skipped = loaded.skipped;
                self.total_seconds = loaded.total_seconds;
                self.selection.clear();
                self.editing = None;
                self.filters.clear();

                // Columns remembered for this provider may not be in this
                // file; drop those rather than show empty columns.
                self.extra_columns
                    .retain(|column| self.source_columns.contains(column));

                if self.entries.is_empty() {
                    self.notify(
                        NoticeKind::Warning,
                        "No entries could be read from that file.",
                    );
                } else {
                    self.notify(
                        NoticeKind::Success,
                        format!(
                            "Loaded {} entries from {}.",
                            self.entries.len(),
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    );
                }

                if !self.skipped.is_empty() {
                    self.modal = Some(Modal::Skipped);
                }

                self.assign_targets();
                self.mark_view_dirty();
            }
            Err(error) => {
                self.entries.clear();
                self.source_columns.clear();
                self.skipped.clear();
                self.file = None;
                self.report(&error);
                self.mark_view_dirty();
            }
        }
    }

    fn on_connected(
        &mut self,
        consumer_id: &str,
        result: Result<Box<dyn Consumer>, consumer::ConsumerError>,
    ) {
        // The user may have switched trackers while this was in flight.
        if consumer_id != self.settings.consumer {
            return;
        }

        match result {
            Ok(consumer) => {
                let consumer: Arc<dyn Consumer> = Arc::from(consumer);
                self.bulk_choices = self
                    .consumer_spec()
                    .targets
                    .iter()
                    .map(|field| (field.key.to_owned(), consumer.every_choice(field.key)))
                    .collect();
                self.consumer = Some(consumer);
                self.connection = Connection::Connected;
                self.notify(
                    NoticeKind::Success,
                    format!("Connected to {}.", self.consumer_spec().label),
                );
                self.assign_targets();
            }
            Err(error) => {
                let error = error.to_user_error();
                self.consumer = None;
                self.connection = Connection::Failed(Box::new(error.clone()));
                self.modal = Some(Modal::Error(Box::new(error)));
            }
        }
    }

    fn on_progress(&mut self, progress: Progress) {
        match progress {
            Progress::Started { total } => {
                if let Some(run) = &mut self.run {
                    run.total = total;
                }
            }
            Progress::Sending { id } => {
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                    entry.status = EntryStatus::Submitting;
                }
            }
            Progress::Settled { outcome, done, .. } => {
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == outcome.id) {
                    entry.status = outcome.status;
                    entry.outcome = outcome.error;
                }
                if let Some(run) = &mut self.run {
                    run.done = done;
                }
                self.mark_view_dirty();
            }
            Progress::Finished(summary) => {
                // Anything still queued was never attempted.
                for entry in &mut self.entries {
                    if matches!(entry.status, EntryStatus::Queued | EntryStatus::Submitting) {
                        entry.status = EntryStatus::Pending;
                    }
                }
                self.run = None;
                self.mark_view_dirty();

                if summary.attempted() > 0 || summary.cancelled > 0 {
                    self.modal = Some(Modal::Summary(summary));
                }
            }
        }
    }

    /// Work that needs the window to exist first.
    pub fn on_first_frame(&mut self) {
        if self.started {
            return;
        }
        self.started = true;

        let consumer = self.settings.consumer.clone();
        self.load_credentials(&consumer);

        if let Some(error) = self.secrets.unavailable() {
            let error = error.to_user_error();
            self.notify(NoticeKind::Warning, error.message.clone());
        }

        if let Some(path) = self.opened_with.take() {
            self.load(path);
        }
    }
}
