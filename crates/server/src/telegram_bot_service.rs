use reqwest::Client;
use simulate::SimulationArgs;
use starknet_api::core::ChainId;
use tracing::error;
use urlencoding::encode;
use walnut_shared::chain_id_to_url_format;

pub async fn send_telegram_notification_tx_id(
    tx_id: &str,
    chain_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let message = format!("New transaction [{chain_id}]: https://app.walnut.dev/transactions?chainId={chain_id}&txHash={tx_id}&skip_tracking=true");
    send_telegram_notification(message.as_str()).await
}

pub async fn send_telegram_notification_custom_rpc(
    tx_id: &str,
    rpc_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let message = format!("New transaction [custom RPC]: https://app.walnut.dev/transactions?rpcUrl={}&txHash={}&skip_tracking=true", encode(rpc_url), tx_id);
    send_telegram_notification(message.as_str()).await
}

pub async fn send_telegram_notification_calldata(
    simulation_args: &SimulationArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let calldata_string = simulation_args
        .calldata
        .0
        .iter()
        .map(|x| format!("{:?}", x))
        .collect::<Vec<String>>()
        .join(",");
    let calldata = encode(&calldata_string);

    let mut query_params = vec![
        format!("senderAddress={}", simulation_args.sender_address),
        format!("calldata={}", calldata),
        format!(
            "transactionVersion={}",
            simulation_args.transaction_version.to_string()
        ),
        format!(
            "chainId={}",
            chain_id_to_url_format(&simulation_args.chain_id)
        ),
    ];

    if let Some(block_number) = simulation_args.block_number {
        query_params.push(format!("blockNumber={}", block_number));
    }

    if !matches!(
        simulation_args.chain_id,
        ChainId::Mainnet | ChainId::Sepolia
    ) {
        query_params.push(format!(
            "rpcUrl={}",
            encode(simulation_args.rpc_url.as_str())
        ));
    }

    // Add `skip_tracking=true` at the end
    query_params.push("skip_tracking=true".to_string());

    let url = format!(
        "https://app.walnut.dev/simulations?{}",
        query_params.join("&")
    );

    let message = format!("New transaction [simulation]: {}", url);
    send_telegram_notification(message.as_str()).await
}

async fn send_telegram_notification(message: &str) -> Result<(), Box<dyn std::error::Error>> {
    let telegram_bot_api_key = std::env::var("TELEGRAM_BOT_API_KEY")?;
    let telegram_walnut_notifications_chat_id =
        std::env::var("TELEGRAM_WALNUT_NOTIFICATIONS_CHAT_ID")?;

    if telegram_bot_api_key.is_empty() || telegram_walnut_notifications_chat_id.is_empty() {
        error!("Telegram API key or chat ID is not set, skipping notification.");
        return Ok(());
    }
    let telegram_bot_api_url: String =
        format!("https://api.telegram.org/bot{telegram_bot_api_key}/sendMessage");

    let client = Client::new();

    // Split the message into chunks of at most 4096 characters
    let messages = message
        .chars()
        .collect::<Vec<_>>()
        .chunks(4096)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<String>>();

    for message in messages.iter() {
        let payload = serde_json::json!({
            "chat_id": telegram_walnut_notifications_chat_id,
            "text": message,
        });

        let res = client
            .post(&telegram_bot_api_url)
            .json(&payload)
            .send()
            .await;

        match res {
            Ok(res) => {
                if !res.status().is_success() {
                    let status = res.status();
                    let error_text = res
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    error!(
                        "Failed to send telegram notification. HTTP Status: {}. Message: {}. Error: {}",
                        status, message, error_text
                    );
                }
            }
            Err(e) => {
                error!(
                    "Failed to send telegram notification. Message: {}. Error: {}",
                    message, e
                );
            }
        }
    }
    Ok(())
}
