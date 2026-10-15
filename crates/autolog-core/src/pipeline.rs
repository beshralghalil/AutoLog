//! Posting a batch of entries without flooding the tracker.
//!
//! Entries go out several at a time behind a pace-keeper, and every one
//! reports back as it settles so the table fills in live rather than at the
//! end. The run can be stopped between entries, and stopping never leaves a
//! half-posted entry behind: each one is a single request.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt as _;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::RateLimit;
use crate::consumer::{Consumer, ConsumerError};
use crate::error::{UserError, UserFacing as _};
use crate::model::{EntryId, EntryStatus, WorklogEntry};

/// What became of one entry.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub id: EntryId,
    pub status: EntryStatus,
    /// Why, when the status needs explaining.
    pub error: Option<UserError>,
    /// The tracker's own id for what it created.
    pub reference: Option<String>,
}

impl Outcome {
    fn posted(id: EntryId, reference: Option<String>) -> Self {
        Self {
            id,
            status: EntryStatus::Success,
            error: None,
            reference,
        }
    }

    fn from_error(id: EntryId, error: &ConsumerError) -> Self {
        let status = match error {
            ConsumerError::Duplicate => EntryStatus::Skipped,
            ConsumerError::Incomplete { .. } => EntryStatus::Invalid,
            _ => EntryStatus::Failed,
        };

        Self {
            id,
            status,
            error: Some(error.to_user_error()),
            reference: None,
        }
    }
}

/// How a run is getting on.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Sent once, before anything goes out.
    Started { total: usize },
    /// An entry has left for the tracker.
    Sending { id: EntryId },
    /// An entry has settled, one way or another.
    Settled {
        outcome: Outcome,
        done: usize,
        total: usize,
    },
    /// The run ended, whether by finishing or by being stopped.
    Finished(Summary),
}

/// The tally at the end of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub posted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub invalid: usize,
    /// Entries never attempted because the run was stopped.
    pub cancelled: usize,
}

impl Summary {
    pub fn attempted(&self) -> usize {
        self.posted + self.skipped + self.failed + self.invalid
    }

    fn count(&mut self, status: EntryStatus) {
        match status {
            EntryStatus::Success => self.posted += 1,
            EntryStatus::Skipped => self.skipped += 1,
            EntryStatus::Invalid => self.invalid += 1,
            _ => self.failed += 1,
        }
    }

    /// One sentence for the dialog at the end.
    pub fn sentence(&self) -> String {
        let mut parts = vec![format!("{} posted", self.posted)];
        if self.skipped > 0 {
            parts.push(format!("{} already logged", self.skipped));
        }
        if self.invalid > 0 {
            parts.push(format!("{} incomplete", self.invalid));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.cancelled > 0 {
            parts.push(format!("{} not attempted", self.cancelled));
        }
        parts.join(", ")
    }
}

/// A flag the UI raises to stop a run between entries.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Check every entry against the consumer without sending anything.
///
/// This is what the Dry run button does: the same `validate` the real run
/// uses, so a clean dry run means the only thing left that can fail is the
/// network.
pub fn dry_run(consumer: &dyn Consumer, entries: &[WorklogEntry]) -> Vec<Outcome> {
    entries
        .iter()
        .map(|entry| match consumer.validate(entry) {
            Ok(()) => Outcome {
                id: entry.id,
                status: EntryStatus::Pending,
                error: None,
                reference: None,
            },
            Err(error) => Outcome::from_error(entry.id, &error),
        })
        .collect()
}

/// Post every entry, reporting each as it settles.
///
/// `report` is called from the runtime's threads, so it should hand the event
/// on rather than do work.
pub async fn run<F>(
    consumer: Arc<dyn Consumer>,
    entries: Vec<WorklogEntry>,
    limit: RateLimit,
    cancel: Cancel,
    report: F,
) -> Summary
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    let limit = limit.clamped();
    let total = entries.len();
    let report = Arc::new(report);
    let pacer = Arc::new(Pacer::new(limit.per_minute));
    let settled = Arc::new(AtomicUsize::new(0));

    report(Progress::Started { total });

    let outcomes: Vec<Option<Outcome>> = futures::stream::iter(entries)
        .map(|entry| {
            let consumer = Arc::clone(&consumer);
            let pacer = Arc::clone(&pacer);
            let report = Arc::clone(&report);
            let settled = Arc::clone(&settled);
            let cancel = cancel.clone();

            async move {
                if cancel.is_stopped() {
                    return None;
                }

                pacer.wait_for_a_slot().await;

                // Checked again: a slot may have been waited out after the
                // user pressed stop.
                if cancel.is_stopped() {
                    return None;
                }

                report(Progress::Sending { id: entry.id });

                let outcome = match consumer.submit(&entry).await {
                    Ok(posted) => Outcome::posted(entry.id, posted.reference),
                    Err(error) => Outcome::from_error(entry.id, &error),
                };

                // Reported here, as this entry settles, rather than after the
                // whole batch has drained: a progress bar that only moves once
                // everything is done is not a progress bar.
                let done = settled.fetch_add(1, Ordering::SeqCst) + 1;
                report(Progress::Settled {
                    outcome: outcome.clone(),
                    done,
                    total,
                });

                Some(outcome)
            }
        })
        .buffer_unordered(limit.concurrency)
        .collect()
        .await;

