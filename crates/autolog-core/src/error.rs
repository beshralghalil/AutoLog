//! Turning failures into something a person can act on.
//!
//! Every error that can reach the window implements [`UserFacing`]. The GUI
//! never prints a `Debug` representation: it shows the message, the hint when
//! there is one, and keeps the technical detail behind a disclosure.

use std::fmt;

/// A failure described for somebody who is not reading the log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserError {
    /// Short heading, e.g. `"Could not reach Kimai"`.
    pub title: String,
    /// One or two sentences saying what went wrong.
    pub message: String,
    /// What the user can do about it, when that is knowable.
    pub hint: Option<String>,
    /// Raw technical text, shown only when the user asks for it.
    pub detail: Option<String>,
}

impl UserError {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            hint: None,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The message and hint as one line, for a table cell or a tooltip.
    pub fn summary(&self) -> String {
        match &self.hint {
            Some(hint) => format!("{} {hint}", self.message),
            None => self.message.clone(),
        }
    }
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.title, self.message)
    }
}

impl std::error::Error for UserError {}

/// Implemented by every error the user may end up looking at.
pub trait UserFacing {
    fn to_user_error(&self) -> UserError;
}

impl UserFacing for UserError {
    fn to_user_error(&self) -> UserError {
        self.clone()
    }
}

/// Fallback for errors with no dedicated presentation, e.g. `anyhow::Error`
/// escaping a background task.
impl UserFacing for anyhow::Error {
    fn to_user_error(&self) -> UserError {
        UserError::new("Something went wrong", self.to_string()).with_detail(format!("{self:?}"))
    }
}
