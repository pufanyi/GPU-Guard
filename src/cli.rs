use crate::gpu;
use crate::task_manager::{TaskId, TaskManager, TaskStatus};

use reedline::{
    default_emacs_keybindings, ColumnarMenu, Completer, DefaultHinter, EditCommand, Emacs,
    FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder, Prompt, PromptEditMode,
    PromptHistorySearch, PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu, Signal,
    Span, Suggestion,
};
use std::borrow::Cow;
use tokio::sync::Mutex;

// ── Prompt ──────────────────────────────────────────────────────────────

struct GpuGuardPrompt {
    running_tasks: usize,
}

impl Prompt for GpuGuardPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        if self.running_tasks > 0 {
            Cow::Owned(format!("gpu-guard [{}]", self.running_tasks))
        } else {
            Cow::Borrowed("gpu-guard")
        }
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(" \x1b[32m❯\x1b[0m ")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("  ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "(failed) ",
        };
        Cow::Owned(format!("{prefix}search: "))
    }
}

// ── Completer ───────────────────────────────────────────────────────────

const COMMANDS: &[&str] = &[
    "submit", "run", "start", "tasks", "ls", "ps", "logs", "kill", "stop", "status", "gpu",
    "help", "quit", "exit",
];

#[derive(Clone)]
struct CmdCompleter;

impl Completer for CmdCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let prefix = &line[..pos];
        // Only complete the first token
        if prefix.contains(' ') {
            return vec![];
        }
        COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|cmd| Suggestion {
                value: cmd.to_string(),
                description: None,
                style: None,
                extra: None,
                span: Span::new(0, pos),
                append_whitespace: true,
            })
            .collect()
    }
}

// ── Command parsing ─────────────────────────────────────────────────────

enum CliCommand {
    Submit { command: String },
    Tasks,
    Logs { task_id: TaskId, lines: usize },
    Kill { task_id: TaskId },
    Status,
    Help,
    Quit,
    Empty,
    Unknown(String),
}

fn parse_input(line: &str) -> CliCommand {
    let line = line.trim();
    if line.is_empty() {
        return CliCommand::Empty;
    }

    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

    match cmd {
        "submit" | "run" | "start" => {
            if rest.is_empty() {
                CliCommand::Unknown("Usage: submit <command>".to_string())
            } else {
                CliCommand::Submit {
                    command: rest.to_string(),
                }
            }
        }
        "tasks" | "ls" | "ps" => CliCommand::Tasks,
        "logs" | "log" => {
            let args: Vec<&str> = rest.split_whitespace().collect();
            let task_id = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            let lines = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(50);
            CliCommand::Logs { task_id, lines }
        }
        "kill" | "stop" => {
            let task_id = rest.split_whitespace().next().and_then(|s| s.parse().ok());
            match task_id {
                Some(id) => CliCommand::Kill { task_id: id },
                None => CliCommand::Unknown("Usage: kill <task_id>".to_string()),
            }
        }
        "status" | "gpu" => CliCommand::Status,
        "help" | "?" => CliCommand::Help,
        "quit" | "exit" | "q" => CliCommand::Quit,
        _ => CliCommand::Unknown(format!("Unknown command: {cmd}. Type 'help' for usage.")),
    }
}

// ── Main REPL ───────────────────────────────────────────────────────────

pub async fn run(task_manager: &Mutex<TaskManager>) -> anyhow::Result<()> {
    print_banner();

    let history_path = data_dir().join("history.txt");
    let history = FileBackedHistory::with_file(1000, history_path)?;

    let completer = Box::new(CmdCompleter);
    let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));

    let hinter = Box::new(DefaultHinter::default().with_style(nu_ansi_term::Style::new().dimmed()));

    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('d'),
        ReedlineEvent::Edit(vec![EditCommand::Clear]),
    );
    let edit_mode = Box::new(Emacs::new(keybindings));

    let mut line_editor = Reedline::create()
        .with_history(Box::new(history))
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_hinter(hinter)
        .with_edit_mode(edit_mode);

    loop {
        let running = count_running(task_manager).await;
        let prompt = GpuGuardPrompt {
            running_tasks: running,
        };

        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => match parse_input(&line) {
                CliCommand::Submit { command } => cmd_submit(task_manager, &command).await,
                CliCommand::Tasks => cmd_tasks(task_manager).await,
                CliCommand::Logs { task_id, lines } => {
                    cmd_logs(task_manager, task_id, lines).await
                }
                CliCommand::Kill { task_id } => cmd_kill(task_manager, task_id).await,
                CliCommand::Status => cmd_status(),
                CliCommand::Help => cmd_help(),
                CliCommand::Quit => {
                    println!("Goodbye.");
                    break;
                }
                CliCommand::Empty => {}
                CliCommand::Unknown(msg) => println!("{msg}"),
            },
            Ok(Signal::CtrlC) => {
                println!("^C (type 'quit' to exit)");
            }
            Ok(Signal::CtrlD) => {
                println!("Goodbye.");
                break;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                break;
            }
        }
    }

    Ok(())
}

