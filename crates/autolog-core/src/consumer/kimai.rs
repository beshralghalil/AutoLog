//! Kimai, which files work under a project and an activity.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{Duration, Utc};
use chrono_tz::Tz;
use reqwest::Method;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;

use crate::consumer::{
    Consumer, ConsumerError, ConsumerSpec, CredentialField, Credentials, Hint, Posted,
    TargetFieldSpec,
};
use crate::http::{ApiClient, DEFAULT_TIMEOUT, HttpError, Idempotency};
use crate::matching::MatchStrategy;
use crate::model::{LoggedEntry, WorklogEntry};
use crate::time;

/// Kimai's own datetime shape, local to the signed-in user's zone.
const KIMAI_DATETIME: &str = "%Y-%m-%dT%H:%M:%S";
/// Kimai's maximum page size for collection endpoints.
const PAGE_SIZE: u32 = 100;
/// Stop paging rather than loop forever if the server miscounts its pages.
const MAX_PAGES: u32 = 200;

pub const PROJECT: &str = "project";
pub const ACTIVITY: &str = "activity";

pub static SPEC: ConsumerSpec = ConsumerSpec {
    id: "kimai",
    label: "Kimai",
    about: "Files each entry under a project and an activity.",
    credentials: &[
        CredentialField {
            key: "base_url",
            label: "Kimai URL",
            placeholder: "https://kimai.example.com",
            secret: false,
        },
        CredentialField {
            key: "api_token",
            label: "API token",
            placeholder: "User profile -> API",
            secret: true,
        },
    ],
    targets: &[
        TargetFieldSpec {
            key: PROJECT,
            label: "Project",
            about: "Matched from the work item, which usually names the project in its \
                    key. Narrows which activities the row can use.",
            default_strategy: MatchStrategy::Contains,
            hints: &[Hint::ProjectKey, Hint::RawIssue, Hint::Activity],
            scoped_by: None,
        },
        TargetFieldSpec {
            key: ACTIVITY,
            label: "Activity",
            about: "Matched within the chosen project. Loose by default, since an activity \
                    is free text somebody typed twice.",
            default_strategy: MatchStrategy::Fuzzy,
            hints: &[Hint::IssueKey, Hint::RawIssue, Hint::Activity],
            scoped_by: Some(PROJECT),
        },
    ],
};

#[derive(Debug, Clone)]
struct Project {
    /// What the user picks from; carries the customer where names repeat.
    label: String,
    id: i64,
}

#[derive(Debug, Clone)]
struct Activity {
    name: String,
    id: i64,
    /// `None` for a global activity, which every project may use.
    project: Option<i64>,
}

#[derive(Debug)]
pub struct Kimai {
    api: ApiClient,
    prevent_duplicates: bool,
    /// The signed-in user's zone, which is how Kimai reads a naked datetime.
    timezone: Tz,
    projects: Vec<Project>,
    activities: Vec<Activity>,
    /// What the tracker already has, for the range being posted.
    logged: RwLock<Vec<LoggedEntry>>,
}

