//! Events delivered to the app loop from background tasks.

use crate::docker::model::Snapshot;

#[derive(Debug)]
pub enum AppEvent {
    /// Fresh docker state.
    Snapshot(Box<Snapshot>),
    /// The daemon could not be reached / the events stream failed.
    DockerError(String),
}

pub type EventTx = tokio::sync::mpsc::UnboundedSender<AppEvent>;
pub type EventRx = tokio::sync::mpsc::UnboundedReceiver<AppEvent>;
