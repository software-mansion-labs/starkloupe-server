use anyhow::{anyhow, Result};
use ethers::providers::Middleware;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use starknet::core::types::Felt;
use starknet::providers::Provider;
use starknet_api::core::ChainId;
use std::str::FromStr;
use url::Url;
use utoipa::ToSchema;
use walnut_shared::{
    chain_id_to_readable_string, create_eth_provider_from_url, create_rpc_client,
    create_rpc_client_from_url, get_rpc_urls, parse_transaction_hash_per_network, EChainId,
    ENetwork, ETransactionHashType,
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
    ChainId(EChainId),
    RpcUrl(Url),
}

impl From<&ESourceType> for ESource {
    fn from(source: &ESourceType) -> Self {
        match source {
            ESourceType::ChainId(chain_id) => {
                let core_chain_id = ChainId::from(*chain_id);
                ESource::ChainId(chain_id_to_readable_string(&core_chain_id))
            }
            ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
        }
    }
}

pub fn sources_from_rpc_urls(urls: Option<&str>) -> Result<Vec<ESourceType>> {
    let mut sources = vec![
        ESourceType::ChainId(EChainId::StarknetMainnet),
        ESourceType::ChainId(EChainId::StarknetSepolia),
        ESourceType::ChainId(EChainId::EthereumMainnet),
        ESourceType::ChainId(EChainId::EthereumSepolia),
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
    let mut futures = sources
        .iter()
        .map(|source| async move {
            let (network, starknet_rpc_url, ethereum_rpc_url) = match source {
                ESourceType::ChainId(e_chain_id) => {
                    let network = match e_chain_id {
                        EChainId::StarknetMainnet | EChainId::StarknetSepolia => ENetwork::Starknet,
                        EChainId::EthereumMainnet | EChainId::EthereumSepolia => ENetwork::Ethereum,
                    };
                    let (starknet_rpc, eth_rpc) = get_rpc_urls(e_chain_id);
                    (network, starknet_rpc, eth_rpc)
                }
                ESourceType::RpcUrl(url) => (ENetwork::Starknet, Some(url.clone()), None),
            };
            let transaction_hash = parse_transaction_hash_per_network(hash, &network)?;
            match transaction_hash {
                ETransactionHashType::Starknet(starknet_hash) => {
                    let provider = match source {
                        ESourceType::ChainId(e_chain_id) => {
                            if let Some(rpc_url) = starknet_rpc_url.clone() {
                                create_rpc_client_from_url(rpc_url)
                            } else {
                                match e_chain_id {
                                    EChainId::StarknetMainnet | EChainId::StarknetSepolia => {
                                        let core_chain_id = ChainId::from(*e_chain_id);
                                        create_rpc_client(&core_chain_id)
                                    }
                                    _ => {
                                        return None;
                                    }
                                }
                            }
                        }
                        ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
                    };
                    provider
                        .get_transaction_status(starknet_hash)
                        .await
                        .ok()
                        .map(|_| Data {
                            source: source.into(),
                            hash: hash.to_string(),
                        })
                }
                ETransactionHashType::Ethereum(ethereum_hash) => {
                    if let ESourceType::ChainId(ref _chain_id) = source {
                        if let Some(rpc_url) = ethereum_rpc_url {
                            let provider = create_eth_provider_from_url(rpc_url);
                            //provider.get_transaction(ethereum_hash).await.ok() return Option<Result<Transaction, _>>,
                            //and then ok() add one extra Option, which flatten() remove
                            let data = provider.get_transaction(ethereum_hash).await.ok().flatten();
                            if data.is_some() {
                                Some(Data {
                                    source: source.into(),
                                    hash: hash.to_string(),
                                })
                            } else {
                                None
                            }
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
    let hash_felt = Felt::from_str(hash).ok()?;

    let mut futures: FuturesUnordered<_> = sources
        .iter()
        .filter_map(|source| {
            if let ESourceType::ChainId(e_chain_id) = source {
                match e_chain_id {
                    EChainId::StarknetMainnet | EChainId::StarknetSepolia => (),
                    _ => return None,
                }
            }
            Some(async move {
                let provider = match source {
                    ESourceType::ChainId(e_chain_id) => {
                        let core_chain_id = ChainId::from(*e_chain_id);
                        create_rpc_client(&core_chain_id)
                    }
                    ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
                };

                provider
                    .get_class_hash_at(
                        starknet::core::types::BlockId::Tag(
                            starknet::core::types::BlockTag::Latest,
                        ),
                        hash_felt,
                    )
                    .await
                    .ok()
                    .map(|_| Data {
                        source: match source {
                            ESourceType::ChainId(chain_id) => {
                                let core_chain_id = ChainId::from(*chain_id);
                                ESource::ChainId(chain_id_to_readable_string(&core_chain_id))
                            }
                            ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
                        },
                        hash: hash.to_string(),
                    })
            })
        })
        .collect();
    let mut contracts = vec![];

    while let Some(result) = futures.next().await {
        if let Some(data) = result {
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
    let hash_felt = Felt::from_str(hash).ok()?;

    let mut futures: FuturesUnordered<_> = sources
        .iter()
        .filter_map(|source| {
            if let ESourceType::ChainId(e_chain_id) = source {
                match e_chain_id {
                    EChainId::StarknetMainnet | EChainId::StarknetSepolia => (),
                    _ => return None,
                }
            }
            Some(async move {
                let provider = match source {
                    ESourceType::ChainId(chain_id) => {
                        let core_chain_id = ChainId::from(*chain_id);
                        create_rpc_client(&core_chain_id)
                    }
                    ESourceType::RpcUrl(url) => create_rpc_client_from_url(url.clone()),
                };
                provider
                    .get_class(
                        starknet::core::types::BlockId::Tag(
                            starknet::core::types::BlockTag::Latest,
                        ),
                        hash_felt,
                    )
                    .await
                    .map(|_| Data {
                        source: match source {
                            ESourceType::ChainId(chain_id) => {
                                let core_chain_id = ChainId::from(*chain_id);
                                ESource::ChainId(chain_id_to_readable_string(&core_chain_id))
                            }
                            ESourceType::RpcUrl(url) => ESource::RpcUrl(url.to_string()),
                        },
                        hash: hash.to_string(),
                    })
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
