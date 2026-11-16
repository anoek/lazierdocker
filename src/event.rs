//! Events delivered to the app loop from background tasks.

use ratatui::text::Line;

use crate::app::inspect::ContentKey;
use crate::docker::model::Snapshot;
use crate::docker::stats::StatsSample;
use crate::ui::layout::PaneId;

#[derive(Debug)]
pub enum AppEvent {
    /// Fresh docker state.
    Snapshot(Box<Snapshot>),
    /// The daemon could not be reached / the events stream failed.
    DockerError(String),
    /// Loaded document content for a pane.
    Doc { pane: PaneId, key: ContentKey, lines: Vec<Line<'static>> },
    /// A terminal surface produced output.
    TermActivity(PaneId),
    /// A terminal surface's child process exited: (pane, surface id, code).
    TermExited(PaneId, u64, Option<u32>),
    /// A container log stream ended.
    LogsEnded(PaneId, ContentKey),
    /// One stats sample for a container.
    Stats(String, StatsSample),
}

pub type EventTx = tokio::sync::mpsc::UnboundedSender<AppEvent>;
pub type EventRx = tokio::sync::mpsc::UnboundedReceiver<AppEvent>;
