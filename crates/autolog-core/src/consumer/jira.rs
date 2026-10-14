//! Jira, which files work against an issue key.
//!
//! Jira needs no destination picked — the key is already in the work item —
//! but it is worth knowing which projects exist before posting fifty entries
//! against a key whose prefix was mistyped. So the projects are fetched on
//! connect and matched like any other destination, and choosing one rewrites
//! the issue key's prefix.

use std::collections::{BTreeSet, HashMap};
use std::sync::RwLock;

use base64::Engine as _;
use futures::StreamExt as _;
use reqwest::Method;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;

use crate::consumer::{
    Consumer, ConsumerError, ConsumerSpec, CredentialField, Credentials, Hint, Posted, Strategies,
    TargetFieldSpec,
};
use crate::http::{ApiClient, DEFAULT_TIMEOUT, HttpError, Idempotency};
use crate::matching::MatchStrategy;
use crate::model::{LoggedEntry, WorklogEntry};
use crate::time;

/// The shape Jira insists a worklog's start is written in.
const JIRA_DATETIME: &str = "%Y-%m-%dT%H:%M:%S%.3f%z";
/// Issue worklogs read back at once.
const PAGE_SIZE: u32 = 100;
const MAX_PAGES: u32 = 100;
/// Issues whose worklogs are read back at the same time.
const PRELOAD_CONCURRENCY: usize = 6;

pub const PROJECT: &str = "project";

pub static SPEC: ConsumerSpec = ConsumerSpec {
    id: "jira",
    label: "Jira",
    about: "Files each entry against its issue key.",
    credentials: &[
        CredentialField {
            key: "base_url",
            label: "Jira URL",
            placeholder: "https://your-company.atlassian.net",
            secret: false,
        },
        CredentialField {
            key: "email",
            label: "Email",
            placeholder: "you@example.com",
            secret: false,
        },
        CredentialField {
            key: "api_key",
            label: "API token",
            placeholder: "id.atlassian.com -> Security -> API tokens",
            secret: true,
        },
    ],
    targets: &[TargetFieldSpec {
        key: PROJECT,
        label: "Project",
        about: "Read from the issue key and checked against the projects Jira knows. \
                Choosing a different one rewrites the key's prefix for that entry.",
        default_strategy: MatchStrategy::Exact,
        hints: &[Hint::ProjectKey, Hint::RawIssue, Hint::Activity],
        scoped_by: None,
    }],
};

#[derive(Debug)]
pub struct Jira {
    api: ApiClient,
    prevent_duplicates: bool,
    /// Project keys the signed-in user can see. Empty until connected, and
    /// treated as "unknown" rather than "none" while it is.
    projects: Vec<String>,
    /// Worklogs already on each issue, by key.
    logged: RwLock<HashMap<String, Vec<LoggedEntry>>>,
}

