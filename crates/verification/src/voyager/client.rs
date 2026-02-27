use super::types::{VoyagerConfig, VoyagerSourceResponse};
use anyhow::Result;
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// HTTP client for Voyager API
#[derive(Clone)]
pub struct VoyagerClient {
    client: Client,
    config: VoyagerConfig,
}

impl VoyagerClient {
    /// Create a new Voyager client with the given configuration
    pub fn new(config: VoyagerConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self { client, config })
    }

    /// Check if the client is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Fetch source code for a class from Voyager API
    ///
    /// Returns:
    /// - Ok(Some(response)) if the class is found and verified
    /// - Ok(None) if the class is not found (404)
    /// - Err if there's an API error or network issue
    pub async fn fetch_source_code(
        &self,
        class_hash: &str,
    ) -> Result<Option<VoyagerSourceResponse>> {
        if !self.config.enabled {
            return Ok(None);
        }

        // Skip specific addresses that are known to not be on Voyager
        if class_hash == "0x0000000000000000000000000000000000000000000000000000000000000117" {
            return Ok(None);
        }

        let url = format!("{}/classes/{}/source", self.config.base_url, class_hash);

        let response = self
            .client
            .get(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.config.api_key)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status();

                if status.is_success() {
                    match resp.json::<VoyagerSourceResponse>().await {
                        Ok(source_response) => {
                            debug!(
                                "Successfully fetched source code from Voyager for class {} ({})",
                                class_hash, source_response.verified_name
                            );
                            Ok(Some(source_response))
                        }
                        Err(e) => {
                            error!(
                                "Failed to parse Voyager response for {}: {:?}",
                                class_hash, e
                            );
                            Err(anyhow::anyhow!("Failed to parse Voyager response: {}", e))
                        }
                    }
                } else if status == reqwest::StatusCode::NOT_FOUND {
                    debug!("Class {} not found on Voyager (404)", class_hash);
                    Ok(None)
                } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    Err(anyhow::anyhow!("Voyager API rate limited"))
                } else {
                    let error_text = resp.text().await.unwrap_or_default();
                    Err(anyhow::anyhow!(
                        "Voyager API error: status={}, body={}",
                        status,
                        error_text
                    ))
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    warn!("Voyager API timeout for class {}", class_hash);
                    Err(anyhow::anyhow!("Voyager API timeout"))
                } else if e.is_connect() {
                    warn!("Failed to connect to Voyager API: {:?}", e);
                    Err(anyhow::anyhow!("Failed to connect to Voyager API"))
                } else {
                    error!("Voyager API network error for {}: {:?}", class_hash, e);
                    Err(anyhow::anyhow!("Voyager API network error: {}", e))
                }
            }
        }
    }
}
