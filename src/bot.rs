use crate::gpu;
use crate::task_manager::{TaskId, TaskManager};
use tokio::sync::Mutex;

pub enum BotCommand {
    Status,
    Tasks,
    Logs { task_id: TaskId, lines: usize },
    Start { command: String },
    Kill { task_id: TaskId },
    Help,
    Unknown(String),
}

pub fn parse_command(text: &str) -> BotCommand {
    let text = text.trim();

    // Strip the @bot_name suffix from commands (e.g., /status@my_bot)
    let first_token = text.split_whitespace().next().unwrap_or("");
    let cmd = first_token.split('@').next().unwrap_or(first_token);
    let rest = text[first_token.len()..].trim();

    match cmd {
        "/status" => BotCommand::Status,
        "/tasks" => BotCommand::Tasks,
        "/logs" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let task_id = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            let lines = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(50);
            BotCommand::Logs { task_id, lines }
        }
        "/start" => {
            if rest.is_empty() {
                BotCommand::Unknown("Usage: /start <command>".to_string())
            } else {
                BotCommand::Start {
                    command: rest.to_string(),
                }
            }
        }
        "/kill" => {
            let task_id = rest.split_whitespace().next().and_then(|s| s.parse().ok());
            match task_id {
                Some(id) => BotCommand::Kill { task_id: id },
                None => BotCommand::Unknown("Usage: /kill <task_id>".to_string()),
            }
        }
        "/help" => BotCommand::Help,
        _ => BotCommand::Unknown(format!("Unknown command: {cmd}\nSend /help for usage.")),
    }
}

pub async fn handle_command(cmd: BotCommand, task_manager: &Mutex<TaskManager>) -> String {
    match cmd {
        BotCommand::Status => handle_status(),
        BotCommand::Tasks => handle_tasks(task_manager).await,
        BotCommand::Logs { task_id, lines } => handle_logs(task_manager, task_id, lines).await,
        BotCommand::Start { command } => handle_start(task_manager, &command).await,
        BotCommand::Kill { task_id } => handle_kill(task_manager, task_id).await,
        BotCommand::Help => handle_help(),
        BotCommand::Unknown(msg) => msg,
    }
}

fn handle_status() -> String {
    match gpu::query_gpus() {
        Ok(gpus) if gpus.is_empty() => "No GPUs detected.".to_string(),
        Ok(gpus) => {
            let mut msg = String::from("<b>GPU Status</b>\n\n");
            for g in &gpus {
                msg.push_str(&format!(
                    "GPU {} (<b>{}</b>)\n  Utilization: {:.1}%\n  Memory: {:.0}/{:.0} MiB ({:.1}%)\n\n",
                    g.index,
                    g.name,
                    g.gpu_utilization,
                    g.memory_used,
                    g.memory_total,
                    g.memory_utilization(),
                ));
            }
            msg
        }
        Err(e) => format!("Failed to query GPUs: {e}"),
    }
}

async fn handle_tasks(task_manager: &Mutex<TaskManager>) -> String {
    let tm = task_manager.lock().await;
    let tasks = tm.list_tasks().await;

    if tasks.is_empty() {
        return "No tasks.".to_string();
    }

    let mut msg = String::from("<b>Tasks</b>\n\n");
    for t in &tasks {
        let elapsed = format_duration(t.elapsed_secs);
        let cmd_display = if t.command.len() > 60 {
            format!("{}...", &t.command[..57])
        } else {
            t.command.clone()
        };
        msg.push_str(&format!(
            "<b>#{}</b> [{}] {}\n  <code>{}</code>\n\n",
            t.id, t.status, elapsed, cmd_display,
        ));
    }
    msg
}

async fn handle_logs(task_manager: &Mutex<TaskManager>, task_id: TaskId, lines: usize) -> String {
    let tm = task_manager.lock().await;
    match tm.get_logs(task_id, lines).await {
        Ok(logs) if logs.is_empty() => format!("Task #{task_id}: no logs yet."),
        Ok(logs) => {
            let mut msg = format!("<b>Logs for task #{task_id}</b> (last {} lines)\n\n<pre>", logs.len());
            for line in &logs {
                // Escape HTML in log lines
                let escaped = line
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                msg.push_str(&escaped);
                msg.push('\n');
            }
            msg.push_str("</pre>");
            // Telegram message limit is 4096 chars
            if msg.len() > 4000 {
                msg.truncate(4000);
                msg.push_str("\n... (truncated)");
            }
            msg
        }
        Err(e) => format!("Error: {e}"),
    }
}

async fn handle_start(task_manager: &Mutex<TaskManager>, command: &str) -> String {
    let mut tm = task_manager.lock().await;
    match tm.start_task(command) {
        Ok(id) => format!("Started task <b>#{id}</b>\n<code>{command}</code>"),
        Err(e) => format!("Failed to start task: {e}"),
    }
}

async fn handle_kill(task_manager: &Mutex<TaskManager>, task_id: TaskId) -> String {
    let mut tm = task_manager.lock().await;
    match tm.kill_task(task_id).await {
        Ok(()) => format!("Killed task <b>#{task_id}</b>"),
        Err(e) => format!("Failed to kill task #{task_id}: {e}"),
    }
}

fn handle_help() -> String {
    "\
<b>GPU Guard Bot</b>

/status — Show GPU utilization
/tasks — List managed tasks
/logs &lt;id&gt; [lines] — Show task logs (default 50)
/start &lt;command&gt; — Run a shell command
/kill &lt;id&gt; — Kill a task
/help — This message"
        .to_string()
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}
