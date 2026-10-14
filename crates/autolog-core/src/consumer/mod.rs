//! Where entries are sent, and how each tracker's own names are guessed.
//!
//! A consumer is described by a [`ConsumerSpec`] — the credentials it needs
//! and the destination fields each entry must be given — and implements
//! [`Consumer`] to talk to its API. Everything about *guessing* those
//! destinations is handled here once, driven by the spec, so a new tracker
//! only writes the calls that are actually its own.

pub mod jira;
pub mod kimai;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::error::{UserError, UserFacing};
use crate::http::HttpError;
use crate::matching::{self, MatchStrategy};
use crate::model::WorklogEntry;
use crate::secrets::Secret;

/// One field on the credentials form.
#[derive(Debug, Clone, Copy)]
pub struct CredentialField {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    /// Secret fields go to the system credential store and are never written
    /// to the config file or shown in full.
    pub secret: bool,
}

/// Where a hint for matching comes from on an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// `ACME` out of `ACME-6055`.
    ProjectKey,
    /// `ACME-6055`.
    IssueKey,
    /// The work item exactly as the file wrote it.
    RawIssue,
    /// The provider's name for the work.
    Activity,
}

/// A destination the user must give each entry, e.g. a Kimai project.
#[derive(Debug, Clone, Copy)]
pub struct TargetFieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    /// One line explaining what this field is, for the settings panel.
    pub about: &'static str,
    /// How hard to guess, unless the user has said otherwise in settings.
    pub default_strategy: MatchStrategy,
    /// Hints to match on, most trusted first.
    pub hints: &'static [Hint],
    /// Field this one is scoped by. When that field changes, this one is
    /// matched again — a Kimai activity belongs to one project, so choosing a
    /// different project invalidates it.
    pub scoped_by: Option<&'static str>,
}

/// Everything the UI needs to know about a tracker before connecting.
#[derive(Debug, Clone, Copy)]
pub struct ConsumerSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub credentials: &'static [CredentialField],
    pub targets: &'static [TargetFieldSpec],
}

impl ConsumerSpec {
    pub fn target(&self, key: &str) -> Option<&'static TargetFieldSpec> {
        self.targets.iter().find(|field| field.key == key)
    }
}

/// Credential values as typed, secret and not alike.
#[derive(Debug, Clone, Default)]
pub struct Credentials(BTreeMap<String, Secret>);

impl Credentials {
    pub fn get(&self, key: &str) -> &str {
        self.0.get(key).map_or("", Secret::expose)
    }

    pub fn set(&mut self, key: &str, value: Secret) {
        self.0.insert(key.to_owned(), value);
    }

    /// Check every field the tracker needs is filled, before a pointless
    /// round trip tells the user the same thing more slowly.
    pub fn check(&self, spec: &ConsumerSpec) -> Result<(), ConsumerError> {
        for field in spec.credentials {
            if self.get(field.key).trim().is_empty() {
                return Err(ConsumerError::MissingCredential { label: field.label });
            }
        }
        Ok(())
    }
}

/// Match strategies in force, by destination field key.
pub type Strategies = BTreeMap<String, MatchStrategy>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConsumerError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("{label} is required")]
    MissingCredential { label: &'static str },
    #[error("this entry has no {what}")]
    Incomplete { what: String },
    #[error("already logged")]
    Duplicate,
    #[error("not connected yet")]
    NotConnected,
}

impl UserFacing for ConsumerError {
    fn to_user_error(&self) -> UserError {
        match self {
            Self::Http(error) => error.to_user_error(),
            Self::MissingCredential { label } => {
                UserError::new("Missing credentials", format!("{label} is required."))
                    .with_hint("Fill it in above and try again.")
            }
            Self::Incomplete { what } => {
                UserError::new("Incomplete entry", format!("This entry has no {what}."))
                    .with_hint("Pick one in the table, or use the bulk pickers in the toolbar.")
            }
            Self::Duplicate => UserError::new(
                "Already logged",
                "An entry with the same start, length and comment is already there.",
            )
            .with_hint(
                "Nothing was sent. Turn off duplicate checking in Settings to post it anyway.",
            ),
            Self::NotConnected => UserError::new(
                "Not connected",
                "AutoLog has not signed in to the tracker yet.",
            )
            .with_hint("Press Connect first."),
        }
    }
}

/// The record a tracker kept of an entry we sent.
#[derive(Debug, Clone, Default)]
pub struct Posted {
    /// The tracker's own id for it, when it gives one.
    pub reference: Option<String>,
}

/// A destination for worklog entries.
///
/// Implementors write only what is genuinely theirs: signing in, listing their
/// names, reading back what is already logged, and posting. Guessing which
/// name an entry belongs to is done once, by the provided methods below.
#[async_trait::async_trait]
pub trait Consumer: Send + Sync + std::fmt::Debug {
    fn spec(&self) -> &'static ConsumerSpec;

