mod bot;
mod cli;
mod config;
mod gpu;
mod monitor;
mod task_manager;
mod telegram;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{error, info, warn};

use task_manager::TaskManager;
use telegram::TelegramClient;

fn main() -> anyhow::Result<()> {
    // Config setup runs BEFORE tokio runtime so blocking reqwest works
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let config = config::Config::load_or_setup(&config_path)?;

    // Now start the async runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(config))
}

async fn async_main(config: config::Config) -> anyhow::Result<()> {
    // Log to file so it doesn't interfere with the interactive CLI
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gpu-guard");
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "gpu-guard.log");
    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false)
        .init();

    info!("Config loaded");

    let tg = Arc::new(TelegramClient::new(config.telegram.bot_token.clone()));
    let task_mgr = Arc::new(Mutex::new(TaskManager::new(
        config.tasks.max_tasks,
        config.tasks.log_ring_size,
    )));
    let allowed_ids = config.telegram.allowed_ids();
    let alert_chat_id = config.telegram.chat_id.clone();
    let monitor_interval = Duration::from_secs(config.monitor.interval_secs);
    let monitor_config = config.monitor;

    // Background: GPU monitor
    tokio::spawn({
        let tg = tg.clone();
        let chat_id = alert_chat_id.clone();
        async move {
            monitor_loop(&tg, &chat_id, &monitor_config, monitor_interval).await;
        }
    });

    // Background: Telegram bot
    tokio::spawn({
        let tg = tg.clone();
        let tm = task_mgr.clone();
        async move {
            bot_loop(&tg, &tm, &allowed_ids).await;
        }
    });

    // Background: task reaper
    tokio::spawn({
        let tm = task_mgr.clone();
        async move {
            reaper_loop(&tm).await;
        }
    });

    // Foreground: interactive CLI
    cli::run(&task_mgr).await?;

    Ok(())
}

async fn monitor_loop(
    tg: &TelegramClient,
    chat_id: &str,
    config: &config::MonitorConfig,
    interval: Duration,
) {
    loop {
        match gpu::query_gpus() {
            Ok(gpus) => {
                info!("Queried {} GPU(s)", gpus.len());
                if let Some(alert) = monitor::check_thresholds(&gpus, config) {
                    warn!("Threshold breached, sending alert");
                    if let Err(e) = tg.send_message(chat_id, &alert.message).await {
                        error!("Failed to send alert: {e}");
                    }
                }
            }
            Err(e) => error!("Failed to query GPUs: {e}"),
        }
        tokio::time::sleep(interval).await;
    }
}

async fn bot_loop(
    tg: &TelegramClient,
    task_mgr: &Mutex<TaskManager>,
    allowed_ids: &HashSet<String>,
) {
    let mut offset: i64 = 0;

    loop {
        match tg.get_updates(offset, 30).await {
            Ok(updates) => {
                for update in updates {
                    offset = update.update_id + 1;

                    let Some(message) = update.message else {
                        continue;
                    };
                    let Some(text) = message.text.as_deref() else {
                        continue;
                    };

                    let chat_id = message.chat.id.to_string();
                    if !allowed_ids.contains(&chat_id) {
                        info!("Ignoring message from unauthorized chat: {chat_id}");
                        continue;
                    }

                    let cmd = bot::parse_command(text);
                    let reply = bot::handle_command(cmd, task_mgr).await;

                    if let Err(e) = tg.send_message(&chat_id, &reply).await {
                        error!("Failed to send reply: {e}");
                    }
                }
            }
            Err(e) => {
                error!("getUpdates error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn reaper_loop(task_mgr: &Mutex<TaskManager>) {
    loop {
        {
            let mut tm = task_mgr.lock().await;
            tm.reap_finished().await;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
