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

/// Issues fetched for the picker. Enough to cover what anybody logs against
/// in a stretch, without making signing in slow.
const ISSUE_LIMIT: u32 = 500;

pub const PROJECT: &str = "project";
pub const ISSUE: &str = "issue";

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
    targets: &[
        TargetFieldSpec {
            key: PROJECT,
            label: "Project",
            about: "Matched against the projects Jira knows, from the work item's key.",
            default_strategy: MatchStrategy::Contains,
            hints: &[Hint::ProjectKey, Hint::RawIssue, Hint::Activity],
            scoped_by: None,
        },
        TargetFieldSpec {
            key: ISSUE,
            label: "Issue",
            about: "The issue the work is logged against, matched within the chosen project. \
                    Issues you are assigned to or have logged against are fetched on connect; \
                    any other key can be typed into the picker.",
            default_strategy: MatchStrategy::Contains,
            hints: &[Hint::IssueKey, Hint::RawIssue, Hint::Activity],
            scoped_by: Some(PROJECT),
        },
    ],
};

#[derive(Debug)]
pub struct Jira {
    api: ApiClient,
    prevent_duplicates: bool,
    /// Project keys the signed-in user can see. Empty until connected, and
    /// treated as "unknown" rather than "none" while it is.
    projects: Vec<String>,
    /// Issues offered in the picker, newest first.
    issues: Vec<Issue>,
    /// Worklogs already on each issue, by key.
    logged: RwLock<HashMap<String, Vec<LoggedEntry>>>,
}