    let mut summary = Summary::default();
    for outcome in outcomes.into_iter().flatten() {
        summary.count(outcome.status);
    }

    summary.cancelled = total - summary.attempted();
    report(Progress::Finished(summary.clone()));
    summary
}

/// Spaces requests out so a batch does not arrive as a burst.
///
/// Handing out slots under one lock means the pace holds however many entries
/// are in flight, which a per-task sleep would not.
#[derive(Debug)]
struct Pacer {
    /// Gap between the starts of two requests, or `None` for no ceiling.
    interval: Option<Duration>,
    next_slot: Mutex<Instant>,
}

impl Pacer {
    fn new(per_minute: u32) -> Self {
        Self {
            interval: (per_minute > 0)
                .then(|| Duration::from_secs_f64(60.0 / f64::from(per_minute))),
            next_slot: Mutex::new(Instant::now()),
        }
    }

    async fn wait_for_a_slot(&self) {
        let Some(interval) = self.interval else {
            return;
        };

        let slot = {
            let mut next = self.next_slot.lock().await;
            let slot = (*next).max(Instant::now());
            *next = slot + interval;
            slot
        };

        tokio::time::sleep_until(slot).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::{ConsumerSpec, Posted};
    use crate::model::fixtures;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;

    static SPEC: ConsumerSpec = ConsumerSpec {
        id: "fake",
        label: "Fake",
        about: "",
        credentials: &[],
        targets: &[],
    };

    /// A consumer that answers however the test tells it to.
    #[derive(Debug, Default)]
    struct Fake {
        /// Entry ids that should fail, and how.
        verdicts: StdMutex<std::collections::HashMap<EntryId, ConsumerError>>,
        /// Ids that `validate` should refuse.
        invalid: StdMutex<std::collections::HashSet<EntryId>>,
        submitted: AtomicUsize,
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Consumer for Fake {
        fn spec(&self) -> &'static ConsumerSpec {
            &SPEC
        }
        async fn connect(&mut self) -> Result<(), ConsumerError> {
            Ok(())
        }
        async fn refresh(&mut self) -> Result<(), ConsumerError> {
            Ok(())
        }
        fn choices(&self, _field: &str, _entry: &WorklogEntry) -> Vec<String> {
            Vec::new()
        }
        fn every_choice(&self, _field: &str) -> Vec<String> {
            Vec::new()
        }
        fn validate(&self, entry: &WorklogEntry) -> Result<(), ConsumerError> {
            if self.invalid.lock().unwrap().contains(&entry.id) {
                return Err(ConsumerError::Incomplete {
                    what: "project".to_owned(),
                });
            }
            Ok(())
        }
        async fn preload(&mut self, _entries: &[WorklogEntry]) -> Result<(), ConsumerError> {
            Ok(())
        }
        fn is_duplicate(&self, _entry: &WorklogEntry) -> bool {
            false
        }
        async fn submit(&self, entry: &WorklogEntry) -> Result<Posted, ConsumerError> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.submitted.fetch_add(1, Ordering::SeqCst);

            if let Some(error) = self.verdicts.lock().unwrap().get(&entry.id) {
                return Err(error.clone());
            }
            Ok(Posted {
                reference: Some(format!("ref-{}", entry.id)),
            })
        }
    }

    fn entries(count: usize) -> Vec<WorklogEntry> {
        (0..count)
            .map(|id| {
                let mut entry = fixtures::entry("ABC-1");
                entry.id = id;
                entry
            })
            .collect()
    }

    fn collector() -> (
        Arc<StdMutex<Vec<Progress>>>,
        impl Fn(Progress) + Send + Sync + 'static,
    ) {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (events, move |event| sink.lock().unwrap().push(event))
    }

    fn unlimited() -> RateLimit {
        RateLimit {
            concurrency: 4,
            per_minute: 0,
        }
    }