impl Jira {
    pub fn new(credentials: &Credentials, prevent_duplicates: bool) -> Result<Self, ConsumerError> {
        credentials.check(&SPEC)?;

        let token = base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            credentials.get("email").trim(),
            credentials.get("api_key").trim()
        ));

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {token}")).map_err(|_| {
                ConsumerError::Http(HttpError::InvalidBaseUrl {
                    input: "credentials".to_owned(),
                    reason:
                        "the email or token contains characters that cannot be sent in a header"
                            .to_owned(),
                })
            })?,
        );

        Ok(Self {
            api: ApiClient::new(
                "Jira",
                credentials.get("base_url"),
                headers,
                DEFAULT_TIMEOUT,
            )?,
            prevent_duplicates,
            projects: Vec::new(),
            logged: RwLock::new(HashMap::new()),
        })
    }

    async fn load_projects(&mut self) -> Result<(), ConsumerError> {
        #[derive(Deserialize)]
        struct Project {
            key: String,
        }

        let request = self.api.request(Method::GET, "rest/api/2/project")?;
        let projects: Vec<Project> = self.api.send_json(request, Idempotency::Safe).await?;

        self.projects = projects.into_iter().map(|project| project.key).collect();
        self.projects.sort_unstable();
        Ok(())
    }

    /// Everything already logged against one issue.
    async fn issue_worklogs(&self, key: &str) -> Result<Vec<LoggedEntry>, HttpError> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            worklogs: Vec<RawWorklog>,
            #[serde(default)]
            total: u32,
        }

        #[derive(Deserialize)]
        struct RawWorklog {
            #[serde(default)]
            started: Option<String>,
            #[serde(default, rename = "timeSpentSeconds")]
            time_spent_seconds: Option<u32>,
            #[serde(default)]
            comment: Option<serde_json::Value>,
        }

        let mut logged = Vec::new();
        let mut start_at = 0_u32;

        for _ in 0..MAX_PAGES {
            let request = self
                .api
                .request(Method::GET, &format!("rest/api/2/issue/{key}/worklog"))?
                .query(&[
                    ("startAt", start_at.to_string()),
                    ("maxResults", PAGE_SIZE.to_string()),
                ]);

            let page: Page = self.api.send_json(request, Idempotency::Safe).await?;
            let read = u32::try_from(page.worklogs.len()).unwrap_or(0);

            logged.extend(page.worklogs.into_iter().filter_map(|worklog| {
                Some(LoggedEntry {
                    started: time::parse_utc(worklog.started.as_deref()?)?,
                    duration: worklog.time_spent_seconds.unwrap_or(0),
                    description: comment_text(worklog.comment.as_ref()),
                })
            }));

            start_at += read;
            if read == 0 || start_at >= page.total {
                break;
            }
        }

        Ok(logged)
    }

    /// Whether `key`'s project is one Jira told us about.
    ///
    /// An empty project list means the fetch has not happened or failed, and
    /// an unverifiable key is better posted than refused.
    fn project_is_known(&self, key: &str) -> bool {
        if self.projects.is_empty() {
            return true;
        }
        key.split('-')
            .next()
            .is_some_and(|prefix| self.projects.iter().any(|project| project == prefix))
    }
}

