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

// =============================================================================
// Tests — TriggerRunner lifecycle (native only; spawn_local has no JoinHandle)
// =============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use crate::content::Content;
    use crate::types::ToolResult;
    use tokio::sync::Notify;

    /// A minimal `Connection` for trigger lifecycle tests. Counts how many
    /// trigger messages reached the wire and can gate `wait_for_idle` so a
    /// `send_when_idle` trigger blocks until the test flips it idle.
    struct MockConn {
        trigger_count: AtomicU32,
        idle: AtomicBool,
        idle_notify: Notify,
    }

    impl MockConn {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                trigger_count: AtomicU32::new(0),
                idle: AtomicBool::new(false),
                idle_notify: Notify::new(),
            })
        }
        fn go_idle(&self) {
            self.idle.store(true, Ordering::Release);
            self.idle_notify.notify_waiters();
        }
        fn triggers_sent(&self) -> u32 {
            self.trigger_count.load(Ordering::Acquire)
        }
    }

    #[async_trait]
    impl Connection for MockConn {
        fn is_idle(&self) -> bool {
            self.idle.load(Ordering::Acquire)
        }
        fn conversation_id(&self) -> &str {
            "mock"
        }
        async fn send(&self, _content: Content) -> Result<()> {
            Ok(())
        }
        async fn send_trigger(&self, _content: String) -> Result<()> {
            self.trigger_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
        async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
            Ok(())
        }
        fn subscribe_steps(&self) -> crate::connections::StepStream {
            Box::pin(futures_util::stream::empty())
        }
        async fn wait_for_idle(&self) -> Result<()> {
            loop {
                if self.is_idle() {
                    return Ok(());
                }
                self.idle_notify.notified().await;
            }
        }
        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    /// A trigger whose `run` fires exactly once (sends one message) then
    /// returns Ok, plus a flag we can inspect to know it actually executed.
    struct OneShot {
        name: String,
        ran: Arc<AtomicBool>,
        delivery: TriggerDelivery,
    }

    #[async_trait]
    impl Trigger for OneShot {
        fn name(&self) -> &str {
            &self.name
        }
        fn delivery(&self) -> TriggerDelivery {
            self.delivery
        }
        async fn run(&self, ctx: TriggerContext) -> Result<()> {
            match self.delivery {
                TriggerDelivery::WaitIdle => ctx.send_when_idle("ping").await?,
                TriggerDelivery::SendImmediately => ctx.send("ping").await?,
            }
            self.ran.store(true, Ordering::Release);
            Ok(())
        }
    }

    /// A trigger whose `run` always returns an error WITHOUT panicking — the
    /// runner must log + isolate it, not abort sibling triggers.
    struct AlwaysErr {
        name: String,
    }

    #[async_trait]
    impl Trigger for AlwaysErr {
        fn name(&self) -> &str {
            &self.name
        }
        async fn run(&self, _ctx: TriggerContext) -> Result<()> {
            Err(Error::other("trigger boom"))
        }
    }

    /// A trigger that panics inside `run` — tokio isolates the panic to the
    /// task; the runner + siblings must survive (the `JoinError` is swallowed
    /// by `stop`).
    struct Panicker {
        name: String,
    }

    #[async_trait]
    impl Trigger for Panicker {
        fn name(&self) -> &str {
            &self.name
        }
        async fn run(&self, _ctx: TriggerContext) -> Result<()> {
            panic!("trigger panic");
        }
    }

    /// CONTRACT: `start` is single-shot. A second `start` while running is an
    /// `AlreadyStarted` error, NOT a silent double-spawn (which would run every
    /// trigger twice — duplicate sends, duplicate side effects).
    #[tokio::test]
    async fn double_start_is_rejected() {
        let conn = MockConn::new();
        let runner = TriggerRunner::new(vec![], conn.clone());
        runner.start().expect("first start ok");
        let err = runner.start().expect_err("second start must fail");
        assert!(matches!(err, Error::AlreadyStarted));
        runner.stop().await;
    }

    /// CONTRACT: after `stop`, the runner is reusable — `start` may be called
    /// again (stop clears the handle slot). Guards against a regression that
    /// would leave `tasks` permanently `Some` and brick restart.
    #[tokio::test]
    async fn start_after_stop_succeeds() {
        let conn = MockConn::new();
        let runner = TriggerRunner::new(vec![], conn.clone());
        runner.start().expect("start");
        runner.stop().await;
        runner.start().expect("restart after stop");
        runner.stop().await;
    }

    /// CONTRACT: an immediate-delivery trigger reaches the wire, and `stop`
    /// JOINS (not just aborts) — after `stop` returns, the task is finished and
    /// its effect is observable.
    #[tokio::test]
    async fn immediate_trigger_delivers_and_stop_joins() {
        let conn = MockConn::new();
        let ran = Arc::new(AtomicBool::new(false));
        let trig = Arc::new(OneShot {
            name: "imm".into(),
            ran: ran.clone(),
            delivery: TriggerDelivery::SendImmediately,
        });
        let runner = TriggerRunner::new(vec![trig], conn.clone());
        runner.start().expect("start");
        // The OneShot returns after one send; poll until it lands (cooperative,
        // bounded — a hang here is a real failure surfaced by the timeout).
        tokio::time::timeout(Duration::from_secs(5), async {
            while conn.triggers_sent() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("immediate trigger must deliver");
        runner.stop().await;
        assert_eq!(conn.triggers_sent(), 1, "exactly one trigger send");
        assert!(ran.load(Ordering::Acquire), "trigger body ran to completion");
    }

    /// CONTRACT: `send_when_idle` BLOCKS until the connection is idle, then
    /// fires. This is the trigger-racing-an-active-turn case: the message must
    /// NOT land while the agent is mid-turn (idle == false).
    #[tokio::test]
    async fn send_when_idle_waits_for_idle() {
        let conn = MockConn::new(); // starts NOT idle
        let ran = Arc::new(AtomicBool::new(false));
        let trig = Arc::new(OneShot {
            name: "idle".into(),
            ran: ran.clone(),
            delivery: TriggerDelivery::WaitIdle,
        });
        let runner = TriggerRunner::new(vec![trig], conn.clone());
        runner.start().expect("start");

        // Give the trigger task a chance to reach `wait_for_idle`. While the
        // connection is busy it must NOT have sent.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(conn.triggers_sent(), 0, "must not fire while turn is active");
        assert!(!ran.load(Ordering::Acquire));

        // Flip idle; the trigger unblocks and delivers.
        conn.go_idle();
        tokio::time::timeout(Duration::from_secs(5), async {
            while conn.triggers_sent() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("idle trigger must fire once idle");
        runner.stop().await;
        assert_eq!(conn.triggers_sent(), 1);
    }

    /// CONTRACT: an erroring trigger is ISOLATED — it does not prevent a
    /// sibling trigger from running, and does not poison the runner.
    #[tokio::test]
    async fn erroring_trigger_does_not_kill_siblings() {
        let conn = MockConn::new();
        let ran = Arc::new(AtomicBool::new(false));
        let good = Arc::new(OneShot {
            name: "good".into(),
            ran: ran.clone(),
            delivery: TriggerDelivery::SendImmediately,
        });
        let bad = Arc::new(AlwaysErr { name: "bad".into() });
        let runner = TriggerRunner::new(vec![bad, good], conn.clone());
        runner.start().expect("start");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ran.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the good trigger must still run despite the erroring sibling");
        runner.stop().await;
        assert_eq!(conn.triggers_sent(), 1, "only the good trigger sent");
    }

    /// CONTRACT: a PANICKING trigger task is isolated by tokio; siblings run,
    /// and `stop` (which `.await`s the panicked handle → `JoinError`) does not
    /// itself panic.
    #[tokio::test]
    async fn panicking_trigger_is_isolated_and_stop_survives() {
        let conn = MockConn::new();
        let ran = Arc::new(AtomicBool::new(false));
        let good = Arc::new(OneShot {
            name: "good".into(),
            ran: ran.clone(),
            delivery: TriggerDelivery::SendImmediately,
        });
        let boom = Arc::new(Panicker { name: "boom".into() });
        let runner = TriggerRunner::new(vec![boom, good], conn.clone());
        runner.start().expect("start");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ran.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("sibling runs despite a panicking trigger");
        // The panicked handle is awaited inside stop(); it must not propagate.
        runner.stop().await;
        assert_eq!(conn.triggers_sent(), 1);
    }

    /// CONTRACT: `Drop` aborts running trigger tasks. A long-running periodic
    /// trigger keeps sending until the runner is dropped; after drop, the send
    /// count freezes (no task survives the runner).
    #[tokio::test(start_paused = true)]
    async fn drop_aborts_running_triggers() {
        let conn = MockConn::new();
        conn.go_idle(); // so any send_when_idle path wouldn't block (we use send())
        let counter = Arc::new(AtomicU32::new(0));
        let c2 = counter.clone();
        // A periodic trigger that bumps a counter every 10ms.
        let trig = every(Duration::from_millis(10), "tick", move |_ctx| {
            let c = c2.clone();
            async move {
                c.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
        });
        let runner = TriggerRunner::new(vec![trig], conn.clone());
        runner.start().expect("start");

        // Let the spawned periodic task reach its first `interval.tick()` and
        // register its timer with the (paused) clock before we advance it.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        // Paused clock: step time forward so the periodic handler fires a few
        // times. The `interval` skips the first immediate tick, so we advance in
        // 10ms steps and yield between each so the task gets re-polled.
        for _ in 0..4 {
            tokio::time::advance(Duration::from_millis(10)).await;
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
        }
        let before = counter.load(Ordering::Acquire);
        assert!(before >= 1, "periodic trigger fired at least once, got {before}");

        // Drop the runner — its Drop impl aborts the periodic task.
        drop(runner);
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }

        // Advance more time; an aborted task must NOT keep firing.
        for _ in 0..10 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }
        let after = counter.load(Ordering::Acquire);
        assert_eq!(
            before, after,
            "dropped runner must not keep firing ({before} -> {after})"
        );
    }
}