    /// Each entry has to report as it settles, not once the batch has drained.
    ///
    /// Collecting the whole stream before reporting left the bar on zero for
    /// the length of the run and then jumped it to full, which is the one
    /// thing a progress bar must not do.
    #[tokio::test]
    async fn an_entry_reports_as_it_settles_rather_than_at_the_end() {
        let fake = Arc::new(Fake::default());
        let (events, report) = collector();

        run(
            fake as Arc<dyn Consumer>,
            entries(3),
            // One at a time, so the order below is the only possible one.
            RateLimit {
                concurrency: 1,
                per_minute: 0,
            },
            Cancel::new(),
            report,
        )
        .await;

        let order: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .map(|event| match event {
                Progress::Started { .. } => "started".to_owned(),
                Progress::Sending { .. } => "sending".to_owned(),
                Progress::Settled { done, .. } => format!("settled {done}"),
                Progress::Finished(_) => "finished".to_owned(),
            })
            .collect();

        assert_eq!(
            order,
            [
                "started",
                "sending",
                "settled 1",
                "sending",
                "settled 2",
                "sending",
                "settled 3",
                "finished",
            ],
            "each entry should settle before the next is sent"
        );
    }

    #[tokio::test]
    async fn every_entry_is_posted_and_counted() {
        let fake = Arc::new(Fake::default());
        let (events, report) = collector();

        let summary = run(
            Arc::clone(&fake) as Arc<dyn Consumer>,
            entries(7),
            unlimited(),
            Cancel::new(),
            report,
        )
        .await;

        assert_eq!(
            summary,
            Summary {
                posted: 7,
                ..Summary::default()
            }
        );
        assert_eq!(fake.submitted.load(Ordering::SeqCst), 7);

        let events = events.lock().unwrap();
        assert!(matches!(
            events.first(),
            Some(Progress::Started { total: 7 })
        ));
        assert!(matches!(events.last(), Some(Progress::Finished(_))));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Progress::Settled { .. }))
                .count(),
            7
        );
    }

    #[tokio::test]
    async fn each_kind_of_refusal_lands_on_its_own_status() {
        let fake = Arc::new(Fake::default());
        fake.verdicts
            .lock()
            .unwrap()
            .insert(1, ConsumerError::Duplicate);
        fake.verdicts.lock().unwrap().insert(
            2,
            ConsumerError::Incomplete {
                what: "project".to_owned(),
            },
        );
        fake.verdicts
            .lock()
            .unwrap()
            .insert(3, ConsumerError::NotConnected);

        let (_events, report) = collector();
        let summary = run(
            fake as Arc<dyn Consumer>,
            entries(4),
            unlimited(),
            Cancel::new(),
            report,
        )
        .await;

        assert_eq!(
            summary,
            Summary {
                posted: 1,
                skipped: 1,
                invalid: 1,
                failed: 1,
                cancelled: 0
            }
        );
        assert_eq!(
            summary.sentence(),
            "1 posted, 1 already logged, 1 incomplete, 1 failed"
        );
    }

    #[tokio::test]
    async fn stopping_leaves_the_rest_unattempted() {
        let fake = Arc::new(Fake::default());
        let cancel = Cancel::new();
        cancel.stop();

        let (_events, report) = collector();
        let summary = run(
            Arc::clone(&fake) as Arc<dyn Consumer>,
            entries(5),
            unlimited(),
            cancel,
            report,
        )
        .await;

        assert_eq!(summary.cancelled, 5);
        assert_eq!(summary.attempted(), 0);
        assert_eq!(fake.submitted.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_more_than_the_configured_number_are_in_flight() {
        let fake = Arc::new(Fake::default());
        let (_events, report) = collector();

        run(
            Arc::clone(&fake) as Arc<dyn Consumer>,
            entries(12),
            RateLimit {
                concurrency: 3,
                per_minute: 0,
            },
            Cancel::new(),
            report,
        )
        .await;

        assert!(fake.peak_in_flight.load(Ordering::SeqCst) <= 3);
        assert!(fake.peak_in_flight.load(Ordering::SeqCst) > 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_spaces_requests_out() {
        let fake = Arc::new(Fake::default());
        let (_events, report) = collector();
        let started = Instant::now();

        run(
            fake as Arc<dyn Consumer>,
            entries(6),
            RateLimit {
                concurrency: 6,
                per_minute: 60,
            },
            Cancel::new(),
            report,
        )
        .await;

        // Six requests at one a second cannot finish in under five.
        assert!(
            started.elapsed() >= Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_dry_run_reports_the_incomplete_without_sending_anything() {
        let fake = Fake::default();
        fake.invalid.lock().unwrap().insert(2);

        let outcomes = dry_run(&fake, &entries(4));

        assert_eq!(outcomes[2].status, EntryStatus::Invalid);
        assert!(
            outcomes[2]
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("project")
        );
        assert_eq!(outcomes[0].status, EntryStatus::Pending);
        assert_eq!(fake.submitted.load(Ordering::SeqCst), 0);
    }
}
