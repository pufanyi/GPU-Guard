use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub monitor: MonitorConfig,
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub tasks: TasksConfig,
}

#[derive(Debug, Deserialize)]
pub struct MonitorConfig {
    /// Check interval in seconds
    pub interval_secs: u64,
    /// GPU utilization threshold (0-100). Alert when below this.
    pub gpu_utilization_threshold: f64,
    /// Memory utilization threshold (0-100, optional).
    pub memory_utilization_threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: String,
    /// Chat IDs allowed to send commands. Defaults to [chat_id].
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

#[derive(Debug, Deserialize)]
pub struct TasksConfig {
    /// Max concurrent managed tasks
    pub max_tasks: usize,
    /// Lines kept per task in the log ring buffer
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
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
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
