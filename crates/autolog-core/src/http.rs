//! One shared way of talking to a tracker's REST API.
//!
//! Three things matter here and none of them are the happy path: a request
//! must not hang forever, a retry must never post the same worklog twice, and
//! whatever comes back must turn into a sentence the user can act on.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use url::Url;

use crate::error::{UserError, UserFacing};

/// How long to wait for the connection itself before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for a whole request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Attempts past the first, for requests that may be retried at all.
const MAX_RETRIES: u32 = 3;
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
/// Never sleep longer than this between attempts, whatever the server asks.
const MAX_BACKOFF: Duration = Duration::from_secs(20);

/// Whether sending a request twice is harmless.
///
/// A `GET` can always be tried again. A `POST` that creates a worklog cannot:
/// a reply lost on the way back is indistinguishable from one never sent, and
/// a second attempt would log the work twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idempotency {
    Safe,
    Unsafe,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum HttpError {
    #[error("`{input}` is not a usable address: {reason}")]
    InvalidBaseUrl { input: String, reason: String },
    #[error("could not reach {host}: {reason}")]
    Unreachable { host: String, reason: String },
    #[error("{host} did not answer within {seconds}s")]
    Timeout { host: String, seconds: u64 },
    #[error("could not establish a secure connection to {host}: {reason}")]
    Tls { host: String, reason: String },
    #[error("{service} rejected the credentials")]
    Unauthorized { service: String },
    #[error("{service} refused the request: {message}")]
    Forbidden { service: String, message: String },
    #[error("{service} has no {what}")]
    NotFound { service: String, what: String },
    #[error("{service} is rate limiting this client")]
    RateLimited {
        service: String,
        retry_after: Option<u64>,
    },
    #[error("{service} refused the request ({status}): {message}")]
    Rejected {
        service: String,
        status: u16,
        message: String,
    },
    #[error("{service} failed ({status}): {message}")]
    Server {
        service: String,
        status: u16,
        message: String,
    },
    #[error("could not make sense of {service}'s reply: {reason}")]
    Malformed { service: String, reason: String },
}

impl UserFacing for HttpError {
    fn to_user_error(&self) -> UserError {
        match self {
            Self::InvalidBaseUrl { input, reason } => {
                UserError::new("Check the address", format!("`{input}` is not a valid URL."))
                    .with_hint("It should look like https://your-company.atlassian.net.")
                    .with_detail(reason.clone())
            }
            Self::Unreachable { host, reason } => {
                UserError::new("Cannot reach the server", format!("{host} did not answer."))
                    .with_hint("Check the address, your internet connection, and whether you need to be on the VPN.")
                    .with_detail(reason.clone())
            }
            Self::Timeout { host, seconds } => UserError::new(
                "The server took too long",
                format!("{host} did not reply within {seconds} seconds."),
            )
            .with_hint("It may be busy. Try again in a moment."),
            Self::Tls { host, reason } => UserError::new(
                "Insecure connection",
                format!("The secure connection to {host} could not be set up."),
            )
            .with_hint("This often means a company proxy is inspecting traffic, or the server's certificate has expired.")
            .with_detail(reason.clone()),
            Self::Unauthorized { service } => UserError::new(
                "Sign-in failed",
                format!("{service} did not accept your credentials."),
            )
            .with_hint("Check the token, and that it has not expired or been revoked."),
            Self::Forbidden { service, message } => UserError::new(
                "Not allowed",
                format!("{service} accepted your sign-in but refused the action."),
            )
            .with_hint("Your account may lack permission on this project.")
            .with_detail(message.clone()),
            Self::NotFound { service, what } => {
                UserError::new("Not found", format!("{service} has no {what}."))
                    .with_hint("Check the issue key or project; it may have been moved or deleted.")
            }
            Self::RateLimited { service, retry_after } => UserError::new(
                "Too many requests",
                format!("{service} is asking this client to slow down."),
            )
            .with_hint(retry_after.map_or_else(
                || "Lower the rate limit in Settings and try again.".to_owned(),
                |seconds| format!("It will accept requests again in about {seconds} seconds."),
            )),
            Self::Rejected { service, status, message } => {
                UserError::new("Rejected", format!("{service} refused this entry: {message}"))
                    .with_detail(format!("HTTP {status}"))
            }
            Self::Server { service, status, message } => UserError::new(
                "The server had a problem",
                format!("{service} returned an error and did not say why."),
            )
            .with_hint("This is on their side. Try again shortly.")
            .with_detail(format!("HTTP {status}: {message}")),
            Self::Malformed { service, reason } => UserError::new(
                "Unexpected reply",
                format!("{service} answered with something AutoLog could not read."),
            )
            .with_hint("Check that the address points at the API and not at a sign-in page.")
            .with_detail(reason.clone()),
        }
    }
}

