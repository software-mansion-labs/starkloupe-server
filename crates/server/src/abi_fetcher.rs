use data_decoder::type_decoder::{expand_enums_recursively, expand_structs_recursively};
use moka::future::Cache;
use starknet_rust::core::types::{BlockId, BlockTag, ContractClass, Felt};
use starknet_rust::providers::Provider;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use walnut_shared::abi::{get_enums, get_functions, get_structs, Enum, Function, Item, Struct};
use walnut_shared::utils::simplify_type_name;
use walnut_shared::{extract_chain_id, get_rpc_urls};

/// Parsed ABI data needed for calldata encoding.
/// Wrapped in `Arc` inside the cache so hits are cheap (no deep clones).
pub struct ContractAbiData {
    pub functions: Vec<Function>,
    pub structs: HashMap<String, Struct>,
    pub enums: HashMap<String, Enum>,
}

/// Cache key: (contract_address_lowercase, chain_id)
type AbiCacheKey = (String, String);

/// Global, lazily-initialised ABI cache (10-minute TTL, max 500 entries).
fn abi_cache() -> &'static Cache<AbiCacheKey, Arc<ContractAbiData>> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Cache<AbiCacheKey, Arc<ContractAbiData>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Cache::builder()
            .max_capacity(500)
            .time_to_live(Duration::from_secs(600))
            .build()
    })
}

/// Fetch contract ABI for encoding/decoding purposes.
/// Results are cached in-memory keyed by `(contract_address, chain_id)`.
pub async fn fetch_contract_abi(
    contract_address: &str,
    chain_id: &str,
) -> Result<Arc<ContractAbiData>, String> {
    let cache = abi_cache();
    let key: AbiCacheKey = (contract_address.to_lowercase(), chain_id.to_string());

    if let Some(cached) = cache.get(&key).await {
        info!("ABI cache hit for {} on {}", contract_address, chain_id);
        return Ok(cached);
    }

    info!("ABI cache miss for {} on {}", contract_address, chain_id);
    let data = fetch_contract_abi_uncached(contract_address, chain_id).await?;
    let data = Arc::new(data);
    cache.insert(key, data.clone()).await;
    Ok(data)
}

/// The actual RPC + parsing logic, called only on cache miss.
async fn fetch_contract_abi_uncached(
    contract_address: &str,
    chain_id: &str,
) -> Result<ContractAbiData, String> {
    let (e_chain_id, _network) =
        extract_chain_id(chain_id).map_err(|e| format!("Invalid chain ID: {}", e))?;

    let (starknet_rpc_url, _) = get_rpc_urls(&e_chain_id);
    let rpc_url = starknet_rpc_url.ok_or("No RPC URL available for chain")?;

    let provider = walnut_shared::create_rpc_client_from_url(rpc_url);

    // Parse contract address
    let contract_address_felt = Felt::from_hex(contract_address.trim_start_matches("0x"))
        .map_err(|e| format!("Invalid contract address: {}", e))?;

    // Get contract class at address
    let contract_class = provider
        .get_class_at(BlockId::Tag(BlockTag::Latest), contract_address_felt)
        .await
        .map_err(|e| format!("Failed to fetch contract class: {}", e))?;

    // Use existing ABI parsing logic from contracts handler
    match contract_class {
        ContractClass::Sierra(sierra_class) => {
            match serde_json::from_str::<Vec<Item>>(&sierra_class.abi) {
                Ok(parsed_abi) => {
                    // Pre-extract structs and enums once, create lookup maps
                    let structs = get_structs(&parsed_abi);
                    let enums = get_enums(&parsed_abi);
                    let functions = get_functions(&parsed_abi);

                    // Found structs and enums in ABI
                    let struct_map: HashMap<String, Struct> = structs
                        .into_iter()
                        .map(|s| (simplify_type_name(&s.name), s))
                        .collect();
                    let enum_map: HashMap<String, Enum> = enums
                        .into_iter()
                        .map(|e| (simplify_type_name(&e.name), e))
                        .collect();

                    // Recursively enhance all structs and enums with their members/variants
                    let expanded_struct_map = expand_structs_recursively(&struct_map, &enum_map);
                    let expanded_enum_map =
                        expand_enums_recursively(&enum_map, &expanded_struct_map);

                    Ok(ContractAbiData {
                        functions,
                        structs: expanded_struct_map,
                        enums: expanded_enum_map,
                    })
                }
                Err(err) => Err(format!("Failed to parse ABI: {}", err)),
            }
        }
        ContractClass::Legacy(_) => {
            Err("Legacy contracts not supported for ABI decoding".to_string())
        }
    }
}
