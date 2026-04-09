use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{error, info};

pub type TaskId = u32;

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Running,
    Exited(Option<i32>),
    Killed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Running => write!(f, "Running"),
            TaskStatus::Exited(Some(code)) => write!(f, "Exited({code})"),
            TaskStatus::Exited(None) => write!(f, "Exited(?)"),
            TaskStatus::Killed => write!(f, "Killed"),
        }
    }
}

pub struct ManagedTask {
    pub id: TaskId,
    pub command: String,
    pub started_at: Instant,
    pub status: Arc<Mutex<TaskStatus>>,
    pub logs: Arc<Mutex<VecDeque<String>>>,
    child: Option<Child>,
}

pub struct TaskSummary {
    pub id: TaskId,
    pub command: String,
    pub status: TaskStatus,
    pub elapsed_secs: u64,
}

pub struct TaskManager {
    tasks: HashMap<TaskId, ManagedTask>,
    next_id: TaskId,
    max_tasks: usize,
    log_ring_size: usize,
}

impl TaskManager {
    pub fn new(max_tasks: usize, log_ring_size: usize) -> Self {
        Self {
            tasks: HashMap::new(),
            next_id: 1,
            max_tasks,
            log_ring_size,
        }
    }

    pub fn start_task(&mut self, command: &str) -> anyhow::Result<TaskId> {
        let running_count = self
            .tasks
            .values()
            .filter(|t| matches!(*t.status.blocking_lock(), TaskStatus::Running))
            .count();

        if running_count >= self.max_tasks {
            anyhow::bail!("Max concurrent tasks ({}) reached", self.max_tasks);
        }

        let mut child = Command::new("sh")
            .args(["-c", command])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let id = self.next_id;
        self.next_id += 1;

        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(self.log_ring_size)));
        let status = Arc::new(Mutex::new(TaskStatus::Running));
        let ring_size = self.log_ring_size;

        // Spawn readers for stdout and stderr
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(stdout) = stdout {
            let logs_clone = logs.clone();
            tokio::spawn(async move {
                read_lines_into_ring(stdout, logs_clone, ring_size, "stdout").await;
            });
        }
        if let Some(stderr) = stderr {
            let logs_clone = logs.clone();
            tokio::spawn(async move {
                read_lines_into_ring(stderr, logs_clone, ring_size, "stderr").await;
            });
        }

        info!("Started task {id}: {command}");

        self.tasks.insert(
            id,
            ManagedTask {
                id,
                command: command.to_string(),
                started_at: Instant::now(),
                status,
                logs,
                child: Some(child),
            },
        );

        Ok(id)
    }

    pub async fn kill_task(&mut self, id: TaskId) -> anyhow::Result<()> {
        let task = self
            .tasks
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Task {id} not found"))?;

        if let Some(ref mut child) = task.child {
            child.kill().await?;
            task.child = None;
        }
        *task.status.lock().await = TaskStatus::Killed;
        info!("Killed task {id}");
        Ok(())
    }

    pub async fn list_tasks(&self) -> Vec<TaskSummary> {
        let mut summaries = Vec::new();
        for task in self.tasks.values() {
            let status = task.status.lock().await.clone();
            summaries.push(TaskSummary {
                id: task.id,
                command: task.command.clone(),
                status,
                elapsed_secs: task.started_at.elapsed().as_secs(),
            });
        }
        summaries.sort_by_key(|s| s.id);
        summaries
    }

    pub async fn get_logs(&self, id: TaskId, n: usize) -> anyhow::Result<Vec<String>> {
        let task = self
            .tasks
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Task {id} not found"))?;

        let logs = task.logs.lock().await;
        let start = logs.len().saturating_sub(n);
        Ok(logs.iter().skip(start).cloned().collect())
    }

    pub async fn reap_finished(&mut self) {
        for task in self.tasks.values_mut() {
            let is_running = matches!(*task.status.lock().await, TaskStatus::Running);
            if !is_running {
                continue;
            }
            if let Some(ref mut child) = task.child {
                match child.try_wait() {
                    Ok(Some(exit_status)) => {
                        let code = exit_status.code();
                        *task.status.lock().await = TaskStatus::Exited(code);
                        task.child = None;
                        info!("Task {} exited with {:?}", task.id, code);
                    }
                    Ok(None) => {} // still running
                    Err(e) => {
                        error!("Error checking task {}: {e}", task.id);
                    }
                }
            }
        }
    }
}

async fn read_lines_into_ring<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    logs: Arc<Mutex<VecDeque<String>>>,
    max_lines: usize,
    _label: &str,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut buf = logs.lock().await;
        if buf.len() >= max_lines {
            buf.pop_front();
        }
        buf.push_back(line);
    }
}
