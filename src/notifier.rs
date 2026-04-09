use reqwest::Client;

pub struct TelegramNotifier {
    client: Client,
    bot_token: String,
    chat_id: String,
}

impl TelegramNotifier {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            client: Client::new(),
            bot_token,
            chat_id,
        }
    }

    pub async fn send_message(&self, text: &str) -> anyhow::Result<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );

        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": &self.chat_id,
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
}
