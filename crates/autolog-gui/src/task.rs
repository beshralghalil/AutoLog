//! Keeping slow work off the painting thread.
//!
//! Reading a spreadsheet, talking to a tracker and asking the keyring for a
//! token all take long enough to drop frames. Each runs on the Tokio runtime
//! and reports back as a [`Message`]; the window picks those up at the top of
//! the next frame, which is the only place the state is ever changed.

use std::future::Future;

use autolog_core::consumer::{Consumer, ConsumerError};
use autolog_core::error::UserError;
use autolog_core::pipeline::{Progress, Summary};
use autolog_core::source::{LoadedFile, SourceError};
use crossbeam_channel::{Receiver, Sender};

/// Something finished in the background.
#[derive(Debug)]
pub enum Message {
    /// A file was read, or could not be.
    Loaded {
        path: std::path::PathBuf,
        result: Box<Result<LoadedFile, SourceError>>,
    },
    /// A tracker was signed in to and its names fetched.
    Connected {
        /// Which consumer this is for, so a reply that arrives after the user
        /// has switched trackers can be dropped.
        consumer_id: String,
        result: Result<Box<dyn Consumer>, ConsumerError>,
    },
    /// A posting run moved along.
    Run(Progress),
    /// A posting run could not start at all.
    RunFailed(Box<UserError>),
    /// Credentials were read out of the system store.
    CredentialsLoaded {
        consumer_id: String,
        /// The stored values, or why the store would not give them up. A
        /// store that cannot be read is not the same as one holding nothing,
        /// and only the latter is safe to save over.
        result: Result<Vec<(String, String)>, autolog_core::secrets::SecretError>,
    },
}

/// The Tokio runtime, and the channel back to the window.
#[derive(Debug)]
pub struct Tasks {
    runtime: tokio::runtime::Runtime,
    emitter: Emitter,
    inbox: Receiver<Message>,
}

impl Tasks {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("autolog-net")
            .build()?;

        let (sender, inbox) = crossbeam_channel::unbounded();

        Ok(Self {
            runtime,
            emitter: Emitter {
                sender,
                ctx: ctx.clone(),
            },
            inbox,
        })
    }

    /// Run `work` on the runtime and deliver whatever it returns.
    pub fn spawn<F>(&self, work: F)
    where
        F: Future<Output = Message> + Send + 'static,
    {
        let emitter = self.emitter.clone();
        self.runtime.spawn(async move {
            emitter.send(work.await);
        });
    }

    /// Run blocking work — a file read, a keyring lookup — off the UI thread.
    pub fn spawn_blocking<F>(&self, work: F)
    where
        F: FnOnce() -> Message + Send + 'static,
    {
        let emitter = self.emitter.clone();
        self.runtime.spawn_blocking(move || {
            emitter.send(work());
        });
    }

    /// Run `work`, which reports progress of its own as it goes.
    pub fn spawn_reporting<F, Fut>(&self, work: F)
    where
        F: FnOnce(Emitter) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let emitter = self.emitter.clone();
        self.runtime.spawn(work(emitter));
    }

    /// Everything that arrived since the last frame.
    pub fn drain(&self) -> impl Iterator<Item = Message> + '_ {
        self.inbox.try_iter()
    }
}

/// A way back to the window from a background task.
#[derive(Clone)]
pub struct Emitter {
    sender: Sender<Message>,
    ctx: egui::Context,
}

impl Emitter {
    /// Hand a message to the window and wake it to paint.
    ///
    /// Without the repaint the message would sit unread until the user moved
    /// the mouse, which looks exactly like the app having hung.
    pub fn send(&self, message: Message) {
        if self.sender.send(message).is_ok() {
            self.ctx.request_repaint();
        }
    }

    pub fn progress(&self, progress: Progress) {
        self.send(Message::Run(progress));
    }

    pub fn failed(&self, error: UserError) {
        self.send(Message::RunFailed(Box::new(error)));
    }

    /// Report a run that never got off the ground, so the UI still leaves the
    /// running state.
    pub fn aborted(&self, error: UserError) {
        self.failed(error);
        self.progress(Progress::Finished(Summary::default()));
    }
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Emitter")
    }
}