/// A client bound to one tracker, carrying its credentials and its name.
#[derive(Debug, Clone)]
pub struct ApiClient {
    inner: reqwest::Client,
    base: Url,
    /// What to call this server when something goes wrong, e.g. `"Kimai"`.
    service: String,
    timeout: Duration,
}

impl ApiClient {
    /// Build a client for `base_url`, with `headers` sent on every request.
    ///
    /// Authorization belongs in `headers` and is marked sensitive, so it stays
    /// out of any log line reqwest or tracing might write.
    pub fn new(
        service: impl Into<String>,
        base_url: &str,
        mut headers: HeaderMap,
        timeout: Duration,
    ) -> Result<Self, HttpError> {
        let service = service.into();
        let base = parse_base_url(base_url)?;

        for value in headers.values_mut() {
            value.set_sensitive(true);
        }
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );

        let inner = reqwest::Client::builder()
            .user_agent(concat!("AutoLog/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .build()
            .map_err(|error| HttpError::InvalidBaseUrl {
                input: base_url.to_owned(),
                reason: error.to_string(),
            })?;

        Ok(Self {
            inner,
            base,
            service,
            timeout,
        })
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    fn host(&self) -> String {
        self.base.host_str().unwrap_or("the server").to_owned()
    }

    /// Start a request against `path`, which is resolved under the base URL's
    /// API root rather than replacing it.
    pub fn request(
        &self,
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, HttpError> {
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|error| HttpError::InvalidBaseUrl {
                input: format!("{}{path}", self.base),
                reason: error.to_string(),
            })?;
        Ok(self.inner.request(method, url))
    }

    /// Send a request, retrying only where retrying is safe, and turn any
    /// failure into something explainable.
    pub async fn send(
        &self,
        builder: reqwest::RequestBuilder,
        idempotency: Idempotency,
    ) -> Result<reqwest::Response, HttpError> {
        let mut attempt = 0;

        loop {
            let request = builder.try_clone().ok_or_else(|| HttpError::Malformed {
                service: self.service.clone(),
                reason: "request body cannot be replayed".to_owned(),
            })?;

            let request = request.build().map_err(|error| HttpError::Malformed {
                service: self.service.clone(),
                reason: error.to_string(),
            })?;

            let error = match self.inner.execute(request).await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => self.classify_response(response).await,
                Err(error) => self.classify_transport(&error),
            };
            let Some(delay) = Self::backoff(&error, idempotency, attempt) else {
                return Err(error);
            };

            tracing::warn!(
                service = %self.service,
                attempt = attempt + 1,
                delay_ms = delay.as_millis(),
                "retrying after {error}"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    /// Send a request and read the reply as `T`.
    pub async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        builder: reqwest::RequestBuilder,
        idempotency: Idempotency,
    ) -> Result<T, HttpError> {
        let response = self.send(builder, idempotency).await?;
        let body = response
            .text()
            .await
            .map_err(|error| HttpError::Malformed {
                service: self.service.clone(),
                reason: error.to_string(),
            })?;

        serde_json::from_str(&body).map_err(|error| HttpError::Malformed {
            service: self.service.clone(),
            reason: format!("{error}; the reply began: {}", snippet(&body)),
        })
    }

    /// How long to wait before trying again, or `None` to give up.
    fn backoff(error: &HttpError, idempotency: Idempotency, attempt: u32) -> Option<Duration> {
        if attempt >= MAX_RETRIES {
            return None;
        }

        let retryable = match error {
            // The server said outright that it did not process the request,
            // and a connection never established sent nothing: neither can
            // duplicate anything by being tried again.
            HttpError::RateLimited { .. } | HttpError::Unreachable { .. } => true,
            HttpError::Server { status, .. } => matches!(status, 502..=504),
            // A reply that never arrived may still have been acted on.
            HttpError::Timeout { .. } => idempotency == Idempotency::Safe,
            _ => false,
        };

        if !retryable {
            return None;
        }

        if let HttpError::RateLimited {
            retry_after: Some(seconds),
            ..
        } = error
        {
            return Some(Duration::from_secs(*seconds).min(MAX_BACKOFF));
        }

        Some(exponential_backoff(attempt))
    }

    async fn classify_response(&self, response: reqwest::Response) -> HttpError {
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let path = response.url().path().to_owned();
        let body = response.text().await.unwrap_or_default();
        let message = extract_message(&body).unwrap_or_else(|| snippet(&body));

        match status {
            StatusCode::UNAUTHORIZED => HttpError::Unauthorized {
                service: self.service.clone(),
            },
            StatusCode::FORBIDDEN => HttpError::Forbidden {
                service: self.service.clone(),
                message,
            },
            StatusCode::NOT_FOUND => HttpError::NotFound {
                service: self.service.clone(),
                what: path,
            },
            StatusCode::TOO_MANY_REQUESTS => HttpError::RateLimited {
                service: self.service.clone(),
                retry_after,
            },
            _ if status.is_server_error() => HttpError::Server {
                service: self.service.clone(),
                status: status.as_u16(),
                message,
            },
            _ => HttpError::Rejected {
                service: self.service.clone(),
                status: status.as_u16(),
                message,
            },
        }
    }

    fn classify_transport(&self, error: &reqwest::Error) -> HttpError {
        let host = self.host();
        let reason = chain(error);

        if error.is_timeout() {
            return HttpError::Timeout {
                host,
                seconds: self.timeout.as_secs(),
            };
        }
        if reason.to_lowercase().contains("certificate") || reason.contains("tls") {
            return HttpError::Tls { host, reason };
        }
        HttpError::Unreachable { host, reason }
    }
}

/// Read the base URL the user typed, filling in what they left out.
///
/// People paste `company.atlassian.net`, with and without a trailing slash.
/// The trailing slash matters: without it, joining a path would replace the
/// last segment of a URL like `https://host/kimai`.
fn parse_base_url(input: &str) -> Result<Url, HttpError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(HttpError::InvalidBaseUrl {
            input: input.to_owned(),
            reason: "the address is empty".to_owned(),
        });
    }

    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };

