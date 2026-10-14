//! Settings that outlive the window.
//!
//! Everything the user picks is remembered: the consumer and provider, the
//! zone, how hard each destination field tries to match, which source columns
//! the table shows, and the non-secret half of each account. Tokens are not
//! here — those live in [`crate::secrets`].
//!
//! Every field carries `#[serde(default)]`, so a config written by an older or
//! newer build still loads and only the unknown parts fall back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::error::{UserError, UserFacing};
use crate::matching::MatchStrategy;
use crate::time::DateOrder;

const QUALIFIER: &str = "AutoLog";
const SETTINGS_FILE: &str = "settings.toml";

#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigError {
    #[error("no place to keep settings on this system")]
    NoHome,
    #[error("could not write {path}: {reason}")]
    Unwritable { path: String, reason: String },
}

impl UserFacing for ConfigError {
    fn to_user_error(&self) -> UserError {
        match self {
            Self::NoHome => UserError::new(
                "Settings cannot be saved",
                "AutoLog could not work out where to keep its settings on this system.",
            )
            .with_hint("Your choices will be forgotten when AutoLog closes."),
            Self::Unwritable { path, reason } => UserError::new(
                "Settings cannot be saved",
                format!("Could not write to {path}."),
            )
            .with_hint("Check that the folder exists and is writable.")
            .with_detail(reason.clone()),
        }
    }
}

/// Which source columns the table shows, and in what order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnLayout {
    /// Columns from the file, beyond the ones AutoLog always shows.
    #[serde(default)]
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// How hard to push the tracker's API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    /// Requests in flight at once.
    pub concurrency: usize,
    /// Ceiling on requests started per minute; 0 means no ceiling.
    pub per_minute: u32,
}

impl Default for RateLimit {
    fn default() -> Self {
        // Four at a time is a brisk but unremarkable load for a Jira or Kimai
        // instance, and 120/min stays well under Atlassian's published limits.
        Self {
            concurrency: 4,
            per_minute: 120,
        }
    }
}

impl RateLimit {
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            concurrency: self.concurrency.clamp(1, 16),
            per_minute: self.per_minute.min(6000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Consumer selected last time, so it comes back selected.
    pub consumer: String,
    pub provider: String,
    pub timezone: Tz,
    /// `None` leaves each provider's own date convention in charge.
    pub date_order: Option<DateOrder>,
    pub prevent_duplicates: bool,
    pub rate_limit: RateLimit,
    pub theme: Theme,
    pub check_for_updates: bool,
    /// Folder the file picker opens in.
    pub last_directory: Option<PathBuf>,
    /// Non-secret credential fields, by consumer then field.
    pub accounts: BTreeMap<String, BTreeMap<String, String>>,
    /// Match strategy overrides, by consumer then destination field.
    pub matching: BTreeMap<String, BTreeMap<String, MatchStrategy>>,
    /// Table layout, by provider.
    pub columns: BTreeMap<String, ColumnLayout>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            consumer: "jira".to_owned(),
            provider: "grindstone".to_owned(),
            timezone: Tz::UTC,
            date_order: None,
            prevent_duplicates: true,
            rate_limit: RateLimit::default(),
            theme: Theme::default(),
            check_for_updates: true,
            last_directory: None,
            accounts: BTreeMap::new(),
            matching: BTreeMap::new(),
            columns: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Load the saved settings, or the defaults.
    ///
    /// A config file that cannot be read is logged and stepped over rather
    /// than refused: losing preferences is a smaller harm than refusing to
    /// start.
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };

        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };

        match toml::from_str(&text) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(path = %path.display(), "ignoring unreadable settings: {error}");
                Self::default()
            }
        }
    }

    /// Write the settings, replacing the file in one step so an interrupted
    /// save cannot leave a half-written config behind.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = settings_path().ok_or(ConfigError::NoHome)?;
        let directory = path.parent().ok_or(ConfigError::NoHome)?;

        std::fs::create_dir_all(directory).map_err(|error| ConfigError::Unwritable {
            path: directory.display().to_string(),
            reason: error.to_string(),
        })?;

        let text = toml::to_string_pretty(self).map_err(|error| ConfigError::Unwritable {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;

        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text).map_err(|error| ConfigError::Unwritable {
            path: temporary.display().to_string(),
            reason: error.to_string(),
        })?;

        std::fs::rename(&temporary, &path).map_err(|error| ConfigError::Unwritable {
            path: path.display().to_string(),
            reason: error.to_string(),
        })
    }

    /// The saved non-secret fields for one consumer.
    pub fn account(&self, consumer: &str) -> BTreeMap<String, String> {
        self.accounts.get(consumer).cloned().unwrap_or_default()
    }

    pub fn set_account_field(&mut self, consumer: &str, field: &str, value: impl Into<String>) {
        let value = value.into();
        let account = self.accounts.entry(consumer.to_owned()).or_default();
        if value.is_empty() {
            account.remove(field);
        } else {
            account.insert(field.to_owned(), value);
        }
    }

    /// How a destination field should match, falling back to the consumer's
    /// own default when the user has not chosen.
    pub fn strategy(&self, consumer: &str, field: &str, fallback: MatchStrategy) -> MatchStrategy {
        self.matching
            .get(consumer)
            .and_then(|fields| fields.get(field))
            .copied()
            .unwrap_or(fallback)
    }

    pub fn set_strategy(&mut self, consumer: &str, field: &str, strategy: MatchStrategy) {
        self.matching
            .entry(consumer.to_owned())
            .or_default()
            .insert(field.to_owned(), strategy);
    }

    pub fn layout(&self, provider: &str) -> ColumnLayout {
        self.columns.get(provider).cloned().unwrap_or_default()
    }

    pub fn set_layout(&mut self, provider: &str, layout: ColumnLayout) {
        self.columns.insert(provider.to_owned(), layout);
    }

    pub fn remember_directory(&mut self, file: &Path) {
        if let Some(parent) = file.parent() {
            self.last_directory = Some(parent.to_path_buf());
        }
    }
}

fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", QUALIFIER)
}

/// Where the settings file lives, e.g. `~/.config/AutoLog/settings.toml`.
pub fn settings_path() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().join(SETTINGS_FILE))
}

/// Where the rolling log is written.
pub fn log_directory() -> Option<PathBuf> {
    Some(project_dirs()?.data_dir().join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip_through_toml() {
        let mut settings = Settings {
            consumer: "kimai".to_owned(),
            timezone: Tz::Asia__Damascus,
            ..Settings::default()
        };
        settings.set_account_field("kimai", "base_url", "https://kimai.example");
        settings.set_strategy("kimai", "project", MatchStrategy::Exact);
        settings.set_layout(
            "grindstone",
            ColumnLayout {
                extra: vec!["Notes".to_owned()],
            },
        );

        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
        // The token is nowhere near this file.
        assert!(!text.contains("token"));
    }

    #[test]
    fn an_empty_file_loads_as_the_defaults() {
        let settings: Settings = toml::from_str("").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn an_older_config_missing_newer_keys_still_loads() {
        let settings: Settings = toml::from_str(r#"consumer = "kimai""#).unwrap();
        assert_eq!(settings.consumer, "kimai");
        assert_eq!(settings.rate_limit, RateLimit::default());
        assert!(settings.prevent_duplicates);
    }

    #[test]
    fn a_blank_account_field_is_forgotten_rather_than_stored_empty() {
        let mut settings = Settings::default();
        settings.set_account_field("jira", "email", "a@b.c");
        settings.set_account_field("jira", "email", "");
        assert!(settings.account("jira").is_empty());
    }

    #[test]
    fn a_field_with_no_override_keeps_the_consumers_own_default() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.strategy("kimai", "activity", MatchStrategy::Fuzzy),
            MatchStrategy::Fuzzy
        );
        settings.set_strategy("kimai", "activity", MatchStrategy::Manual);
        assert_eq!(
            settings.strategy("kimai", "activity", MatchStrategy::Fuzzy),
            MatchStrategy::Manual
        );
    }

    #[test]
    fn rate_limits_are_held_within_something_sane() {
        let limit = RateLimit {
            concurrency: 0,
            per_minute: 99_999,
        }
        .clamped();
        assert_eq!(limit.concurrency, 1);
        assert_eq!(limit.per_minute, 6000);
    }
}
