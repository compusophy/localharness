//! Background triggers — fire-and-forget tasks that push messages into the
//! agent. Each trigger runs in its own tokio task; the runner owns the
//! task handles and aborts them on shutdown.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
use tracing::warn;

use crate::connections::Connection;
use crate::error::{Error, Result};
use crate::runtime::MaybeSendSync;
use crate::types::TriggerDelivery;

// =============================================================================
// Context + trait
// =============================================================================

/// Runtime context available to triggers for sending messages.
#[derive(Clone)]
pub struct TriggerContext {
    connection: Arc<dyn Connection>,
}

impl TriggerContext {
    /// Create a trigger context bound to the given connection.
    pub fn new(connection: Arc<dyn Connection>) -> Self {
        Self { connection }
    }

    /// Send a trigger message into the agent immediately.
    pub async fn send(&self, content: impl Into<String>) -> Result<()> {
        self.connection.send_trigger(content.into()).await
    }

    /// Wait until the agent is idle, then send the message.
    pub async fn send_when_idle(&self, content: impl Into<String>) -> Result<()> {
        self.connection.wait_for_idle().await?;
        self.send(content).await
    }

    /// Whether the agent is currently idle.
    pub fn is_idle(&self) -> bool {
        self.connection.is_idle()
    }
}

/// A background task that pushes messages into the agent.
///
/// Implement this trait and register via [`AgentConfig::with_trigger`]
/// to run periodic or event-driven logic alongside the agent loop.
///
/// [`AgentConfig::with_trigger`]: crate::AgentConfig::with_trigger
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Trigger: MaybeSendSync {
    /// Unique name for diagnostics.
    fn name(&self) -> &str;
    /// Whether messages are sent immediately or after the agent idles.
    fn delivery(&self) -> TriggerDelivery {
        TriggerDelivery::WaitIdle
    }
    /// The trigger's main loop. Runs in its own task until it returns or is aborted.
    async fn run(&self, ctx: TriggerContext) -> Result<()>;
}

// =============================================================================
// Runner
// =============================================================================

/// Owns trigger tasks and manages their lifecycle.
pub struct TriggerRunner {
    triggers: Vec<Arc<dyn Trigger>>,
    connection: Arc<dyn Connection>,
    #[cfg(not(target_arch = "wasm32"))]
    tasks: Mutex<Option<Vec<JoinHandle<()>>>>,
    // On wasm we use spawn_local which returns no handle - shutdown is
    // best-effort (page reload cleans up). Track whether started.
    #[cfg(target_arch = "wasm32")]
    started: Mutex<bool>,
}

impl TriggerRunner {
    /// Create a runner with the given triggers bound to a connection.
    pub fn new(triggers: Vec<Arc<dyn Trigger>>, connection: Arc<dyn Connection>) -> Self {
        Self {
            triggers,
            connection,
            #[cfg(not(target_arch = "wasm32"))]
            tasks: Mutex::new(None),
            #[cfg(target_arch = "wasm32")]
            started: Mutex::new(false),
        }
    }

    /// Spawn all trigger tasks. Returns `AlreadyStarted` if called twice.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(&self) -> Result<()> {
        let mut guard = self.tasks.lock();
        if guard.is_some() {
            return Err(Error::AlreadyStarted);
        }
        let mut handles = Vec::with_capacity(self.triggers.len());
        for trig in &self.triggers {
            let ctx = TriggerContext::new(self.connection.clone());
            let trig = trig.clone();
            handles.push(tokio::spawn(async move {
                let name = trig.name().to_string();
                if let Err(e) = trig.run(ctx).await {
                    warn!(%name, error = %e, "trigger exited with error");
                }
            }));
        }
        *guard = Some(handles);
        Ok(())
    }

    /// Spawn all trigger tasks (wasm variant, fire-and-forget).
    #[cfg(target_arch = "wasm32")]
    pub fn start(&self) -> Result<()> {
        let mut guard = self.started.lock();
        if *guard {
            return Err(Error::AlreadyStarted);
        }
        for trig in &self.triggers {
            let ctx = TriggerContext::new(self.connection.clone());
            let trig = trig.clone();
            crate::runtime::spawn(async move {
                let name = trig.name().to_string();
                if let Err(e) = trig.run(ctx).await {
                    warn!(%name, error = %e, "trigger exited with error");
                }
            });
        }
        *guard = true;
        Ok(())
    }

    /// Abort all running trigger tasks and wait for them to finish.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn stop(&self) {
        let handles = self.tasks.lock().take();
        if let Some(handles) = handles {
            for h in &handles {
                h.abort();
            }
            for h in handles {
                let _ = h.await;
            }
        }
    }

    /// Mark triggers as stopped (wasm: best-effort, no abort handle).
    #[cfg(target_arch = "wasm32")]
    pub async fn stop(&self) {
        // spawn_local has no abort handle; rely on page lifecycle.
        *self.started.lock() = false;
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for TriggerRunner {
    fn drop(&mut self) {
        if let Some(handles) = self.tasks.lock().take() {
            for h in handles {
                h.abort();
            }
        }
    }
}

// =============================================================================
// Built-in helpers
// =============================================================================

/// Runs `handler` every `period`. Mirrors Python's `triggers.every()`.
#[cfg(not(target_arch = "wasm32"))]
pub fn every<F, Fut>(period: Duration, name: impl Into<String>, handler: F) -> Arc<dyn Trigger>
where
    F: Fn(TriggerContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(PeriodicTrigger {
        name: name.into(),
        period,
        handler: Arc::new(move |c| Box::pin(handler(c))),
    })
}

#[cfg(target_arch = "wasm32")]
pub fn every<F, Fut>(period: Duration, name: impl Into<String>, handler: F) -> Arc<dyn Trigger>
where
    F: Fn(TriggerContext) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<()>> + 'static,
{
    Arc::new(PeriodicTrigger {
        name: name.into(),
        period,
        handler: Arc::new(move |c| Box::pin(handler(c))),
    })
}

#[cfg(not(target_arch = "wasm32"))]
struct PeriodicTrigger {
    name: String,
    period: Duration,
    handler: Arc<
        dyn Fn(TriggerContext) -> futures_util::future::BoxFuture<'static, Result<()>>
            + Send
            + Sync,
    >,
}

#[cfg(target_arch = "wasm32")]
struct PeriodicTrigger {
    name: String,
    period: Duration,
    handler: Arc<
        dyn Fn(TriggerContext) -> futures_util::future::LocalBoxFuture<'static, Result<()>>,
    >,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Trigger for PeriodicTrigger {
    fn name(&self) -> &str {
        &self.name
    }
    async fn run(&self, ctx: TriggerContext) -> Result<()> {
        let mut ticker = tokio::time::interval(self.period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Skip the first immediate tick to match Python `every` semantics.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = (self.handler)(ctx.clone()).await {
                warn!(name = %self.name, error = %e, "periodic trigger handler errored");
            }
        }
    }
}
