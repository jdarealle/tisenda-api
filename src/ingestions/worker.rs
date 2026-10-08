use super::Manager;
use anyhow::{Context, Result};
use futures_util::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};
use tracing::Instrument;

/// Owns the single worker. Dropping the router cancels it; serve() grants a grace period.
pub(crate) struct Worker {
    stop: watch::Sender<bool>,
    stopped: watch::Receiver<bool>,
    task: Mutex<Option<JoinHandle<Result<()>>>>,
}
impl Worker {
    pub(crate) fn start(manager: Arc<Manager>) -> Arc<Self> {
        let (stop, mut stopping) = watch::channel(false);
        let (finished, stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let result = AssertUnwindSafe(async {
                loop {
                    if *stopping.borrow() {
                        return Ok(());
                    }
                    let notified = manager.wake.notified();
                    if let Some(job) = manager.store.claim().await? {
                        // A signal racing with claim leaves the claimed job recoverable.
                        if *stopping.borrow() {
                            return Ok(());
                        }
                        let span = tracing::info_span!("ingestion_job", job_id=%job.id);
                        manager.execute(job).instrument(span).await?;
                    } else {
                        tokio::select! {
                            _ = stopping.changed() => {},
                            _ = notified => {},
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                        }
                    }
                }
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("El worker de ingesta entró en pánico")));
            finished.send_replace(true);
            if result.is_err() {
                tracing::error!(
                    event = "ingestion_worker_stopped",
                    error_code = "worker_failed"
                );
            }
            result
        });
        Arc::new(Self {
            stop,
            stopped,
            task: Mutex::new(Some(task)),
        })
    }
    pub(crate) fn running(&self) -> bool {
        !*self.stopped.borrow() && !*self.stop.borrow()
    }
    pub(crate) fn stop(&self) {
        self.stop.send_replace(true);
    }
    pub(crate) async fn wait_stopped(&self) {
        let mut stopped = self.stopped.clone();
        let _ = stopped.wait_for(|v| *v).await;
    }
    pub(crate) async fn wait_shutdown_requested(&self) {
        let mut stop = self.stop.subscribe();
        tokio::select! {
            _ = stop.wait_for(|v| *v) => {},
            _ = self.wait_stopped() => {},
        }
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.stop();
        let task = self.task.lock().expect("worker mutex poisoned").take();
        if let Some(mut task) = task {
            match tokio::time::timeout(Duration::from_secs(30), &mut task).await {
                Ok(result) => result.context("El worker no terminó correctamente")?,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    tracing::warn!(event = "ingestion_worker_interrupted", grace_seconds = 30);
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.get_mut().expect("worker mutex poisoned").take() {
            task.abort();
        }
    }
}
