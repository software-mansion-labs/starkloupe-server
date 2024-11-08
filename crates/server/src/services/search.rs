use std::str::FromStr;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use starknet::core::types::Felt;
use starknet_api::core::ChainId;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::Provider;
use url::Url;
use utoipa::ToSchema;
use walnut_shared::{
    chain_id_to_readable_string, create_rpc_client, create_rpc_client_from_url,
    felt_to_field_element,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct Data {
    pub source: Source,
    pub hash: String,
}

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub enum Source {
    ChainId(String),
    RpcUrl(String),
}

#[derive(Debug)]
pub enum SourceType {
    ChainId(ChainId),
    RpcUrl(Url),
}

pub fn sources_from_rpc_urls(urls: Option<&str>) -> Result<Vec<SourceType>> {
    let mut sources = vec![
        SourceType::ChainId(ChainId::Mainnet),
        SourceType::ChainId(ChainId::Sepolia),
    ];

    if let Some(urls) = urls {
        for url in urls.split(',') {
            let trimmed_url = url.trim();
            match Url::parse(trimmed_url) {
                Ok(parsed_url) => {
                    let host = parsed_url.host_str().unwrap_or("");
                    if host == "localhost" || host == "127.0.0.1" || host == "0.0.0.0" {
                        return Err(anyhow!(
                            "Invalid URL '{}': localhost or local IP addresses are not allowed",
                            trimmed_url
                        ));
                    }
                    sources.push(SourceType::RpcUrl(parsed_url));
                }
                Err(err) => {
                    return Err(anyhow!("Invalid URL '{}': {}", trimmed_url, err));
                }
            }
        }
    }
    Ok(sources)
}

pub async fn check_class(hash: &str, sources: &[SourceType]) -> Option<Vec<Data>> {
    let mut classes = Vec::new();
    for source in sources {
        match source {
            SourceType::ChainId(chain_id) => {
                let provider_client = create_rpc_client(chain_id);
                match provider_client
                    .get_class(
                        starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                        felt_to_field_element(Felt::from_str(hash).ok()?),
                    )
                    .await
                {
                    Ok(_) => {
                        classes.push(Data {
                            source: Source::ChainId(chain_id_to_readable_string(chain_id)),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
            SourceType::RpcUrl(url) => {
                let client = create_rpc_client_from_url(url.clone());
                match client
                    .get_class(
                        starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                        felt_to_field_element(Felt::from_str(hash).ok()?),
                    )
                    .await
                {
                    Ok(_) => {
                        classes.push(Data {
                            source: Source::RpcUrl(url.to_string()),
                            hash: hash.to_string(),
                        });
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    if classes.is_empty() {
        return None;
    }
    Some(classes)
}
