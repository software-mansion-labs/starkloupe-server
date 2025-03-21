use anyhow::{anyhow, Result};
use ethers::providers::Middleware;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use starknet::core::types::Felt;
use starknet_api::core::ChainId;
use starknet_old::core::types as starknet_old_types;
use starknet_providers::Provider;
use std::str::FromStr;
use url::Url;
use utoipa::ToSchema;
use walnut_shared::{
    chain_id_to_readable_string, create_eth_provider_from_url, create_rpc_client,
    create_rpc_client_from_url, eth_rpc_url, felt_to_field_element,
    parse_transaction_hash_per_network, ETransactionHashType,
};

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct Data {
    pub source: ESource,
    pub hash: String,
}

#[derive(Deserialize, Debug, Serialize, ToSchema, Eq, Hash, PartialEq)]
pub enum ESource {
    ChainId(String),
    RpcUrl(String),
}

#[derive(Debug)]
pub enum ESourceType {
    ChainId(ChainId),
    RpcUrl(Url),
}

impl From<&ESourceType> for ESource {
    fn from(source: &ESourceType) -> Self {
        match source {
            ESourceType::ChainId(chain_id) => {
                ESource::ChainId(chain_id_to_readable_string(chain_id))
            }
            ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
        }
    }
}

pub fn sources_from_rpc_urls(urls: Option<&str>) -> Result<Vec<ESourceType>> {
    let mut sources = vec![
        ESourceType::ChainId(ChainId::Mainnet),
        ESourceType::ChainId(ChainId::Sepolia),
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
                    sources.push(ESourceType::RpcUrl(parsed_url));
                }
                Err(err) => {
                    return Err(anyhow!("Invalid URL '{}': {}", trimmed_url, err));
                }
            }
        }
    }
    Ok(sources)
}

pub async fn check_transaction(hash: &str, sources: &[ESourceType]) -> Option<Vec<Data>> {
    let transaction_hash = parse_transaction_hash_per_network(hash)?;

    let mut futures = sources
        .iter()
        .map(|source| async move {
            let tx_hash = transaction_hash;
            match tx_hash {
                ETransactionHashType::Starknet(starknet_hash) => {
                    let provider = match source {
                        ESourceType::ChainId(chain_id) => create_rpc_client(chain_id),
                        ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
                    };
                    let hash_field = felt_to_field_element(starknet_hash);
                    provider
                        .get_transaction_status(hash_field)
                        .await
                        .ok()
                        .map(|_| Data {
                            source: match source {
                                ESourceType::ChainId(chain_id) => {
                                    ESource::ChainId(chain_id_to_readable_string(chain_id))
                                }
                                ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
                            },
                            hash: hash.to_string(),
                        })
                }
                ETransactionHashType::Ethereum(ethereum_hash) => {
                    if let ESourceType::ChainId(ref chain_id) = source {
                        let provider = create_eth_provider_from_url(eth_rpc_url(chain_id));
                        //provider.get_transaction(ethereum_hash).await.ok() return Option<Result<Transaction, _>>,
                        //and then ok() add one extra Option, which flatten() remove
                        let data = provider.get_transaction(ethereum_hash).await.ok().flatten();
                        if let Some(_) = data {
                            Some(Data {
                                source: ESource::ChainId(chain_id_to_readable_string(chain_id)),
                                hash: hash.to_string(),
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            }
        })
        .collect::<FuturesUnordered<_>>();

    let mut transactions = vec![];

    while let Some(result) = futures.next().await {
        if let Some(data) = result {
            transactions.push(data);
        }
    }

    if transactions.is_empty() {
        None
    } else {
        Some(transactions)
    }
}

pub async fn check_contract(hash: &str, sources: &[ESourceType]) -> Option<Vec<Data>> {
    let hash_field = felt_to_field_element(Felt::from_str(hash).ok()?);

    let mut futures: FuturesUnordered<_> = sources
        .iter()
        .map(|source| async move {
            let provider = match source {
                ESourceType::ChainId(chain_id) => create_rpc_client(chain_id),
                ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
            };

            provider
                .get_class_hash_at(
                    starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                    hash_field,
                )
                .await
                .map(|_| Data {
                    source: match source {
                        ESourceType::ChainId(chain_id) => {
                            ESource::ChainId(chain_id_to_readable_string(chain_id))
                        }
                        ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
                    },
                    hash: hash.to_string(),
                })
        })
        .collect();

    let mut contracts = vec![];

    while let Some(result) = futures.next().await {
        if let Ok(data) = result {
            contracts.push(data);
        }
    }

    if contracts.is_empty() {
        None
    } else {
        Some(contracts)
    }
}

pub async fn check_class(hash: &str, sources: &[ESourceType]) -> Option<Vec<Data>> {
    let hash_field = felt_to_field_element(Felt::from_str(hash).ok()?);

    let mut futures: FuturesUnordered<_> = sources
        .iter()
        .map(|source| async move {
            let provider = match source {
                ESourceType::ChainId(chain_id) => create_rpc_client(chain_id),
                ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
            };
            provider
                .get_class(
                    starknet_old_types::BlockId::Tag(starknet_old_types::BlockTag::Latest),
                    hash_field,
                )
                .await
                .map(|_| Data {
                    source: match source {
                        ESourceType::ChainId(chain_id) => {
                            ESource::ChainId(chain_id_to_readable_string(chain_id))
                        }
                        ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
                    },
                    hash: hash.to_string(),
                })
        })
        .collect();

    let mut classes = vec![];

    while let Some(result) = futures.next().await {
        if let Ok(data) = result {
            classes.push(data);
        }
    }

    if classes.is_empty() {
        None
    } else {
        Some(classes)
    }
}