    /// Sign in and fetch whatever the tracker needs to be asked for once.
    async fn connect(&mut self) -> Result<(), ConsumerError>;

    /// Fetch the tracker's names again, for the Refresh button.
    async fn refresh(&mut self) -> Result<(), ConsumerError>;

    /// The names this entry could be filed under for `field`, narrowed by
    /// whatever scoping field it has already been given.
    fn choices(&self, field: &str, entry: &WorklogEntry) -> Vec<String>;

    /// Every name `field` can take, ignoring the scoping that narrows the
    /// per-row list.
    ///
    /// This is what a bulk picker offers, since the value it sets has to suit
    /// rows that are scoped differently from one another.
    fn every_choice(&self, field: &str) -> Vec<String>;

    /// Everything that would stop this entry being posted, said plainly.
    /// Called by the dry run, and again before posting.
    fn validate(&self, entry: &WorklogEntry) -> Result<(), ConsumerError>;

    /// Read back what the tracker already has, so duplicates can be spotted
    /// without a round trip per entry.
    async fn preload(&mut self, entries: &[WorklogEntry]) -> Result<(), ConsumerError>;

    /// Whether `entry` is already logged. Consulted only when the user has
    /// duplicate prevention on.
    fn is_duplicate(&self, entry: &WorklogEntry) -> bool;

    /// Post one entry.
    async fn submit(&self, entry: &WorklogEntry) -> Result<Posted, ConsumerError>;

    /// Fill in every destination the entry is still missing.
    ///
    /// Only gaps are filled, so a refresh keeps whatever the user chose by
    /// hand — which is what makes Refresh safe to press mid-review.
    fn assign(&self, entries: &mut [WorklogEntry], strategies: &Strategies) {
        for entry in entries {
            for field in self.spec().targets {
                if entry.target(field.key).is_none() {
                    self.match_field(entry, field, strategies);
                }
            }
        }
    }

    /// Set one destination by hand, and re-match anything scoped by it.
    fn retarget(
        &self,
        entry: &mut WorklogEntry,
        field: &str,
        value: &str,
        strategies: &Strategies,
    ) {
        entry.set_target(field, value);

        for dependent in self
            .spec()
            .targets
            .iter()
            .filter(|spec| spec.scoped_by == Some(field))
        {
            entry.set_target(dependent.key, "");
            self.match_field(entry, dependent, strategies);
        }

        entry.reset();
    }

    /// Guess one field, leaving it empty when nothing is close enough.
    fn match_field(
        &self,
        entry: &mut WorklogEntry,
        field: &TargetFieldSpec,
        strategies: &Strategies,
    ) {
        let strategy = strategies
            .get(field.key)
            .copied()
            .unwrap_or(field.default_strategy);

        let choices = self.choices(field.key, entry);
        let hints = hints_for(entry, field.hints);
        let matched = matching::best_match(
            choices.iter().map(String::as_str),
            &hints.iter().map(String::as_str).collect::<Vec<_>>(),
            strategy,
            matching::DEFAULT_THRESHOLD,
        );

        entry.set_target(field.key, matched.unwrap_or_default());
    }
}

/// Read the hint values named by a field's spec off an entry.
fn hints_for(entry: &WorklogEntry, wanted: &[Hint]) -> Vec<String> {
    wanted
        .iter()
        .map(|hint| match hint {
            Hint::ProjectKey => entry.project_key.clone().unwrap_or_default(),
            Hint::IssueKey => entry.issue_key.clone().unwrap_or_default(),
            Hint::RawIssue => entry.raw_issue_key.clone(),
            Hint::Activity => entry.activity.clone(),
        })
        .filter(|hint| !hint.is_empty())
        .collect()
}

/// Every tracker AutoLog can post to.
pub static CONSUMERS: LazyLock<Vec<&'static ConsumerSpec>> =
    LazyLock::new(|| vec![&jira::SPEC, &kimai::SPEC]);

pub fn spec(id: &str) -> Option<&'static ConsumerSpec> {
    CONSUMERS
        .iter()
        .copied()
        .find(|spec| spec.id.eq_ignore_ascii_case(id))
}

/// Build the consumer named by `id`, ready to connect.
pub fn build(
    id: &str,
    credentials: &Credentials,
    prevent_duplicates: bool,
) -> Result<Box<dyn Consumer>, ConsumerError> {
    match id.to_lowercase().as_str() {
        "kimai" => Ok(Box::new(kimai::Kimai::new(
            credentials,
            prevent_duplicates,
        )?)),
        _ => Ok(Box::new(jira::Jira::new(credentials, prevent_duplicates)?)),
    }
}

/// The strategies in force for one consumer, taking the user's overrides over
/// each field's default.
pub fn strategies(spec: &ConsumerSpec, settings: &crate::config::Settings) -> Strategies {
    spec.targets
        .iter()
        .map(|field| {
            (
                field.key.to_owned(),
                settings.strategy(spec.id, field.key, field.default_strategy),
            )
        })
        .collect()
}
