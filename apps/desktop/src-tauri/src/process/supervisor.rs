use crate::activity::{sanitize_message, ActivityEventKind, ActivityLevel, ActivityLog};
use crate::deadline::Deadline;
use crate::error::{DesktopError, DesktopResult};
use crate::platform;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use webcodex_process::{GracefulTermination, ManagedChild};

const LOG_LINES: usize = 80;
const LOG_LINE_BYTES: usize = 2048;
const MACHINE_LINE_BYTES: usize = 16 * 1024;
const MACHINE_EVENT_CAPACITY: usize = 64;
const MACHINE_CRITICAL_RESERVE: usize = 8;
const GRACEFUL_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const LOCAL_EOF_GRACE: std::time::Duration = std::time::Duration::from_millis(250);
const PROCESS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);
const PROCESS_DIAGNOSTIC_MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", content = "tunnel_profile_id", rename_all = "snake_case")]
pub enum ProcessKey {
    LocalServer,
    LocalRunner,
    QuickShare,
    RegularTunnel(crate::connection_id::TunnelProfileId),
}

impl ProcessKey {
    pub fn tunnel_profile_id(self) -> Option<crate::connection_id::TunnelProfileId> {
        match self {
            Self::RegularTunnel(id) => Some(id),
            _ => None,
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::LocalServer => "service",
            Self::LocalRunner => "runner",
            Self::QuickShare => "quick_share",
            Self::RegularTunnel(_) => "regular_tunnel",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPhase {
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub kind: ProcessKey,
    pub generation: u64,
    pub phase: ProcessPhase,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub owned_by_desktop: bool,
}

struct ManagedProcess {
    generation: u64,
    child: ManagedChild,
    phase: ProcessPhase,
    exit_code: Option<i32>,
    logs: Arc<Mutex<VecDeque<String>>>,
    observation_failures: u32,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
}

#[derive(Default)]
struct MachineEventState {
    queue: VecDeque<Value>,
    closed: bool,
    dropped_progress: u64,
    dropped_critical: u64,
}

#[derive(Clone)]
struct MachineEventSender {
    state: Arc<Mutex<MachineEventState>>,
    notify: Arc<Notify>,
}

pub(crate) struct MachineEventReceiver {
    key: Option<ProcessKey>,
    state: Arc<Mutex<MachineEventState>>,
    notify: Arc<Notify>,
}

fn machine_event_channel() -> (MachineEventSender, MachineEventReceiver) {
    let state = Arc::new(Mutex::new(MachineEventState::default()));
    let notify = Arc::new(Notify::new());
    (
        MachineEventSender {
            state: Arc::clone(&state),
            notify: Arc::clone(&notify),
        },
        MachineEventReceiver {
            key: None,
            state,
            notify,
        },
    )
}

impl MachineEventReceiver {
    pub(crate) async fn recv(&mut self) -> Option<Value> {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.dropped_critical > 0 {
                    let dropped = std::mem::take(&mut state.dropped_critical);
                    return Some(serde_json::json!({
                        "event": "machine_event_overflow",
                        "dropped_critical": dropped,
                    }));
                }
                if let Some(mut value) = state.queue.pop_front() {
                    if let (Some(id), Some(object)) = (
                        self.key.and_then(ProcessKey::tunnel_profile_id),
                        value.as_object_mut(),
                    ) {
                        // The supervisor owns attribution, never the child payload.
                        object.insert("tunnel_profile_id".into(), serde_json::json!(id));
                    }
                    return Some(value);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

impl MachineEventSender {
    fn send(&self, value: Value) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return;
        }
        if machine_event_is_progress(&value) {
            let progress_limit = MACHINE_EVENT_CAPACITY.saturating_sub(MACHINE_CRITICAL_RESERVE);
            if state.queue.len() >= progress_limit {
                if let Some(existing) = state
                    .queue
                    .iter_mut()
                    .rev()
                    .find(|event| machine_event_is_progress(event))
                {
                    *existing = value;
                } else {
                    state.dropped_progress = state.dropped_progress.saturating_add(1);
                }
                return;
            }
            state.queue.push_back(value);
        } else {
            if state.queue.len() >= MACHINE_EVENT_CAPACITY {
                if let Some(index) = state.queue.iter().position(machine_event_is_progress) {
                    state.queue.remove(index);
                    state.dropped_progress = state.dropped_progress.saturating_add(1);
                } else if machine_event_is_terminal(&value) {
                    if let Some(index) = state
                        .queue
                        .iter()
                        .position(|event| !machine_event_is_terminal(event))
                    {
                        state.queue.remove(index);
                    } else {
                        state.queue.pop_front();
                    }
                    state.dropped_critical = state.dropped_critical.saturating_add(1);
                } else {
                    state.dropped_critical = state.dropped_critical.saturating_add(1);
                    drop(state);
                    self.notify.notify_one();
                    return;
                }
            }
            state.queue.push_back(value);
        }
        drop(state);
        self.notify.notify_one();
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        drop(state);
        self.notify.notify_waiters();
    }
}

fn machine_event_is_progress(value: &Value) -> bool {
    value.get("event").and_then(Value::as_str) == Some("progress")
}

fn machine_event_is_terminal(value: &Value) -> bool {
    matches!(
        value.get("event").and_then(Value::as_str),
        Some("ready" | "error" | "failed" | "stopped" | "exit" | "exited" | "terminal")
    )
}

pub struct ProcessSupervisor {
    processes: HashMap<ProcessKey, ManagedProcess>,
    next_generation: u64,
    activity: ActivityLog,
}

impl ProcessSupervisor {
    pub fn new(activity: ActivityLog) -> Self {
        Self {
            processes: HashMap::new(),
            next_generation: 1,
            activity,
        }
    }

    pub async fn spawn_owned(
        &mut self,
        kind: ProcessKey,
        mut command: Command,
        machine_stdout: bool,
    ) -> DesktopResult<Option<MachineEventReceiver>> {
        self.refresh();
        if self.processes.get(&kind).is_some_and(|process| {
            matches!(
                process.phase,
                ProcessPhase::Starting | ProcessPhase::Running | ProcessPhase::Stopping
            )
        }) {
            return Err(DesktopError::new(
                "process_already_running",
                format!("Desktop already owns an active {kind:?} process"),
                "Stop the existing Desktop-owned process first.",
            ));
        }
        if self.processes.contains_key(&kind) {
            // A terminal direct child may still own live descendants. Keep the
            // exact ManagedChild generation until its whole tree is reclaimed;
            // never retarget cleanup by a remembered numeric PID/PGID.
            self.stop_checked(kind).await?;
        }

        let generation = self.next_generation;
        self.next_generation = generation.checked_add(1).ok_or_else(|| {
            DesktopError::new(
                "process_generation_exhausted",
                "Process generations are exhausted",
                "Restart Desktop.",
            )
        })?;

        // stdin is the Desktop parent-liveness lease for every long-lived
        // generation. Quick Share/Tunnel already consume EOF; Local Server and
        // Runner do so only when Desktop adds their explicit opt-in CLI flag.
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // The Desktop owns the Runner itself, but the Runner is also a trusted
        // process supervisor: each user command is immediately placed into the
        // Runner's own ManagedChild Job Object. Let only this direct child
        // silently break descendants away from the Desktop's outer Job to avoid
        // nested-Job incompatibilities (notably Git for Windows/MSYS) while
        // preserving exact ownership at both lifecycle layers.
        let silent_child_breakaway = kind == ProcessKey::LocalRunner;
        let mut child = ManagedChild::spawn_with_options(
            &mut command,
            platform::managed_spawn_options(silent_child_breakaway),
        )
        .map_err(|error| {
            DesktopError::new(
                "process_start_failed",
                format!("Could not start the {kind:?} process"),
                "Check the configured WebCodex binaries and retry.",
            )
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
        let pid = child.id();
        let stdout = child.child_mut().stdout.take().ok_or_else(|| {
            DesktopError::new(
                "process_start_failed",
                "Could not capture process output",
                "Retry the operation.",
            )
        })?;
        let stderr = child.child_mut().stderr.take().ok_or_else(|| {
            DesktopError::new(
                "process_start_failed",
                "Could not capture process diagnostics",
                "Retry the operation.",
            )
        })?;

        let logs = Arc::new(Mutex::new(VecDeque::new()));
        let (machine_tx, machine_rx) = if machine_stdout {
            let (tx, mut rx) = machine_event_channel();
            rx.key = Some(kind);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let stdout_logs = Arc::clone(&logs);
        let stdout_task = tokio::task::spawn_blocking(move || {
            drain_stream(
                stdout,
                stdout_logs,
                machine_tx,
                machine_stdout || matches!(kind, ProcessKey::RegularTunnel(_)),
                "stdout",
            )
        });
        let stderr_logs = Arc::clone(&logs);
        let stderr_task = tokio::task::spawn_blocking(move || {
            drain_stream(stderr, stderr_logs, None, false, "stderr")
        });
        self.activity.push_for_profile(
            kind.tunnel_profile_id(),
            ActivityEventKind::ProcessStarted,
            kind.source(),
            ActivityLevel::Info,
            format!("Desktop started the process (PID {pid})"),
        );
        self.processes.insert(
            kind,
            ManagedProcess {
                generation,
                child,
                phase: ProcessPhase::Starting,
                exit_code: None,
                logs,
                observation_failures: 0,
                stdout_task,
                stderr_task,
            },
        );
        Ok(machine_rx)
    }

    pub fn refresh(&mut self) {
        for (kind, process) in &mut self.processes {
            if !matches!(
                process.phase,
                ProcessPhase::Starting | ProcessPhase::Running
            ) {
                continue;
            }
            match process.child.try_wait() {
                Ok(Some(status)) => {
                    process.observation_failures = 0;
                    process.exit_code = status.code();
                    process.phase = if status.success() {
                        ProcessPhase::Exited
                    } else {
                        ProcessPhase::Failed
                    };
                    self.activity.push_for_profile(
                        kind.tunnel_profile_id(),
                        ActivityEventKind::ProcessExited,
                        kind.source(),
                        if status.success() {
                            ActivityLevel::Info
                        } else {
                            ActivityLevel::Error
                        },
                        format!("Desktop-owned process exited with status {status}"),
                    );
                }
                Ok(None) => {
                    process.observation_failures = 0;
                    process.phase = ProcessPhase::Running;
                }
                Err(error) => {
                    process.observation_failures =
                        process.observation_failures.saturating_add(1);
                    let pid = process.child.id();
                    let generation = process.generation;
                    let tree_state = process.child.try_tree_exit();
                    let tree_state_text = match &tree_state {
                        Ok(true) => "empty".to_string(),
                        Ok(false) => "active".to_string(),
                        Err(tree_error) => format!(
                            "unavailable(kind={:?}, raw_os_error={:?}, error={})",
                            tree_error.kind(),
                            tree_error.raw_os_error(),
                            tree_error
                        ),
                    };

                    if process.observation_failures == 1 {
                        let summary = format!(
                            "Desktop could not observe the child process state: kind={kind:?}, pid={pid}, generation={generation}, io_kind={:?}, raw_os_error={:?}, error={}, job_tree={tree_state_text}",
                            error.kind(),
                            error.raw_os_error(),
                            error
                        );
                        let recent = process
                            .logs
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .iter()
                            .rev()
                            .take(8)
                            .cloned()
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>();
                        let diagnostic_path =
                            persist_managed_process_diagnostic(*kind, pid, generation, &summary, &recent);
                        let summary = match diagnostic_path {
                            Some(path) => format!(
                                "{summary}; diagnostic_file={}",
                                path.to_string_lossy()
                            ),
                            None => summary,
                        };
                        self.activity.push_for_profile(
                            kind.tunnel_profile_id(),
                            ActivityEventKind::ProcessObservationFailed,
                            kind.source(),
                            ActivityLevel::Error,
                            summary,
                        );

                        for line in recent {
                            self.activity.push_for_profile(
                                kind.tunnel_profile_id(),
                                ActivityEventKind::ProcessObservationFailed,
                                kind.source(),
                                ActivityLevel::Error,
                                format!("Recent managed-process diagnostic: {line}"),
                            );
                        }
                    }

                    // A failed direct-child status query must not tear down a
                    // still-live Job Object tree. The regular-tunnel observer
                    // also watches the machine-event pipe, so a dead parent is
                    // still detected when that pipe closes. If the Job Object is
                    // empty (or cannot itself be queried), ownership is no
                    // longer safely observable and the process is failed.
                    process.phase = match tree_state {
                        Ok(false) => ProcessPhase::Running,
                        Ok(true) | Err(_) => ProcessPhase::Failed,
                    };
                }
            }
        }
    }

    pub fn snapshot(&mut self, kind: ProcessKey) -> Option<ProcessSnapshot> {
        self.refresh();
        self.processes.get(&kind).map(|process| ProcessSnapshot {
            kind,
            generation: process.generation,
            phase: process.phase,
            pid: Some(process.child.id()),
            exit_code: process.exit_code,
            owned_by_desktop: true,
        })
    }

    pub async fn stop(&mut self, kind: ProcessKey) {
        self.stop_until(kind, Deadline::after(GRACEFUL_STOP_TIMEOUT))
            .await;
    }

    /// Replacement must not overwrite a generation whose cleanup is uncertain.
    pub async fn stop_checked(&mut self, kind: ProcessKey) -> DesktopResult<()> {
        self.stop(kind).await;
        if self.processes.contains_key(&kind) {
            return Err(DesktopError::new(
                "process_stop_unconfirmed",
                "Process cleanup could not be confirmed",
                "Retry stopping the Desktop-owned process before replacing it.",
            ));
        }
        Ok(())
    }

    pub async fn stop_until(&mut self, kind: ProcessKey, deadline: Deadline) {
        let Some(mut process) = self.processes.remove(&kind) else {
            return;
        };
        // Closing the Desktop side of stdin is the generation-scoped parent
        // lease. Do it even if the direct child was already observed terminal:
        // a descendant may still hold the child side of the pipe.
        drop(process.child.child_mut().stdin.take());
        if matches!(
            process.phase,
            ProcessPhase::Starting | ProcessPhase::Running
        ) {
            process.phase = ProcessPhase::Stopping;
            self.activity.push_for_profile(
                kind.tunnel_profile_id(),
                ActivityEventKind::ProcessStopping,
                kind.source(),
                ActivityLevel::Info,
                "Stopping the Desktop-owned process",
            );

            let now = tokio::time::Instant::now();
            let eof_deadline =
                if matches!(kind, ProcessKey::QuickShare | ProcessKey::RegularTunnel(_)) {
                    deadline.instant()
                } else {
                    std::cmp::min(deadline.instant(), now + LOCAL_EOF_GRACE)
                };
            let graceful = wait_for_tree_exit(&mut process.child, eof_deadline).await;
            if !graceful && tokio::time::Instant::now() < deadline.instant() {
                if matches!(
                    process.child.request_terminate_tree(),
                    Ok(GracefulTermination::Requested)
                ) {
                    let signal_deadline = std::cmp::min(
                        deadline.instant(),
                        tokio::time::Instant::now() + LOCAL_EOF_GRACE,
                    );
                    let _ = wait_for_tree_exit(&mut process.child, signal_deadline).await;
                }
            }
            if !process.child.try_tree_exit().unwrap_or(false) {
                let _ = process.child.terminate_tree();
                let _ = wait_for_tree_exit(&mut process.child, deadline.instant()).await;
            }
        }
        if !process.child.try_tree_exit().unwrap_or(false) {
            let _ = process.child.terminate_tree();
            let _ = wait_for_tree_exit(&mut process.child, deadline.instant()).await;
        }
        if !process.child.try_tree_exit().unwrap_or(false) {
            process.phase = ProcessPhase::Stopping;
            self.processes.insert(kind, process);
            self.activity.push_for_profile(
                kind.tunnel_profile_id(),
                ActivityEventKind::ProcessObservationFailed,
                kind.source(),
                ActivityLevel::Error,
                "Process cleanup is unconfirmed; Desktop retained ownership for retry",
            );
            return;
        }
        finish_drain_task(process.stdout_task, deadline.instant()).await;
        finish_drain_task(process.stderr_task, deadline.instant()).await;
        self.activity.push_for_profile(
            kind.tunnel_profile_id(),
            ActivityEventKind::ProcessStopped,
            kind.source(),
            ActivityLevel::Info,
            "Desktop-owned process stopped",
        );
    }

    pub fn keys(&mut self) -> Vec<ProcessKey> {
        self.refresh();
        self.processes.keys().copied().collect()
    }

    /// A stale monitor may reclaim only its own generation, never a replacement.
    pub async fn stop_generation(&mut self, key: ProcessKey, generation: u64) {
        if self
            .processes
            .get(&key)
            .is_some_and(|p| p.generation == generation)
        {
            self.stop(key).await;
        }
    }

    pub async fn stop_all(&mut self) {
        // Stop exposures before the shared runtime, including every profile.
        let mut keys = self.keys();
        keys.sort_by_key(|key| match key {
            ProcessKey::QuickShare => 0,
            ProcessKey::RegularTunnel(_) => 1,
            ProcessKey::LocalRunner => 2,
            ProcessKey::LocalServer => 3,
        });
        for key in keys {
            self.stop(key).await;
        }
    }
}

async fn wait_for_tree_exit(child: &mut ManagedChild, deadline: tokio::time::Instant) -> bool {
    loop {
        let _ = child.try_wait();
        if child.try_tree_exit().unwrap_or(false) {
            return true;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        tokio::time::sleep_until(std::cmp::min(deadline, now + PROCESS_POLL_INTERVAL)).await;
    }
}

async fn finish_drain_task(mut task: JoinHandle<()>, deadline: tokio::time::Instant) {
    if tokio::time::Instant::now() >= deadline
        || tokio::time::timeout_at(deadline, &mut task).await.is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

fn managed_process_diagnostic_path() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("LOCALAPPDATA")?;
        return Some(
            std::path::PathBuf::from(root)
                .join("WebCodex")
                .join("diagnostics")
                .join("managed-process.log"),
        );
    }
    #[cfg(not(windows))]
    {
        if let Some(root) = std::env::var_os("XDG_STATE_HOME") {
            return Some(
                std::path::PathBuf::from(root)
                    .join("webcodex")
                    .join("diagnostics")
                    .join("managed-process.log"),
            );
        }
        let home = std::env::var_os("HOME")?;
        Some(
            std::path::PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("webcodex")
                .join("diagnostics")
                .join("managed-process.log"),
        )
    }
}

fn persist_managed_process_diagnostic(
    kind: ProcessKey,
    pid: u32,
    generation: u64,
    summary: &str,
    recent: &[String],
) -> Option<std::path::PathBuf> {
    let path = managed_process_diagnostic_path()?;
    let parent = path.parent()?;
    std::fs::create_dir_all(parent).ok()?;
    if std::fs::metadata(&path)
        .is_ok_and(|metadata| metadata.len() > PROCESS_DIAGNOSTIC_MAX_BYTES)
    {
        let _ = std::fs::remove_file(&path);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let safe_summary = sanitize_message(summary);
    writeln!(
        file,
        "[{timestamp_ms}] kind={kind:?} pid={pid} generation={generation} {safe_summary}"
    )
    .ok()?;
    for line in recent {
        writeln!(file, "  {}", sanitize_message(line)).ok()?;
    }
    Some(path)
}

fn drain_stream<R>(
    mut reader: R,
    logs: Arc<Mutex<VecDeque<String>>>,
    machine_tx: Option<MachineEventSender>,
    parse_machine_events: bool,
    stream_name: &'static str,
) where
    R: Read,
{
    let mut buffer = [0_u8; 4096];
    let mut line = Vec::with_capacity(4096);
    let line_limit = if parse_machine_events {
        MACHINE_LINE_BYTES
    } else {
        LOG_LINE_BYTES
    };
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                process_line(
                    &line,
                    &logs,
                    machine_tx.as_ref(),
                    parse_machine_events,
                    stream_name,
                );
                line.clear();
            } else if line.len() < line_limit {
                line.push(*byte);
            }
        }
    }
    if !line.is_empty() {
        process_line(
                    &line,
                    &logs,
                    machine_tx.as_ref(),
                    parse_machine_events,
                    stream_name,
                );
    }
    if let Some(tx) = machine_tx {
        tx.close();
    }
}

fn process_line(
    line: &[u8],
    logs: &Arc<Mutex<VecDeque<String>>>,
    machine_tx: Option<&MachineEventSender>,
    parse_machine_events: bool,
    stream_name: &'static str,
) {
    let text = String::from_utf8_lossy(line).trim().to_string();
    if text.is_empty() {
        return;
    }
    if parse_machine_events {
        if let (Some(tx), Ok(value)) = (machine_tx, serde_json::from_str::<Value>(&text)) {
            tx.send(value);
            return;
        }
    }
    let mut logs = logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    logs.push_back(format!("{stream_name}: {}", sanitize_message(&text)));
    while logs.len() > LOG_LINES {
        logs.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn machine_event_progress_flood_stays_bounded_and_ready_is_observed() {
        let (sender, mut receiver) = machine_event_channel();
        for sequence in 0..10_000_u64 {
            sender.send(serde_json::json!({
                "event": "progress",
                "sequence": sequence,
            }));
        }
        sender.send(serde_json::json!({ "event": "ready", "schema_version": 1 }));
        sender.close();

        let queued = sender
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queue
            .len();
        assert!(queued <= MACHINE_EVENT_CAPACITY);

        let mut ready = false;
        while let Some(value) = receiver.recv().await {
            if value.get("event").and_then(Value::as_str) == Some("ready") {
                ready = true;
                break;
            }
        }
        assert!(ready, "readiness event must survive a noisy progress flood");
    }

    #[tokio::test]
    async fn critical_overflow_is_explicit_and_terminal_event_is_retained() {
        let (sender, mut receiver) = machine_event_channel();
        for sequence in 0..MACHINE_EVENT_CAPACITY {
            sender.send(serde_json::json!({
                "event": "diagnostic",
                "sequence": sequence,
            }));
        }
        sender.send(serde_json::json!({ "event": "terminal", "status": "failed" }));
        sender.close();

        let mut saw_overflow = false;
        let mut saw_terminal = false;
        while let Some(value) = receiver.recv().await {
            match value.get("event").and_then(Value::as_str) {
                Some("machine_event_overflow") => saw_overflow = true,
                Some("terminal") => saw_terminal = true,
                _ => {}
            }
        }
        assert!(saw_overflow, "critical loss must never be silent");
        assert!(saw_terminal, "terminal event must remain observable");
    }

    #[tokio::test]
    async fn supervisor_only_stops_children_it_owns() {
        let activity = ActivityLog::default();
        let mut supervisor = ProcessSupervisor::new(activity);
        supervisor.stop(ProcessKey::LocalRunner).await;
        assert!(supervisor.snapshot(ProcessKey::LocalRunner).is_none());
    }

    #[test]
    fn process_kind_has_no_generic_process_surface() {
        let kinds = [
            ProcessKey::LocalServer,
            ProcessKey::LocalRunner,
            ProcessKey::QuickShare,
            ProcessKey::RegularTunnel(crate::connection_id::TunnelProfileId::DEFAULT),
        ];
        assert_eq!(kinds.len(), 4);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_parent_liveness_lease_closes_on_desktop_stop() {
        let marker = std::env::temp_dir().join(format!(
            "webcodex-desktop-local-parent-eof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let marker_arg = marker.to_string_lossy().into_owned();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "cat >/dev/null; printf eof > \"$1\"",
            "webcodex-parent-eof",
            marker_arg.as_str(),
        ]);

        let activity = ActivityLog::default();
        let mut supervisor = ProcessSupervisor::new(activity);
        supervisor
            .spawn_owned(ProcessKey::LocalServer, command, false)
            .await
            .expect("start local parent-liveness fixture");
        supervisor.stop(ProcessKey::LocalServer).await;

        assert!(
            marker.is_file(),
            "local generation must observe stdin EOF before forced tree cleanup"
        );
        let _ = std::fs::remove_file(marker);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quick_share_eof_stop_remains_green() {
        let marker = std::env::temp_dir().join(format!(
            "webcodex-desktop-quick-share-eof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let marker_arg = marker.to_string_lossy().into_owned();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "cat >/dev/null; printf eof > \"$1\"",
            "webcodex-quick-share-eof",
            marker_arg.as_str(),
        ]);

        let activity = ActivityLog::default();
        let mut supervisor = ProcessSupervisor::new(activity);
        supervisor
            .spawn_owned(ProcessKey::QuickShare, command, false)
            .await
            .expect("start Quick Share EOF fixture");
        supervisor.stop(ProcessKey::QuickShare).await;

        assert!(marker.is_file(), "Quick Share child must observe stdin EOF");
        let _ = std::fs::remove_file(marker);
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Desktop Windows real-process lane: waits on a real PowerShell stdin EOF"]
    async fn desktop_real_process_windows_regular_tunnel_stop_closes_stdin_for_canonical_graceful_shutdown(
    ) {
        let marker = std::env::temp_dir().join(format!(
            "webcodex-desktop-regular-tunnel-eof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let escaped_marker = marker.to_string_lossy().replace('\'', "''");
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(format!(
                "$marker = '{escaped_marker}'; Set-Content -LiteralPath $marker -Value 'ready'; $null = [Console]::In.ReadToEnd(); Set-Content -LiteralPath $marker -Value 'eof'"
            ));

        let activity = ActivityLog::default();
        let mut supervisor = ProcessSupervisor::new(activity);
        supervisor
            .spawn_owned(
                ProcessKey::RegularTunnel(crate::connection_id::TunnelProfileId::DEFAULT),
                command,
                false,
            )
            .await
            .expect("start EOF fixture");
        let ready_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if std::fs::read_to_string(&marker)
                .ok()
                .is_some_and(|value| value.trim() == "ready")
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < ready_deadline,
                "regular tunnel EOF fixture must become ready before stop"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        supervisor
            .stop(ProcessKey::RegularTunnel(
                crate::connection_id::TunnelProfileId::DEFAULT,
            ))
            .await;

        assert_eq!(
            std::fs::read_to_string(&marker)
                .expect("regular tunnel EOF fixture marker")
                .trim(),
            "eof",
            "regular tunnel child must observe stdin EOF before the graceful stop completes",
        );
        let _ = std::fs::remove_file(marker);
    }
}