impl Kimai {
    pub fn new(credentials: &Credentials, prevent_duplicates: bool) -> Result<Self, ConsumerError> {
        credentials.check(&SPEC)?;

        let mut headers = HeaderMap::new();
        let token = credentials.get("api_token").trim();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                ConsumerError::Http(HttpError::InvalidBaseUrl {
                    input: "API token".to_owned(),
                    reason: "the token contains characters that cannot be sent in a header"
                        .to_owned(),
                })
            })?,
        );

        Ok(Self {
            api: ApiClient::new(
                "Kimai",
                credentials.get("base_url"),
                headers,
                DEFAULT_TIMEOUT,
            )?,
            prevent_duplicates,
            timezone: Tz::UTC,
            projects: Vec::new(),
            activities: Vec::new(),
            logged: RwLock::new(Vec::new()),
        })
    }

    /// Read a paged collection to the end.
    async fn collect<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<Vec<T>, HttpError> {
        let mut items = Vec::new();

        for page in 1..=MAX_PAGES {
            let page = page.to_string();
            let size = PAGE_SIZE.to_string();
            let request = self
                .api
                .request(Method::GET, path)?
                .query(params)
                .query(&[("size", size.as_str()), ("page", page.as_str())]);

            let response = self.api.send(request, Idempotency::Safe).await?;
            let total_pages = response
                .headers()
                .get("X-Total-Pages")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(1);

            let body = response
                .text()
                .await
                .map_err(|error| HttpError::Malformed {
                    service: "Kimai".to_owned(),
                    reason: error.to_string(),
                })?;
            let mut batch: Vec<T> =
                serde_json::from_str(&body).map_err(|error| HttpError::Malformed {
                    service: "Kimai".to_owned(),
                    reason: error.to_string(),
                })?;

            let drained = batch.len();
            items.append(&mut batch);

            if page.parse::<u32>().unwrap_or(u32::MAX) >= total_pages || drained == 0 {
                break;
            }
        }

        Ok(items)
    }

    async fn load_names(&mut self) -> Result<(), ConsumerError> {
        #[derive(Deserialize)]
        struct RawProject {
            id: i64,
            name: String,
            #[serde(default)]
            #[serde(rename = "parentTitle")]
            parent_title: Option<String>,
        }

        #[derive(Deserialize)]
        struct RawActivity {
            id: i64,
            name: String,
            #[serde(default)]
            project: Option<i64>,
        }

        let raw: Vec<RawProject> = self.collect("api/projects", &[("visible", "1")]).await?;

        // Two customers may both have a project called "Support"; without the
        // customer in the label the user cannot tell them apart in a dropdown.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for project in &raw {
            *counts.entry(project.name.as_str()).or_default() += 1;
        }

        self.projects = raw
            .iter()
            .map(|project| Project {
                label: match (&project.parent_title, counts.get(project.name.as_str())) {
                    (Some(customer), Some(&count)) if count > 1 => {
                        format!("{} ({customer})", project.name)
                    }
                    _ => project.name.clone(),
                },
                id: project.id,
            })
            .collect();

        let raw: Vec<RawActivity> = self.collect("api/activities", &[("visible", "1")]).await?;
        self.activities = raw
            .into_iter()
            .map(|activity| Activity {
                name: activity.name,
                id: activity.id,
                project: activity.project,
            })
            .collect();

        Ok(())
    }

    fn project_id(&self, entry: &WorklogEntry) -> Option<i64> {
        let label = entry.target(PROJECT)?;
        self.projects
            .iter()
            .find(|project| project.label == label)
            .map(|project| project.id)
    }

    /// Activities this entry may use: its project's own, plus the global ones.
    fn available_activities(&self, entry: &WorklogEntry) -> Vec<&Activity> {
        let project = self.project_id(entry);
        self.activities
            .iter()
            .filter(|activity| activity.project.is_none() || activity.project == project)
            .collect()
    }

    fn activity_id(&self, entry: &WorklogEntry) -> Option<i64> {
        let name = entry.target(ACTIVITY)?;
        self.available_activities(entry)
            .into_iter()
            .find(|activity| activity.name == name)
            .map(|activity| activity.id)
    }
}

