use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub monitor: MonitorConfig,
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub tasks: TasksConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MonitorConfig {
    pub interval_secs: u64,
    pub gpu_utilization_threshold: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_utilization_threshold: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_chat_ids: Option<Vec<String>>,
}

impl TelegramConfig {
    pub fn allowed_ids(&self) -> HashSet<String> {
        match &self.allowed_chat_ids {
            Some(ids) => ids.iter().cloned().collect(),
            None => {
                let mut set = HashSet::new();
                set.insert(self.chat_id.clone());
                set
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksConfig {
    pub max_tasks: usize,
    pub log_ring_size: usize,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            max_tasks: 16,
            log_ring_size: 500,
        }
    }
}

impl Config {
    /// Load config from file, or run interactive setup if the file doesn't exist.
    pub fn load_or_setup(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&content)?;
            if config.telegram.bot_token.is_empty() || config.telegram.chat_id.is_empty() {
                println!("Config file exists but is incomplete. Re-running setup.\n");
                let config = Self::interactive_setup()?;
                config.save(path)?;
                Ok(config)
            } else {
                config.validate()?;
                Ok(config)
            }
        } else {
            println!("No config file found. Let's set things up.\n");
            let config = Self::interactive_setup()?;
            config.save(path)?;
            Ok(config)
        }
    }

    fn interactive_setup() -> anyhow::Result<Self> {
        println!("1. Create a bot via @BotFather on Telegram if you haven't already.");
        println!("2. Paste the bot token below.\n");

        let bot_token = prompt("Telegram bot token")?;
        if bot_token.is_empty() {
            anyhow::bail!("Bot token is required. Create one via @BotFather on Telegram.");
        }

        println!("\nNow send any message to your bot on Telegram...");
        println!("Waiting for your message...");

        let chat_id = wait_for_chat_id(&bot_token)?;

        let interval = prompt_default("Check interval in seconds", "600")?
            .parse::<u64>()
            .unwrap_or(600);

        let threshold = prompt_default("GPU utilization alert threshold % (alert when below)", "10")?
            .parse::<f64>()
            .unwrap_or(10.0);

        let config = Config {
            monitor: MonitorConfig {
                interval_secs: interval,
                gpu_utilization_threshold: threshold,
                memory_utilization_threshold: None,
            },
            telegram: TelegramConfig {
                bot_token,
                chat_id,
                allowed_chat_ids: None,
            },
            tasks: TasksConfig::default(),
        };

        config.validate()?;
        println!("\nConfig saved. You can edit it later in the config file.\n");
        Ok(config)
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.telegram.bot_token.is_empty() {
            anyhow::bail!("telegram.bot_token must be set");
        }
        if self.telegram.chat_id.is_empty() {
            anyhow::bail!("telegram.chat_id must be set");
        }
        if self.monitor.gpu_utilization_threshold < 0.0
            || self.monitor.gpu_utilization_threshold > 100.0
        {
            anyhow::bail!("gpu_utilization_threshold must be between 0 and 100");
        }
        Ok(())
    }
}

/// Poll getUpdates until we receive a message, then return the chat ID.
fn wait_for_chat_id(bot_token: &str) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct Resp {
        ok: bool,
        result: Option<Vec<Update>>,
    }
    #[derive(Deserialize)]
    struct Update {
        update_id: i64,
        message: Option<Msg>,
    }
    #[derive(Deserialize)]
    struct Msg {
        chat: Chat,
        from: Option<From>,
    }
    #[derive(Deserialize)]
    struct Chat {
        id: i64,
    }
    #[derive(Deserialize)]
    struct From {
        first_name: Option<String>,
        username: Option<String>,
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("https://api.telegram.org/bot{bot_token}/getUpdates");

    // First, flush old updates by getting the latest offset
    let resp: Resp = client
        .post(&url)
        .json(&serde_json::json!({"offset": 0, "timeout": 0}))
        .send()?
        .json()?;

    let mut offset: i64 = resp
        .result
        .as_ref()
        .and_then(|updates| updates.last())
        .map(|u| u.update_id + 1)
        .unwrap_or(0);

    // Now long-poll for a new message
    loop {
        let resp: Resp = client
            .post(&url)
            .json(&serde_json::json!({"offset": offset, "timeout": 30}))
            .timeout(Duration::from_secs(40))
            .send()?
            .json()?;

        if !resp.ok {
            anyhow::bail!("Telegram API error — check your bot token");
        }

        if let Some(updates) = resp.result {
            for update in &updates {
                offset = update.update_id + 1;
                if let Some(msg) = &update.message {
                    let chat_id = msg.chat.id.to_string();
                    let who = msg
                        .from
                        .as_ref()
                        .and_then(|f| f.first_name.as_deref().or(f.username.as_deref()))
                        .unwrap_or("unknown");
                    println!("Got message from {who} (chat_id: {chat_id})\n");
                    return Ok(chat_id);
                }
            }
        }
    }
}

fn prompt(label: &str) -> anyhow::Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_default(label: &str, default: &str) -> anyhow::Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.to_string())
    }
}
