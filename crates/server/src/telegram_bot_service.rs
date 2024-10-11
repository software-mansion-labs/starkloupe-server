use reqwest::Client;
use starknet_api::core::ChainId;
use tracing::{debug, info, warn};

pub async fn send_telegram_notification(tx_id: &str, chain_id: &ChainId) -> Result<(), Box<dyn std::error::Error>> {
    let telegram_bot_api_key: String = std::env::var("TELEGRAM_BOT_API_KEY").unwrap_or(String::new());
    let telegram_bot_api_url: String = format!("https://api.telegram.org/bot{telegram_bot_api_key}/sendMessage");
    let telegram_walnut_notifications_chat_id: String = std::env::var("TELEGRAM_WALNUT_NOTIFICATIONS_CHAT_ID").unwrap_or(String::new());

    // skip if telegram_bot_api_key is not set
    if telegram_bot_api_key.is_empty() {
        debug!("telegram_bot_api_key is not set, skipping notification.");
        return Ok(());
    }

    if telegram_walnut_notifications_chat_id.is_empty() {
        debug!("telegram_walnut_notifications_chat_id is not set, skipping notification.");
        return Ok(());
    }

    let client = Client::new();

    let payload = serde_json::json!({
        "chat_id": telegram_walnut_notifications_chat_id,
        "text": format!("New transaction [{chain_id}]: https://app.walnut.dev/transactions?chainId={chain_id}&txHash={tx_id}?skip_tracking=true"),
    });

    let res = client.post(telegram_bot_api_url)
        .json(&payload)
        .send()
        .await;

    match res {
        Ok(res) => {
            if !res.status().is_success() {
                info!("Failed to send telegram notification for tx_id: {}. Error: {}", tx_id, res.text().await.unwrap_or_else(|_| "Unknown error".to_string()));
            }
        }
        Err(e) => {
            warn!("Failed to send telegram notification for tx_id: {}. Error: {}", tx_id, e);
        }
    }
    Ok(())
}