#[async_trait::async_trait]
impl Consumer for Jira {
    fn spec(&self) -> &'static ConsumerSpec {
        &SPEC
    }

    async fn connect(&mut self) -> Result<(), ConsumerError> {
        // Cheap, and the first thing that fails when a token is wrong.
        let request = self.api.request(Method::GET, "rest/api/2/myself")?;
        self.api.send(request, Idempotency::Safe).await?;
        self.load_projects().await
    }

    async fn refresh(&mut self) -> Result<(), ConsumerError> {
        self.load_projects().await
    }

    fn choices(&self, field: &str, _entry: &WorklogEntry) -> Vec<String> {
        if field == PROJECT {
            self.projects.clone()
        } else {
            Vec::new()
        }
    }

    /// Picking a project is picking where the work goes, so the issue key
    /// follows it rather than being left contradicting the column beside it.
    fn retarget(
        &self,
        entry: &mut WorklogEntry,
        field: &str,
        value: &str,
        _strategies: &Strategies,
    ) {
        entry.set_target(field, value);

        if field == PROJECT && !value.is_empty() {
            if let Some(number) = entry.issue_key.as_deref().and_then(issue_number) {
                entry.issue_key = Some(format!("{value}-{number}"));
            }
            entry.project_key = Some(value.to_owned());
        }

        entry.reset();
    }

    fn validate(&self, entry: &WorklogEntry) -> Result<(), ConsumerError> {
        let Some(key) = entry
            .issue_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
        else {
            return Err(ConsumerError::Incomplete {
                what: "issue key".to_owned(),
            });
        };

        if !self.project_is_known(key) {
            let project = key.split('-').next().unwrap_or(key);
            return Err(ConsumerError::Incomplete {
                what: format!("issue key AutoLog recognises - Jira has no project `{project}`"),
            });
        }

        Ok(())
    }

    async fn preload(&mut self, entries: &[WorklogEntry]) -> Result<(), ConsumerError> {
        if !self.prevent_duplicates {
            return Ok(());
        }

        let keys: BTreeSet<String> = entries
            .iter()
            .filter_map(|entry| entry.issue_key.clone())
            .collect();

        let fetched: Vec<(String, Vec<LoggedEntry>)> = futures::stream::iter(keys)
            .map(|key| async {
                // An issue whose worklogs cannot be read is not a reason to
                // stop: it only means duplicates there go unnoticed.
                let logged = self.issue_worklogs(&key).await.unwrap_or_else(|error| {
                    tracing::warn!(issue = %key, "could not read existing worklogs: {error}");
                    Vec::new()
                });
                (key, logged)
            })
            .buffer_unordered(PRELOAD_CONCURRENCY)
            .collect()
            .await;

        if let Ok(mut cache) = self.logged.write() {
            cache.extend(fetched);
        }
        Ok(())
    }

    fn is_duplicate(&self, entry: &WorklogEntry) -> bool {
        let Some(key) = entry.issue_key.as_deref() else {
            return false;
        };

        self.prevent_duplicates
            && self.logged.read().is_ok_and(|cache| {
                cache
                    .get(key)
                    .is_some_and(|logged| logged.iter().any(|other| other.duplicates(entry)))
            })
    }

    async fn submit(&self, entry: &WorklogEntry) -> Result<Posted, ConsumerError> {
        self.validate(entry)?;
        let key = entry
            .issue_key
            .clone()
            .ok_or_else(|| ConsumerError::Incomplete {
                what: "issue key".to_owned(),
            })?;

        if self.is_duplicate(entry) {
            return Err(ConsumerError::Duplicate);
        }

        #[derive(Deserialize)]
        struct Created {
            #[serde(default)]
            id: Option<String>,
        }

        let request = self
            .api
            .request(Method::POST, &format!("rest/api/2/issue/{key}/worklog"))?
            .json(&serde_json::json!({
                "started": entry.started.format(JIRA_DATETIME).to_string(),
                "timeSpentSeconds": entry.duration,
                "comment": entry.description,
            }));

        let created: Created = self.api.send_json(request, Idempotency::Unsafe).await?;

        if self.prevent_duplicates
            && let Ok(mut cache) = self.logged.write()
        {
            cache.entry(key).or_default().push(LoggedEntry {
                started: entry.start_utc(),
                duration: entry.duration,
                description: entry.description.clone(),
            });
        }

        Ok(Posted {
            reference: created.id,
        })
    }
}

/// The number half of `ABC-123`.
fn issue_number(key: &str) -> Option<&str> {
    key.rsplit_once('-')
        .map(|(_, number)| number)
        .filter(|number| number.chars().all(|ch| ch.is_ascii_digit()) && !number.is_empty())
}

/// A worklog comment, which is plain text on the v2 API but may arrive as a
/// rich-text document from an instance that answers in the newer shape.
fn comment_text(comment: Option<&serde_json::Value>) -> String {
    match comment {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(document) => flatten_rich_text(document),
    }
}

