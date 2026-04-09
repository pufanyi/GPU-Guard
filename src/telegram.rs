use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TgResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    #[allow(dead_code)]
    pub message_id: i64,
    pub chat: Chat,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
}

pub struct TelegramClient {
    client: Client,
    bot_token: String,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Self {
        Self {
            client: Client::new(),
            bot_token,
        }
    }

    pub async fn send_message(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );

        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": text,
                "parse_mode": "HTML",
            }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Telegram API error: {body}");
        }

        Ok(())
    }

    /// Long-poll for updates. `timeout` is in seconds.
    pub async fn get_updates(
        &self,
        offset: i64,
        timeout: u64,
    ) -> anyhow::Result<Vec<Update>> {
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates",
            self.bot_token
        );

        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "offset": offset,
                "timeout": timeout,
                "allowed_updates": ["message"],
            }))
            .timeout(std::time::Duration::from_secs(timeout + 10))
            .send()
            .await?;

        let body: TgResponse<Vec<Update>> = resp.json().await?;

        if !body.ok {
            anyhow::bail!(
                "getUpdates failed: {}",
                body.description.unwrap_or_default()
            );
        }

        Ok(body.result.unwrap_or_default())
    }
}