    let mut url = Url::parse(&with_scheme).map_err(|error| HttpError::InvalidBaseUrl {
        input: input.to_owned(),
        reason: error.to_string(),
    })?;

    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err(HttpError::InvalidBaseUrl {
            input: input.to_owned(),
            reason: "no host in the address".to_owned(),
        });
    }

    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }

    Ok(url)
}

/// Pull the human part out of an error body.
///
/// Jira answers with `errorMessages`, Kimai with `message` and a nested
/// `errors.children`; both are worth more to the user than a status code.
fn extract_message(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;

    if let Some(messages) = json.get("errorMessages").and_then(|value| value.as_array())
        && !messages.is_empty()
    {
        return Some(join_strings(messages));
    }

    let headline = ["message", "detail", "error", "error_description", "title"]
        .into_iter()
        .find_map(|key| json.get(key).and_then(|value| value.as_str()))
        .map(ToOwned::to_owned);

    let fields = json
        .get("errors")
        .map(|errors| errors.get("children").unwrap_or(errors))
        .and_then(field_errors);

    match (headline, fields) {
        (Some(headline), Some(fields)) => Some(format!("{headline} ({fields})")),
        (Some(headline), None) => Some(headline),
        (None, Some(fields)) => Some(fields),
        (None, None) => None,
    }
}

/// `field: first problem; other field: another` out of a map of field errors.
fn field_errors(errors: &serde_json::Value) -> Option<String> {
    let object = errors.as_object()?;
    let described: Vec<String> = object
        .iter()
        .filter_map(|(field, value)| {
            let messages = value
                .get("errors")
                .or(Some(value))
                .and_then(|value| value.as_array())?;
            if messages.is_empty() {
                return None;
            }
            Some(format!("{field}: {}", join_strings(messages)))
        })
        .collect();

    (!described.is_empty()).then(|| described.join("; "))
}

