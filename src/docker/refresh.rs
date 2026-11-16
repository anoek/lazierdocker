//! Events-driven snapshot refresh (lazydocker: any docker event → throttled full refresh).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{FutureExt, StreamExt};
use tokio::sync::Notify;

use super::client::Docker;
use super::model::LocalCompose;
use crate::event::{AppEvent, EventTx};

const THROTTLE: Duration = Duration::from_millis(50);
const RETRY: Duration = Duration::from_secs(2);
/// Safety-net refresh in case events were missed.
const IDLE_REFRESH: Duration = Duration::from_secs(15);

/// Handle to request an immediate refresh (e.g. after an action completes).
#[derive(Clone)]
pub struct Refresher {
    notify: Arc<Notify>,
}

impl Refresher {
    pub fn poke(&self) {
        self.notify.notify_one();
    }
}

pub fn spawn_refresh(docker: Docker, local: Option<LocalCompose>, tx: EventTx) -> Refresher {
    let notify = Arc::new(Notify::new());

    // Events stream → poke. If the stream fails, refresh every RETRY until it
    // reconnects; the snapshot loop reports an unreachable daemon.
    {
        let docker = docker.clone();
        let notify = notify.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                let mut stream = docker.api.events(None::<bollard::query_parameters::EventsOptions>);
                while let Some(Ok(_)) = stream.next().await {
                    notify.notify_one();
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(RETRY).await;
                notify.notify_one();
            }
        });
    }

    // Snapshot loop.
    {
        let notify = notify.clone();
        tokio::spawn(async move {
            loop {
                match docker.snapshot(local.as_ref()).await {
                    Ok(s) => {
                        if tx.send(AppEvent::Snapshot(Box::new(s))).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        if tx.send(AppEvent::DockerError(format!("{e:#}"))).is_err() {
                            return;
                        }
                        tokio::time::sleep(RETRY).await;
                        continue;
                    }
                }
                tokio::select! {
                    _ = notify.notified() => {}
                    _ = tokio::time::sleep(IDLE_REFRESH) => {}
                }
                // Coalesce bursts of events (a stop emits kill, die, stop, …)
                // into one refresh, dropping the poke they left behind.
                tokio::time::sleep(THROTTLE).await;
                let _ = notify.notified().now_or_never();
            }
        });
    }

    Refresher { notify }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_reports_unreachable_daemon() {
        let docker = Docker::connect_to("tcp://127.0.0.1:1").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _r = spawn_refresh(docker, None, tx);
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.expect("event within 3s");
        assert!(matches!(ev, Some(AppEvent::DockerError(_))), "{ev:?}");
    }
}