/// One issue, as the picker shows it.
#[derive(Debug, Clone)]
struct Issue {
    /// `ABC-123  Fix the thing`. The summary is part of the label on purpose:
    /// it is what the user reads, and it gives the matcher something to work
    /// with beyond a bare key. The key is read back off the front with
    /// [`Jira::key_of`].
    label: String,
    project: String,
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
            issues: Vec::new(),
            logged: RwLock::new(HashMap::new()),
        })
    }

    /// Fetch the issues this user plausibly logs against.
    ///
    /// Listing every issue on the instance is neither possible nor useful, so
    /// this asks for the ones they are assigned to or have already logged
    /// against, most recently touched first. Anything outside that can still
    /// be typed into the picker by hand.
    async fn load_issues(&mut self) -> Result<(), ConsumerError> {
        const JQL: &str = "assignee = currentUser() OR worklogAuthor = currentUser() \
                           ORDER BY updated DESC";

        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            issues: Vec<RawIssue>,
        }

        #[derive(Deserialize)]
        struct RawIssue {
            key: String,
            #[serde(default)]
            fields: Fields,
        }

        #[derive(Deserialize, Default)]
        struct Fields {
            #[serde(default)]
            summary: Option<String>,
        }

        let page: Page = match self.search(JQL).await {
            Ok(page) => page,
            Err(error) => {
                // A picker with nothing in it is a smaller problem than not
                // being able to post at all, so this is not fatal.
                tracing::warn!("could not fetch the issue list: {error}");
                self.issues.clear();
                return Ok(());
            }
        };

        self.issues = page
            .issues
            .into_iter()
            .map(|issue| {
                let project = issue.key.split('-').next().unwrap_or(&issue.key).to_owned();
                let label = match issue.fields.summary {
                    Some(summary) if !summary.trim().is_empty() => {
                        format!("{}  {}", issue.key, summary.trim())
                    }
                    _ => issue.key.clone(),
                };
                Issue { label, project }
            })
            .collect();

        Ok(())
    }

    /// Run a JQL search, over whichever search endpoint this instance has.
    ///
    /// Jira Cloud replaced `search` with `search/jql`; Server and Data Center
    /// only have the old one. Trying the new path first and falling back on a
    /// 404 covers both without asking the user which they are on.
    async fn search<T: serde::de::DeserializeOwned>(&self, jql: &str) -> Result<T, HttpError> {
        let limit = ISSUE_LIMIT.to_string();
        let paths = ["rest/api/2/search/jql", "rest/api/2/search"];
        let mut last = None;

        for path in paths {
            let request = self.api.request(Method::GET, path)?.query(&[
                ("jql", jql),
                ("maxResults", limit.as_str()),
                ("fields", "summary"),
            ]);

            match self.api.send_json(request, Idempotency::Safe).await {
                Ok(found) => return Ok(found),
                Err(error @ HttpError::NotFound { .. }) => last = Some(error),
                Err(error) => return Err(error),
            }
        }

        Err(last.unwrap_or_else(|| HttpError::NotFound {
            service: "Jira".to_owned(),
            what: "a usable search endpoint".to_owned(),
        }))
    }

    /// The key out of a picker value like `ABC-123  Fix the thing`, or out of
    /// a key the user typed themselves.
    fn key_of(value: &str) -> &str {
        value.split_whitespace().next().unwrap_or(value).trim()
    }

    /// Where this entry is going, as a bare issue key.
    fn resolved_key(entry: &WorklogEntry) -> Option<&str> {
        entry
            .target(ISSUE)
            .map(Self::key_of)
            .or(entry.issue_key.as_deref())
            .filter(|key| !key.is_empty())
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
        self.refresh().await
    }

    async fn refresh(&mut self) -> Result<(), ConsumerError> {
        self.load_projects().await?;
        self.load_issues().await
    }

    fn choices(&self, field: &str, entry: &WorklogEntry) -> Vec<String> {
        match field {
            PROJECT => self.projects.clone(),
            // Narrowed to the project the row is going to, the way a Kimai
            // activity is narrowed to its project.
            ISSUE => {
                let project = entry.target(PROJECT);
                self.issues
                    .iter()
                    .filter(|issue| project.is_none_or(|project| issue.project == project))
                    .map(|issue| issue.label.clone())
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn every_choice(&self, field: &str) -> Vec<String> {
        match field {
            PROJECT => self.projects.clone(),
            ISSUE => self
                .issues
                .iter()
                .map(|issue| issue.label.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Picking a project is picking where the work goes, so the issue follows
    /// it rather than being left contradicting the column beside it.
    fn retarget(
        &self,
        entry: &mut WorklogEntry,
        field: &str,
        value: &str,
        strategies: &Strategies,
    ) {
        entry.set_target(field, value);

        if field == PROJECT && !value.is_empty() {
            entry.project_key = Some(value.to_owned());

            // The issue belonged to the old project. Carry its number over
            // where there is one, then look for a better match under the new
            // project.
            if let Some(number) = Self::resolved_key(entry).and_then(issue_number) {
                entry.set_target(ISSUE, format!("{value}-{number}"));
            } else {
                entry.set_target(ISSUE, "");
            }

            if entry.target(ISSUE).is_none()
                && let Some(spec) = SPEC.target(ISSUE)
            {
                self.match_field(entry, spec, strategies);
            }
        }

        entry.reset();
    }

    fn validate(&self, entry: &WorklogEntry) -> Result<(), ConsumerError> {
        let Some(key) = Self::resolved_key(entry) else {
            return Err(ConsumerError::Incomplete {
                what: "issue".to_owned(),
            });
        };

        if !is_wellformed_key(key) {
            return Err(ConsumerError::Incomplete {
                what: format!("usable issue key - `{key}` is not one, it should read like ABC-123"),
            });
        }

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

        // Filtered here too, not only in `validate`: these keys go straight
        // into a request path, and this runs before anything is validated.
        let keys: BTreeSet<String> = entries
            .iter()
            .filter_map(Self::resolved_key)
            .filter(|key| is_wellformed_key(key))
            .map(ToOwned::to_owned)
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
        let Some(key) = Self::resolved_key(entry) else {
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
        let key = Self::resolved_key(entry)
            .ok_or_else(|| ConsumerError::Incomplete {
                what: "issue".to_owned(),
            })?
            .to_owned();

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

/// Whether `key` is shaped like an issue key and nothing else.
///
/// The key is interpolated into a request path, and one carrying `/` or `..`
/// would resolve to a different endpoint than the one intended. Checking the
/// number alone is not enough: it only reads the half after the last dash, so
/// `../../myself-1` passes. Restricting the whole string to the characters a
/// key may contain closes that without anything needing to be escaped.
fn is_wellformed_key(key: &str) -> bool {
    let Some((project, number)) = key.rsplit_once('-') else {
        return false;
    };

    project.starts_with(|ch: char| ch.is_ascii_alphabetic())
        && project
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
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

    /// A Jira that has also fetched an issue list.
    fn jira_with_issues(projects: &[&str], issues: &[&str]) -> Jira {
        let mut jira = jira(projects);
        jira.issues = issues
            .iter()
            .map(|label| Issue {
                label: (*label).to_owned(),
                project: Jira::key_of(label)
                    .split('-')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect();
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
    fn an_entry_with_no_issue_is_refused() {
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
    fn the_issue_is_matched_within_the_chosen_project() {
        let jira = jira_with_issues(
            &["ABC", "ACME"],
            &["ACME-6055  Rename upload-queue", "ABC-1  Something else"],
        );

        let mut entries = vec![entry("ACME-6055")];
        jira.assign(&mut entries, &strategies());

        assert_eq!(entries[0].target(PROJECT), Some("ACME"));
        assert_eq!(
            entries[0].target(ISSUE),
            Some("ACME-6055  Rename upload-queue")
        );
        // The summary rides along in the label, but only the key is posted.
        assert_eq!(Jira::resolved_key(&entries[0]), Some("ACME-6055"));
        assert!(jira.validate(&entries[0]).is_ok());
    }

    #[test]
    fn the_issue_picker_is_narrowed_to_the_rows_project() {
        let jira = jira_with_issues(
            &["ABC", "ACME"],
            &["ACME-6055  Rename upload-queue", "ABC-1  Something else"],
        );

        let mut entry = entry("ACME-6055");
        entry.set_target(PROJECT, "ACME");
        assert_eq!(
            jira.choices(ISSUE, &entry),
            ["ACME-6055  Rename upload-queue"]
        );

        // A bulk pick spans projects, so it is offered everything.
        assert_eq!(jira.every_choice(ISSUE).len(), 2);
    }

    /// A key typed by hand, for an issue the search did not return.
    #[test]
    fn an_issue_that_is_not_in_the_list_can_still_be_posted() {
        let jira = jira_with_issues(&["ABC"], &[]);
        let mut entry = entry("Meetings");
        jira.retarget(&mut entry, ISSUE, "ABC-99", &strategies());

        assert_eq!(Jira::resolved_key(&entry), Some("ABC-99"));
        assert!(jira.validate(&entry).is_ok());
    }

    #[test]
    fn a_target_that_is_not_a_key_is_refused() {
        let jira = jira_with_issues(&["ABC"], &[]);
        let mut entry = entry("Meetings");
        jira.retarget(&mut entry, ISSUE, "not a key", &strategies());

        let error = jira.validate(&entry).unwrap_err();
        assert!(error.to_string().contains("ABC-123"), "{error}");
    }

    #[test]
    fn choosing_a_project_carries_the_issue_key_with_it() {
        let jira = jira_with_issues(&["ABC", "ACME"], &[]);
        let mut entry = entry("ABC-6055");
        jira.retarget(&mut entry, ISSUE, "ABC-6055", &strategies());
        jira.retarget(&mut entry, PROJECT, "ACME", &strategies());

        assert_eq!(Jira::resolved_key(&entry), Some("ACME-6055"));
        assert_eq!(entry.project_key.as_deref(), Some("ACME"));
        assert!(jira.validate(&entry).is_ok());
    }

    #[test]
    fn the_key_is_read_off_the_front_of_a_picker_label() {
        assert_eq!(Jira::key_of("ABC-123  Fix the thing"), "ABC-123");
        assert_eq!(Jira::key_of("ABC-123"), "ABC-123");
    }

    #[test]
    fn the_number_half_of_a_key_is_read_only_when_it_is_a_number() {
        assert_eq!(issue_number("ABC-6055"), Some("6055"));
        assert_eq!(issue_number("ABC"), None);
        assert_eq!(issue_number("ABC-X"), None);
    }

    /// The key is interpolated into a request path, so anything that could
    /// walk out of it has to be refused before it gets there.
    #[test]
    fn a_key_that_could_escape_the_url_path_is_refused() {
        assert!(is_wellformed_key("ABC-123"));
        assert!(is_wellformed_key("ABC1_X-9"));

        for crafted in [
            "../../myself-1",
            "../../../admin-9",
            "ABC/../other-1",
            "ABC%2f..-1",
            "ABC -1",
            "-1",
            "ABC",
            "1ABC-2",
        ] {
            assert!(!is_wellformed_key(crafted), "{crafted} should be refused");
        }
    }

    #[test]
    fn validate_refuses_a_crafted_key_even_though_it_ends_in_a_number() {
        let jira = jira_with_issues(&[], &[]);
        let mut entry = entry("Meetings");
        jira.retarget(&mut entry, ISSUE, "../../myself-1", &strategies());

        // `issue_number` alone would have accepted this: it reads only the
        // half after the last dash.
        assert_eq!(issue_number("../../myself-1"), Some("1"));
        assert!(jira.validate(&entry).is_err());
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
