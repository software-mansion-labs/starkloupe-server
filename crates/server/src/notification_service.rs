use reqwest::Client;
use simulate::SimulationArgs;
use starknet_api::core::ChainId;
use tracing::{debug, error};
use urlencoding::encode;
use walnut_shared::{chain_id_to_url_format, walnut_app_url};

pub async fn send_notification_tx_id(
    tx_id: &str,
    chain_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let message = format!(
        "New transaction [{chain_id}]: {}/transactions?chainId={chain_id}&txHash={tx_id}&skip_tracking=true",
        walnut_app_url()
    );

    let _ = send_slack_notification(&message).await;
    let _ = send_grafana_annotation(tx_id, &message, chain_id, "transaction").await;

    Ok(())
}

pub async fn send_notification_custom_rpc(
    tx_id: &str,
    rpc_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let message = format!(
        "New transaction [custom RPC]: {}/transactions?rpcUrl={}&txHash={}&skip_tracking=true",
        walnut_app_url(),
        encode(rpc_url),
        tx_id
    );
    let chain_id = "custom_rpc";

    let _ = send_slack_notification(&message).await;
    let _ = send_grafana_annotation(tx_id, &message, chain_id, "transaction").await;

    Ok(())
}

pub async fn send_notification_calldata(
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

    query_params.push("skip_tracking=true".to_string());

    let url = format!(
        "{}/simulations?{}",
        walnut_app_url(),
        query_params.join("&")
    );

    let message = format!("New transaction [simulation]: {}", url);
    let chain_id_str = chain_id_to_url_format(&simulation_args.chain_id);

    let _ = send_slack_notification(&message).await;
    let _ = send_grafana_annotation(&calldata, &message, &chain_id_str, "simulation").await;

    Ok(())
}

async fn send_slack_notification(message: &str) -> Result<(), Box<dyn std::error::Error>> {
    let slack_webhook_url = std::env::var("SLACK_WEBHOOK_URL")?;

    if slack_webhook_url.is_empty() {
        error!("Slack webhook URL is not set, skipping notification.");
        return Ok(());
    }

    let client = Client::new();

    let payload = serde_json::json!({
        "text": message,
    });

    let res = client.post(&slack_webhook_url).json(&payload).send().await;

    match res {
        Ok(res) => {
            if !res.status().is_success() {
                let status = res.status();
                let error_text = res
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                error!(
                    "Failed to send slack notification. HTTP Status: {}. Message: {}. Error: {}",
                    status, message, error_text
                );
            }
        }
        Err(e) => {
            error!(
                "Failed to send slack notification. Message: {}. Error: {}",
                message, e
            );
        }
    }
    Ok(())
}

async fn send_grafana_annotation(
    tx_id: &str,
    message: &str,
    chain_id: &str,
    notification_type: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let grafana_url = std::env::var("GRAFANA_URL")?;
    let grafana_api_key = std::env::var("GRAFANA_API_KEY")?;

    if grafana_url.is_empty() || grafana_api_key.is_empty() {
        error!("Grafana URL or API key is not set, skipping annotation.");
        return Ok(());
    }

    let client = Client::new();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let payload = serde_json::json!({
        "text": message,
        "tags": [notification_type, chain_id, tx_id],
        "time": now_ms,
    });

    let res = client
        .post(format!("{grafana_url}/api/annotations"))
        .header("Authorization", format!("Bearer {grafana_api_key}"))
        .json(&payload)
        .send()
        .await;

    match res {
        Ok(res) if !res.status().is_success() => {
            let status = res.status();
            let error_text = res
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!(
                "Failed to send Grafana annotation. Status: {}. Error: {}",
                status, error_text
            );
        }
        Err(e) => {
            error!("Failed to send Grafana annotation. Error: {}", e);
        }
        _ => {
            debug!("Grafana annotation added successfully");
        }
    }
    Ok(())
}