#[async_trait::async_trait]
impl Consumer for Kimai {
    fn spec(&self) -> &'static ConsumerSpec {
        &SPEC
    }

    async fn connect(&mut self) -> Result<(), ConsumerError> {
        #[derive(Deserialize)]
        struct Me {
            #[serde(default)]
            timezone: Option<String>,
        }

        let request = self.api.request(Method::GET, "api/users/me")?;
        let me: Me = self.api.send_json(request, Idempotency::Safe).await?;

        self.timezone = me
            .timezone
            .and_then(|name| name.parse::<Tz>().ok())
            .unwrap_or(Tz::UTC);

        self.load_names().await
    }

    async fn refresh(&mut self) -> Result<(), ConsumerError> {
        self.load_names().await
    }

    fn choices(&self, field: &str, entry: &WorklogEntry) -> Vec<String> {
        let mut names: Vec<String> = match field {
            PROJECT => self
                .projects
                .iter()
                .map(|project| project.label.clone())
                .collect(),
            ACTIVITY => self
                .available_activities(entry)
                .into_iter()
                .map(|activity| activity.name.clone())
                .collect(),
            _ => Vec::new(),
        };

        names.sort_unstable();
        names.dedup();
        names
    }

    fn every_choice(&self, field: &str) -> Vec<String> {
        let mut names: Vec<String> = match field {
            PROJECT => self
                .projects
                .iter()
                .map(|project| project.label.clone())
                .collect(),
            // Every activity, not just one project's: a bulk pick has to suit
            // rows sitting on different projects.
            ACTIVITY => self
                .activities
                .iter()
                .map(|activity| activity.name.clone())
                .collect(),
            _ => Vec::new(),
        };

        names.sort_unstable();
        names.dedup();
        names
    }

    fn validate(&self, entry: &WorklogEntry) -> Result<(), ConsumerError> {
        if self.project_id(entry).is_none() {
            return Err(ConsumerError::Incomplete {
                what: "project".to_owned(),
            });
        }
        if self.activity_id(entry).is_none() {
            return Err(ConsumerError::Incomplete {
                what: "activity for this project".to_owned(),
            });
        }
        Ok(())
    }

    async fn preload(&mut self, entries: &[WorklogEntry]) -> Result<(), ConsumerError> {
        if !self.prevent_duplicates || entries.is_empty() {
            return Ok(());
        }

        let earliest = entries.iter().map(WorklogEntry::start_utc).min();
        let latest = entries
            .iter()
            .map(|entry| entry.end().with_timezone(&Utc))
            .max();
        let (Some(earliest), Some(latest)) = (earliest, latest) else {
            return Ok(());
        };

        // A day either side, so an entry logged against a neighbouring day in
        // another zone is still seen.
        let begin = (earliest - Duration::days(1)).with_timezone(&self.timezone);
        let end = (latest + Duration::days(1)).with_timezone(&self.timezone);

        #[derive(Deserialize)]
        struct RawTimesheet {
            begin: String,
            #[serde(default)]
            duration: Option<i64>,
            #[serde(default)]
            description: Option<String>,
        }

        let raw: Vec<RawTimesheet> = self
            .collect(
                "api/timesheets",
                &[
                    ("begin", &begin.format(KIMAI_DATETIME).to_string()),
                    ("end", &end.format(KIMAI_DATETIME).to_string()),
                ],
            )
            .await?;

        let logged = raw
            .into_iter()
            .filter_map(|sheet| {
                Some(LoggedEntry {
                    started: time::parse_utc(&sheet.begin)?,
                    duration: u32::try_from(sheet.duration.unwrap_or(0)).unwrap_or(0),
                    description: sheet.description.unwrap_or_default(),
                })
            })
            .collect();

        if let Ok(mut cache) = self.logged.write() {
            *cache = logged;
        }
        Ok(())
    }

    fn is_duplicate(&self, entry: &WorklogEntry) -> bool {
        self.prevent_duplicates
            && self
                .logged
                .read()
                .is_ok_and(|logged| logged.iter().any(|other| other.duplicates(entry)))
    }

    async fn submit(&self, entry: &WorklogEntry) -> Result<Posted, ConsumerError> {
        self.validate(entry)?;
        let project = self
            .project_id(entry)
            .ok_or_else(|| ConsumerError::Incomplete {
                what: "project".to_owned(),
            })?;
        let activity = self
            .activity_id(entry)
            .ok_or_else(|| ConsumerError::Incomplete {
                what: "activity".to_owned(),
            })?;

        if self.is_duplicate(entry) {
            return Err(ConsumerError::Duplicate);
        }

        let begin = entry.started.with_timezone(&self.timezone);
        let end = begin + Duration::seconds(i64::from(entry.duration));

        #[derive(Deserialize)]
        struct Created {
            id: i64,
        }

        let request = self
            .api
            .request(Method::POST, "api/timesheets")?
            .json(&serde_json::json!({
                "begin": begin.format(KIMAI_DATETIME).to_string(),
                "end": end.format(KIMAI_DATETIME).to_string(),
                "project": project,
                "activity": activity,
                "description": entry.description,
            }));

        let created: Created = self.api.send_json(request, Idempotency::Unsafe).await?;

        // Remember it, so a second identical row later in the same run is
        // caught without asking the server again.
        if self.prevent_duplicates
            && let Ok(mut logged) = self.logged.write()
        {
            logged.push(LoggedEntry {
                started: entry.start_utc(),
                duration: entry.duration,
                description: entry.description.clone(),
            });
        }

        Ok(Posted {
            reference: Some(created.id.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::Strategies;
    use crate::secrets::Secret;

    fn credentials() -> Credentials {
        let mut credentials = Credentials::default();
        credentials.set("base_url", Secret::new("https://kimai.example.com"));
        credentials.set("api_token", Secret::new("token"));
        credentials
    }

    fn kimai() -> Kimai {
        let mut kimai = Kimai::new(&credentials(), true).unwrap();
        kimai.projects = vec![
            Project {
                label: "ACME".to_owned(),
                id: 1,
            },
            Project {
                label: "WIDGET".to_owned(),
                id: 2,
            },
        ];
        kimai.activities = vec![
            Activity {
                name: "Development".to_owned(),
                id: 10,
                project: Some(1),
            },
            Activity {
                name: "BaseImage".to_owned(),
                id: 11,
                project: Some(2),
            },
            Activity {
                name: "Meetings".to_owned(),
                id: 12,
                project: None,
            },
        ];
        kimai
    }

    use crate::model::fixtures::entry;

    fn strategies() -> Strategies {
        SPEC.targets
            .iter()
            .map(|field| (field.key.to_owned(), field.default_strategy))
            .collect()
    }

    #[test]
    fn missing_credentials_are_caught_before_any_request() {
        let error = Kimai::new(&Credentials::default(), true).unwrap_err();
        assert!(matches!(error, ConsumerError::MissingCredential { .. }));
    }

    #[test]
    fn a_project_is_matched_exactly_and_nothing_looser() {
        let kimai = kimai();
        let mut entries = vec![entry("ACME-6055"), entry("WIDGETS-1")];
        kimai.assign(&mut entries, &strategies());

        assert_eq!(entries[0].target(PROJECT), Some("ACME"));
        // `WIDGETS` reads close to `WIDGET`, but exact matching will not have it.
        assert_eq!(entries[1].target(PROJECT), None);
    }

    #[test]
    fn an_activity_is_matched_only_within_its_project() {
        let kimai = kimai();
        let mut entries = vec![entry("WIDGET-42 BaseImage")];
        kimai.assign(&mut entries, &strategies());

        assert_eq!(entries[0].target(PROJECT), Some("WIDGET"));
        assert_eq!(entries[0].target(ACTIVITY), Some("BaseImage"));
    }

    /// By default a project is found wherever its name appears whole in the
    /// work item, so a row that is not a tidy issue key still lands.
    #[test]
    fn a_work_item_that_is_not_an_issue_key_still_finds_its_project() {
        let kimai = kimai();
        let mut entries = vec![entry("WIDGET-BaseImage")];
        kimai.assign(&mut entries, &strategies());
        assert_eq!(entries[0].target(PROJECT), Some("WIDGET"));
    }

    /// Tightening the field to Exact is still available, and then only a work
    /// item naming the project outright will do.
    #[test]
    fn exact_matching_declines_a_work_item_that_merely_contains_the_name() {
        let kimai = kimai();
        let mut strict = strategies();
        strict.insert(PROJECT.to_owned(), MatchStrategy::Exact);

        let mut entries = vec![entry("WIDGET-BaseImage")];
        kimai.assign(&mut entries, &strict);
        assert_eq!(entries[0].target(PROJECT), None);
    }

    /// A bulk pick has to suit rows on different projects, so it is offered
    /// every activity rather than one project's.
    #[test]
    fn the_bulk_list_is_not_narrowed_to_one_project() {
        let kimai = kimai();
        assert_eq!(
            kimai.every_choice(ACTIVITY),
            ["BaseImage", "Development", "Meetings"]
        );
        assert_eq!(kimai.every_choice(PROJECT), ["WIDGET", "ACME"]);

        let mut entry = entry("ACME-1");
        entry.set_target(PROJECT, "ACME");
        assert_eq!(kimai.choices(ACTIVITY, &entry), ["Development", "Meetings"]);
    }

    #[test]
    fn global_activities_are_offered_to_every_project() {
        let kimai = kimai();
        let mut entry = entry("ACME-1");
        entry.set_target(PROJECT, "ACME");
        let choices = kimai.choices(ACTIVITY, &entry);
        assert_eq!(choices, ["Development", "Meetings"]);
    }

    #[test]
    fn changing_the_project_re_matches_the_activity_under_it() {
        let kimai = kimai();
        let mut entry = entry("WIDGET-42 BaseImage");
        kimai.assign(std::slice::from_mut(&mut entry), &strategies());
        assert_eq!(entry.target(ACTIVITY), Some("BaseImage"));

        kimai.retarget(&mut entry, PROJECT, "ACME", &strategies());
        // BaseImage does not belong to ACME, so it is dropped rather than kept.
        assert_eq!(entry.target(ACTIVITY), None);
    }

    #[test]
    fn an_entry_with_no_destination_is_refused_rather_than_guessed_at() {
        let kimai = kimai();
        let entry = entry("Meetings");
        assert!(matches!(
            kimai.validate(&entry),
            Err(ConsumerError::Incomplete { .. })
        ));
    }

    #[test]
    fn a_complete_entry_validates() {
        let kimai = kimai();
        let mut entry = entry("ACME-6055");
        kimai.assign(std::slice::from_mut(&mut entry), &strategies());
        entry.set_target(ACTIVITY, "Development");
        assert!(kimai.validate(&entry).is_ok());
    }

    #[test]
    fn kimai_datetimes_with_a_colon_less_offset_still_read() {
        let parsed = time::parse_utc("2025-05-04T08:00:00+0200").unwrap();
        assert_eq!(parsed.to_string(), "2025-05-04 06:00:00 UTC");
    }

    #[test]
    fn strategy_overrides_widen_the_match() {
        let kimai = kimai();
        let mut loose: Strategies = strategies();
        loose.insert(PROJECT.to_owned(), MatchStrategy::Contains);

        let mut entries = vec![entry("ACME-6055")];
        entries[0].project_key = None; // only the raw work item is left to go on
        kimai.assign(&mut entries, &loose);
        assert_eq!(entries[0].target(PROJECT), Some("ACME"));
    }
}