async fn count_running(task_manager: &Mutex<TaskManager>) -> usize {
    let tm = task_manager.lock().await;
    let tasks = tm.list_tasks().await;
    tasks
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Running))
        .count()
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn print_banner() {
    println!(
        "\x1b[1;36mGPU Guard\x1b[0m v{}",
        env!("CARGO_PKG_VERSION")
    );
    println!("Type \x1b[1mhelp\x1b[0m for available commands.\n");
}

fn data_dir() -> std::path::PathBuf {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("gpu-guard");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cmd_status() {
    match gpu::query_gpus() {
        Ok(gpus) if gpus.is_empty() => println!("No GPUs detected."),
        Ok(gpus) => {
            println!(
                "\x1b[1m{:<6} {:<30} {:>8} {:>18}\x1b[0m",
                "GPU", "Name", "Util%", "Memory"
            );
            println!("{}", "-".repeat(66));
            for g in &gpus {
                let util_color = if g.gpu_utilization > 80.0 {
                    "\x1b[31m" // red
                } else if g.gpu_utilization > 30.0 {
                    "\x1b[33m" // yellow
                } else {
                    "\x1b[32m" // green
                };
                println!(
                    "{:<6} {:<30} {}{:>7.1}%\x1b[0m {:>8.0}/{:.0} MiB ({:.1}%)",
                    g.index,
                    g.name,
                    util_color,
                    g.gpu_utilization,
                    g.memory_used,
                    g.memory_total,
                    g.memory_utilization(),
                );
            }
        }
        Err(e) => println!("\x1b[31mFailed to query GPUs: {e}\x1b[0m"),
    }
}

async fn cmd_submit(task_manager: &Mutex<TaskManager>, command: &str) {
    let mut tm = task_manager.lock().await;
    match tm.start_task(command) {
        Ok(id) => println!("\x1b[32mTask #{id} started:\x1b[0m {command}"),
        Err(e) => println!("\x1b[31mFailed: {e}\x1b[0m"),
    }
}

async fn cmd_tasks(task_manager: &Mutex<TaskManager>) {
    let tm = task_manager.lock().await;
    let tasks = tm.list_tasks().await;

    if tasks.is_empty() {
        println!("No tasks.");
        return;
    }

    println!(
        "\x1b[1m{:<6} {:<12} {:>10}  {}\x1b[0m",
        "ID", "Status", "Elapsed", "Command"
    );
    println!("{}", "-".repeat(70));
    for t in &tasks {
        let elapsed = format_duration(t.elapsed_secs);
        let cmd_display = if t.command.len() > 40 {
            format!("{}...", &t.command[..37])
        } else {
            t.command.clone()
        };
        let status_color = match t.status {
            TaskStatus::Running => "\x1b[32m",
            TaskStatus::Exited(Some(0)) => "\x1b[34m",
            TaskStatus::Exited(_) => "\x1b[31m",
            TaskStatus::Killed => "\x1b[33m",
        };
        println!(
            "#{:<5} {}{:<12}\x1b[0m {:>10}  {}",
            t.id,
            status_color,
            t.status.to_string(),
            elapsed,
            cmd_display,
        );
    }
}

async fn cmd_logs(task_manager: &Mutex<TaskManager>, task_id: TaskId, lines: usize) {
    if task_id == 0 {
        println!("Usage: logs <task_id> [lines]");
        return;
    }
    let tm = task_manager.lock().await;
    match tm.get_logs(task_id, lines).await {
        Ok(logs) if logs.is_empty() => println!("Task #{task_id}: no logs yet."),
        Ok(logs) => {
            println!(
                "\x1b[1m--- Task #{task_id} (last {} lines) ---\x1b[0m",
                logs.len()
            );
            for line in &logs {
                println!("{line}");
            }
            println!("\x1b[1m--- end ---\x1b[0m");
        }
        Err(e) => println!("\x1b[31mError: {e}\x1b[0m"),
    }
}

async fn cmd_kill(task_manager: &Mutex<TaskManager>, task_id: TaskId) {
    let mut tm = task_manager.lock().await;
    match tm.kill_task(task_id).await {
        Ok(()) => println!("\x1b[33mTask #{task_id} killed.\x1b[0m"),
        Err(e) => println!("\x1b[31mFailed: {e}\x1b[0m"),
    }
}

fn cmd_help() {
    println!(
        "\
\x1b[1mCommands:\x1b[0m
  \x1b[36msubmit\x1b[0m <command>      Run a shell command as a managed task
  \x1b[36mtasks\x1b[0m                 List all tasks
  \x1b[36mlogs\x1b[0m <id> [lines]     Show task logs (default 50 lines)
  \x1b[36mkill\x1b[0m <id>             Kill a running task
  \x1b[36mstatus\x1b[0m                Show GPU utilization
  \x1b[36mhelp\x1b[0m                  Show this message
  \x1b[36mquit\x1b[0m                  Exit GPU Guard

\x1b[2mAliases: run/start = submit, ls/ps = tasks, stop = kill, gpu = status\x1b[0m"
    );
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