/// Pull every `text` node out of an Atlassian document, in order.
fn flatten_rich_text(node: &serde_json::Value) -> String {
    match node {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(flatten_rich_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::Object(fields) => {
            if let Some(serde_json::Value::String(text)) = fields.get("text") {
                return text.clone();
            }
            fields
                .get("content")
                .map(flatten_rich_text)
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::entry;
    use crate::secrets::Secret;

    fn credentials() -> Credentials {
        let mut credentials = Credentials::default();
        credentials.set("base_url", Secret::new("https://company.atlassian.net"));
        credentials.set("email", Secret::new("me@company.com"));
        credentials.set("api_key", Secret::new("token"));
        credentials
    }

    fn jira(projects: &[&str]) -> Jira {
        let mut jira = Jira::new(&credentials(), true).unwrap();
        jira.projects = projects.iter().map(|key| (*key).to_owned()).collect();
        jira
    }

    fn strategies() -> Strategies {
        SPEC.targets
            .iter()
            .map(|field| (field.key.to_owned(), field.default_strategy))
            .collect()
    }

    #[test]
    fn every_credential_is_required_before_a_request_is_made() {
        let mut partial = Credentials::default();
        partial.set("base_url", Secret::new("https://company.atlassian.net"));
        assert!(matches!(
            Jira::new(&partial, true),
            Err(ConsumerError::MissingCredential { .. })
        ));
    }

    #[test]
    fn an_entry_with_no_issue_key_is_refused() {
        let jira = jira(&["ABC"]);
        assert!(matches!(
            jira.validate(&entry("Meetings")),
            Err(ConsumerError::Incomplete { .. })
        ));
    }

    #[test]
    fn a_key_whose_project_jira_does_not_have_is_caught_before_posting() {
        let jira = jira(&["ABC", "ACME"]);
        assert!(jira.validate(&entry("ACME-6055")).is_ok());

        let error = jira.validate(&entry("TYPO-1")).unwrap_err();
        assert!(error.to_string().contains("TYPO"));
    }

    #[test]
    fn an_unfetched_project_list_does_not_block_posting() {
        let jira = jira(&[]);
        assert!(jira.validate(&entry("ANYTHING-1")).is_ok());
    }

    #[test]
    fn the_project_is_matched_from_the_issue_key() {
        let jira = jira(&["ABC", "ACME"]);
        let mut entries = vec![entry("ACME-6055")];
        jira.assign(&mut entries, &strategies());
        assert_eq!(entries[0].target(PROJECT), Some("ACME"));
    }

    #[test]
    fn choosing_a_project_carries_the_issue_key_with_it() {
        let jira = jira(&["ABC", "ACME"]);
        let mut entry = entry("ABC-6055");
        jira.retarget(&mut entry, PROJECT, "ACME", &strategies());

        assert_eq!(entry.issue_key.as_deref(), Some("ACME-6055"));
        assert_eq!(entry.project_key.as_deref(), Some("ACME"));
        assert!(jira.validate(&entry).is_ok());
    }

    #[test]
    fn the_number_half_of_a_key_is_read_only_when_it_is_a_number() {
        assert_eq!(issue_number("ABC-6055"), Some("6055"));
        assert_eq!(issue_number("ABC"), None);
        assert_eq!(issue_number("ABC-X"), None);
    }

    #[test]
    fn a_plain_text_comment_comes_through_unchanged() {
        let comment = serde_json::json!("fixed the bug");
        assert_eq!(comment_text(Some(&comment)), "fixed the bug");
        assert_eq!(comment_text(None), "");
    }

    #[test]
    fn a_rich_text_comment_is_flattened_to_its_words() {
        let comment = serde_json::json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [
                    {"type": "text", "text": "fixed"},
                    {"type": "text", "text": "the bug"}
                ]
            }]
        });
        assert_eq!(comment_text(Some(&comment)), "fixed the bug");
    }

    #[test]
    fn jira_timestamps_read_back_for_duplicate_checking() {
        assert_eq!(
            time::parse_utc("2025-05-04T08:00:00.000+0000")
                .unwrap()
                .to_string(),
            "2025-05-04 08:00:00 UTC"
        );
    }

    #[test]
    fn a_worklog_start_is_written_the_way_jira_wants_it() {
        let entry = crate::model::fixtures::at("ABC-1", "2025-05-04 08:00", 3600);
        assert_eq!(
            entry.started.format(JIRA_DATETIME).to_string(),
            "2025-05-04T08:00:00.000+0000"
        );
    }
}
