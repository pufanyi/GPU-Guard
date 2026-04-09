mod config;
mod gpu;
mod monitor;
mod notifier;

use std::path::PathBuf;
use std::time::Duration;

use tracing::{error, info, warn};

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

    let notifier =
        notifier::TelegramNotifier::new(config.telegram.bot_token, config.telegram.chat_id);

    let interval = Duration::from_secs(config.monitor.interval_secs);

    loop {
        match gpu::query_gpus() {
            Ok(gpus) => {
                info!("Queried {} GPU(s)", gpus.len());
                for g in &gpus {
                    info!("  {g}");
                }

                if let Some(alert) = monitor::check_thresholds(&gpus, &config.monitor) {
                    warn!("Threshold breached, sending alert");
                    if let Err(e) = notifier.send_message(&alert.message).await {
                        error!("Failed to send Telegram message: {e}");
                    } else {
                        info!("Alert sent successfully");
                    }
                }
            }
            Err(e) => {
                error!("Failed to query GPUs: {e}");
            }
        }

        tokio::time::sleep(interval).await;
    }
}
