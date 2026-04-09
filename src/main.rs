mod bot;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let config = config::Config::load(&config_path)?;
    info!("Loaded config from {}", config_path.display());
    info!(
        "Monitoring every {}s, GPU threshold: {:.1}%",
        config.monitor.interval_secs, config.monitor.gpu_utilization_threshold
    );

    let tg = Arc::new(TelegramClient::new(config.telegram.bot_token.clone()));
    let task_mgr = Arc::new(Mutex::new(TaskManager::new(
        config.tasks.max_tasks,
        config.tasks.log_ring_size,
    )));
    let allowed_ids = config.telegram.allowed_ids();
    let alert_chat_id = config.telegram.chat_id.clone();
    let monitor_interval = Duration::from_secs(config.monitor.interval_secs);
    let monitor_config = config.monitor;

    let monitor_handle = tokio::spawn({
        let tg = tg.clone();
        let chat_id = alert_chat_id.clone();
        async move {
            monitor_loop(&tg, &chat_id, &monitor_config, monitor_interval).await;
        }
    });

    let bot_handle = tokio::spawn({
        let tg = tg.clone();
        let tm = task_mgr.clone();
        async move {
            bot_loop(&tg, &tm, &allowed_ids).await;
        }
    });

    let reaper_handle = tokio::spawn({
        let tm = task_mgr.clone();
        async move {
            reaper_loop(&tm).await;
        }
    });

    tokio::select! {
        r = monitor_handle => { error!("Monitor loop exited: {r:?}"); }
        r = bot_handle => { error!("Bot loop exited: {r:?}"); }
        r = reaper_handle => { error!("Reaper loop exited: {r:?}"); }
    }

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
                for g in &gpus {
                    info!("  {g}");
                }
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