fn join_strings(values: &[serde_json::Value]) -> String {
    values
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Everything a `reqwest::Error` wraps, since the outer message alone is
/// usually just "error sending request".
fn chain(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts.join(": ")
}

/// Exponential, with jitter so a batch that trips a limit together does not
/// come back together.
fn exponential_backoff(attempt: u32) -> Duration {
    let base = FIRST_BACKOFF.saturating_mul(1 << attempt.min(5));
    let jitter = rand::random::<f32>().mul_add(0.4, 0.8);
    base.mul_f32(jitter).min(MAX_BACKOFF)
}

/// The start of a body, for when it is a sign-in page rather than JSON.
fn snippet(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 200 {
        format!("{}...", flat.chars().take(200).collect::<String>())
    } else if flat.is_empty() {
        "(no reply body)".to_owned()
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_host_is_assumed_to_be_https() {
        let url = parse_base_url("company.atlassian.net").unwrap();
        assert_eq!(url.as_str(), "https://company.atlassian.net/");
    }

    #[test]
    fn a_sub_path_keeps_its_trailing_slash_so_joins_do_not_eat_it() {
        let url = parse_base_url("https://host/kimai").unwrap();
        assert_eq!(
            url.join("api/projects").unwrap().as_str(),
            "https://host/kimai/api/projects"
        );
    }

    #[test]
    fn surrounding_space_is_forgiven() {
        assert_eq!(
            parse_base_url("  https://host/  ").unwrap().as_str(),
            "https://host/"
        );
    }

    #[test]
    fn an_address_with_no_host_is_refused() {
        assert!(parse_base_url("").is_err());
        assert!(parse_base_url("https://").is_err());
    }

    #[test]
    fn jira_error_messages_are_read_out_of_the_body() {
        let body = r#"{"errorMessages":["Issue does not exist"],"errors":{}}"#;
        assert_eq!(
            extract_message(body).as_deref(),
            Some("Issue does not exist")
        );
    }

    #[test]
    fn kimai_field_errors_are_folded_into_the_message() {
        let body = r#"{"message":"Validation Failed","errors":{"children":{"begin":{"errors":["This value is not valid."]}}}}"#;
        assert_eq!(
            extract_message(body).as_deref(),
            Some("Validation Failed (begin: This value is not valid.)")
        );
    }

    #[test]
    fn a_body_that_is_not_json_yields_no_message() {
        assert_eq!(extract_message("<html>Sign in</html>"), None);
    }

    #[test]
    fn a_long_body_is_cut_down_for_display() {
        let snippet = snippet(&"x".repeat(500));
        assert!(snippet.ends_with("..."));
        assert_eq!(snippet.chars().count(), 203);
    }

    #[test]
    fn a_post_is_never_retried_after_a_timeout() {
        let timeout = HttpError::Timeout {
            host: "host".to_owned(),
            seconds: 30,
        };
        assert!(ApiClient::backoff(&timeout, Idempotency::Unsafe, 0).is_none());
        assert!(ApiClient::backoff(&timeout, Idempotency::Safe, 0).is_some());
    }

    #[test]
    fn a_rate_limit_is_retried_even_for_a_post_and_honours_the_server() {
        let limited = HttpError::RateLimited {
            service: "Kimai".to_owned(),
            retry_after: Some(7),
        };
        assert_eq!(
            ApiClient::backoff(&limited, Idempotency::Unsafe, 0),
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn a_refusal_is_never_retried() {
        let rejected = HttpError::Rejected {
            service: "Jira".to_owned(),
            status: 400,
            message: "bad".to_owned(),
        };
        assert!(ApiClient::backoff(&rejected, Idempotency::Safe, 0).is_none());
    }

    #[test]
    fn retries_stop_after_the_limit() {
        let unreachable = HttpError::Unreachable {
            host: "host".to_owned(),
            reason: "refused".to_owned(),
        };
        assert!(ApiClient::backoff(&unreachable, Idempotency::Safe, MAX_RETRIES).is_none());
        assert!(ApiClient::backoff(&unreachable, Idempotency::Safe, MAX_RETRIES - 1).is_some());
    }

    #[test]
    fn backoff_grows_and_stays_under_the_ceiling() {
        assert!(exponential_backoff(0) < exponential_backoff(4));
        assert!(exponential_backoff(20) <= MAX_BACKOFF);
    }
}
