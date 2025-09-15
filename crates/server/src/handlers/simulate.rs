use crate::app_state::AppState;
use crate::services::CacheKey;
use crate::telegram_bot_service::{
    send_telegram_notification_calldata, send_telegram_notification_custom_rpc,
    send_telegram_notification_tx_id,
};
use axum::extract::Query;
use axum::{
    debug_handler,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use starknet::core::types::{BlockId, BlockTag, ContractClass, Felt};
use starknet::providers::Provider;
use simulate::{
    simulate::{simulate_by_calldata, simulate_transaction_by_hash},
    SimulationArgs, SimulationRawArgs,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tokio::time::timeout;
use tracing::{error, info};
use walnut_shared::{extract_chain_id, get_rpc_urls, ENetwork};
use walnut_shared::abi::{get_enums, get_functions, get_structs, Item};
use walnut_shared::utils::simplify_type_name;
use data_decoder::type_decoder::{expand_enums_recursively, expand_structs_recursively, TypeDecoder};

/// Fetch contract ABI for encoding purposes
async fn fetch_contract_abi_for_encoding(
    contract_address: &str,
    chain_id: &str,
) -> Result<(Vec<walnut_shared::abi::Function>, TypeDecoder, std::collections::HashMap<String, walnut_shared::abi::Struct>, std::collections::HashMap<String, walnut_shared::abi::Enum>), String> {
    let (e_chain_id, _network) = extract_chain_id(chain_id)
        .map_err(|e| format!("Invalid chain ID: {}", e))?;
    
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
                    let struct_map: std::collections::HashMap<
                        String,
                        walnut_shared::abi::Struct,
                    > = structs
                        .into_iter()
                        .map(|s| (simplify_type_name(&s.name), s))
                        .collect();
                    let enum_map: std::collections::HashMap<String, walnut_shared::abi::Enum> =
                        enums
                            .into_iter()
                            .map(|e| (simplify_type_name(&e.name), e))
                            .collect();

                    // Recursively enhance all structs and enums with their members/variants
                    let expanded_struct_map =
                        expand_structs_recursively(&struct_map, &enum_map);
                    let expanded_enum_map =
                        expand_enums_recursively(&enum_map, &expanded_struct_map);

                    // Create TypeDecoder with complete type information
                    let type_decoder = TypeDecoder::new(expanded_struct_map.clone(), expanded_enum_map.clone());
                    
                    Ok((functions, type_decoder, expanded_struct_map, expanded_enum_map))
                }
                Err(err) => {
                    Err(format!("Failed to parse ABI: {}", err))
                }
            }
        }
        ContractClass::Legacy(_) => {
            Err("Legacy contracts not supported for ABI decoding".to_string())
        }
    }
}



/// Convert decoded calldata back to raw calldata format with ABI information
fn encode_decoded_calldata_with_abi(
    decoded_calls: &[ContractCall],
    structs: &std::collections::HashMap<String, walnut_shared::abi::Struct>,
    enums: &std::collections::HashMap<String, walnut_shared::abi::Enum>,
) -> Result<Vec<String>, String> {
    let mut raw_calldata = Vec::new();
    
    // Add number of calls
    raw_calldata.push(format!("0x{:x}", decoded_calls.len()));
    
    for call in decoded_calls {
        // Add contract address
        raw_calldata.push(call.contract_address.clone());
        
        // Add function selector
        raw_calldata.push(call.function_selector.clone());
        
        // Encode parameters to calldata
        let call_calldata = encode_parameters_with_abi(&call.parameters, structs, enums)?;
        
        // Add calldata length
        raw_calldata.push(format!("0x{:x}", call_calldata.len()));
        
        // Add actual calldata
        raw_calldata.extend(call_calldata);
    }
    
    Ok(raw_calldata)
}

/// Encode parameters to calldata format
fn encode_parameters(parameters: &[DecodedValue]) -> Result<Vec<String>, String> {
    encode_parameters_with_abi(parameters, &std::collections::HashMap::new(), &std::collections::HashMap::new())
}

/// Encode parameters to calldata format with ABI information
fn encode_parameters_with_abi(
    parameters: &[DecodedValue],
    structs: &std::collections::HashMap<String, walnut_shared::abi::Struct>,
    enums: &std::collections::HashMap<String, walnut_shared::abi::Enum>,
) -> Result<Vec<String>, String> {
    let mut calldata = Vec::new();
    
    for param in parameters {
        match &param.value {
            DecodedValueType::String(value) => {
                // For simple values, just add them as hex
                calldata.push(value.clone());
            }
            DecodedValueType::Struct(fields) => {
                // For structs, recursively encode each field
                let struct_calldata = encode_parameters(fields)?;
                calldata.extend(struct_calldata);
            }
            DecodedValueType::Array(items) => {
                // For arrays, recursively encode each item
                for item in items {
                    match item {
                        DecodedValueType::String(value) => {
                            calldata.push(value.clone());
                        }
                        DecodedValueType::Struct(fields) => {
                            // For struct arrays, encode each field recursively
                            let struct_calldata = encode_parameters(fields)?;
                            calldata.extend(struct_calldata);
                        }
                        DecodedValueType::Array(nested_items) => {
                            // For nested arrays, encode each nested item
                            for nested_item in nested_items {
                                match nested_item {
                                    DecodedValueType::String(value) => {
                                        calldata.push(value.clone());
                                    }
                        DecodedValueType::Struct(_) | DecodedValueType::Enum(_, _) | DecodedValueType::Array(_) => {
                            return Err(format!("Unsupported nested array item type for parameter: {}", param.name.as_deref().unwrap_or("unnamed")));
                        }
                                }
                            }
                        }
                        DecodedValueType::Enum(_variant, inner) => {
                            // For enum arrays, encode the inner value
                            match &inner.value {
                                DecodedValueType::String(value) => {
                                    if let Ok(hex_val) = u64::from_str_radix(value.trim_start_matches("0x"), 16) {
                                        calldata.push(format!("0x{:x}", hex_val));
                                    } else if let Ok(dec_val) = value.parse::<u64>() {
                                        calldata.push(format!("0x{:x}", dec_val));
                                    } else {
                                        return Err(format!("Invalid enum value in array: {}", value));
                                    }
                                }
                                _ => {
                                    return Err(format!("Enum inner value must be a string in array"));
                                }
                            }
                        }
                        _ => {
                            return Err(format!("Unsupported array item type for parameter: {}", param.name.as_deref().unwrap_or("unnamed")));
                        }
                    }
                }
            }
            DecodedValueType::Enum(variant, inner) => {
                // For enums, first encode the variant index, then the inner data
                // Simple enums have only numeric values
                // Complex enums have variant + inner structures
                
                // Check if this is a simple enum (inner value is just a string/number)
                if matches!(inner.value, DecodedValueType::String(_)) {
                    // Simple enum - just encode the numeric value
                    match &inner.value {
                        DecodedValueType::String(value) => {
                            if let Ok(hex_val) = u64::from_str_radix(value.trim_start_matches("0x"), 16) {
                                calldata.push(format!("0x{:x}", hex_val));
                            } else if let Ok(dec_val) = value.parse::<u64>() {
                                calldata.push(format!("0x{:x}", dec_val));
                            } else {
                                return Err(format!("Invalid simple enum value: {}", value));
                            }
                        }
                        _ => {
                            return Err(format!("Unexpected inner value type for simple enum"));
                        }
                    }
                } else {
                    // Complex enum - encode variant index + inner structures
                    // Try to find the enum definition in ABI to get variant index
                    let variant_index = if let Ok(index) = variant.parse::<u64>() {
                        // If variant name is already a number (e.g., "0", "1", "2")
                        index
                    } else {
                        // Try to find variant index from ABI
                        let enum_name = param.type_name.split("::").next().unwrap_or(&param.type_name);
                        if let Some(enum_def) = enums.get(enum_name) {
                            // Find variant index by name
                            enum_def.variants.iter()
                                .position(|v| v.name == *variant)
                                .map(|idx| idx as u64)
                                .ok_or_else(|| format!(
                                    "Variant '{}' not found in enum '{}'. Available variants: {:?}",
                                    variant, enum_name, 
                                    enum_def.variants.iter().map(|v| &v.name).collect::<Vec<_>>()
                                ))?
                        } else {
                            return Err(format!(
                                "Enum '{}' not found in ABI. Cannot determine variant index for '{}'",
                                enum_name, variant
                            ));
                        }
                    };
                    
                    // Add variant index
                    calldata.push(format!("0x{:x}", variant_index));
                    
                    // Recursively encode inner structures
                    let inner_calldata = encode_parameters_with_abi(&[inner.as_ref().clone()], structs, enums)?;
                    calldata.extend(inner_calldata);
                }
            }
        }
    }
    
    Ok(calldata)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
pub enum SimulationPayload {
    WithCalldata(SimulationRawArgs),
    WithDecodedCalldata(SimulationDecodedArgs),
    WithTxHash(SimulationTxHashArgs),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SimulationDecodedArgs {
    pub chain_id: Option<String>,
    pub rpc_url: Option<String>,
    pub block_number: Option<u64>,
    pub nonce: Option<u64>,
    pub sender_address: String,
    pub decoded_calldata: Vec<ContractCall>,
    pub transaction_version: usize,
    pub transaction_signature: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContractCall {
    pub contract_address: String,
    pub function_selector: String,
    pub function_name: Option<String>,
    pub parameters: Vec<DecodedValue>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DecodedValue {
    pub name: Option<String>,
    pub type_name: String,
    pub value: DecodedValueType,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum DecodedValueType {
    String(String),
    Struct(Vec<DecodedValue>),
    Array(Vec<DecodedValueType>),
    Enum(String, Box<DecodedValue>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SimulationTxHashArgs {
    pub rpc_url: String,
    pub tx_hash: String,
}

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    skip_tracking: Option<String>,
}

#[debug_handler]
pub async fn simulate_transaction(
    State(state): State<Arc<AppState>>,
    Query(query_params): Query<QueryParams>,
    Json(payload): Json<SimulationPayload>,
) -> Response {
    let db_pool = state.db_pool.clone();
    let s3_client = state.s3_client.clone();
    let skip_tracking = query_params.skip_tracking.clone();
    let payload = payload.clone();
    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            match payload {
                SimulationPayload::WithCalldata(args) => {
                    // Parse calldata args
                    let simulation_args: SimulationArgs =
                        match SimulationArgs::try_from_raw_args(args).await {
                            Ok(args) => args,
                            Err(e) => {
                                return Err((StatusCode::BAD_REQUEST, e.to_string()));
                            }
                        };

                    // Check cache first
                    let cache_key = CacheKey::from_simulation_args(&simulation_args);
                    if let Some(cached_result) = state.simulation_cache.get(&cache_key).await {
                        info!("Cache hit! Returning cached result");
                        return Ok((StatusCode::OK, cached_result)); // Return Arc directly - no clone!
                    }
                    info!("Cache miss, proceeding with simulation");

                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) =
                            send_telegram_notification_calldata(&simulation_args).await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    // Run simulation
                    let result = simulate_by_calldata(&db_pool, &s3_client, simulation_args).await;
                    match result {
                        Ok(sim_info) => {
                            // Wrap in Arc and cache the result
                            let sim_info_arc = Arc::new(sim_info);
                            state.simulation_cache.set(&cache_key, sim_info_arc.clone()).await;
                            info!("Cached simulation result");
                            
                            Ok((StatusCode::OK, sim_info_arc))
                        },
                        Err(e) => {
                            info!("Simulation failed after: {}", e);
                            Err((StatusCode::BAD_REQUEST, e.to_string()))
                        }
                    }
                }

                SimulationPayload::WithDecodedCalldata(args) => {
                    // Fetch ABI for the first contract to get struct and enum definitions
                    let (structs, enums) = if let Some(first_call) = args.decoded_calldata.first() {
                        match fetch_contract_abi_for_encoding(&first_call.contract_address, &args.chain_id.clone().unwrap_or_default()).await {
                            Ok((_, _, structs, enums)) => (structs, enums),
                            Err(e) => {
                                error!("Failed to fetch ABI for encoding: {}", e);
                                // Fallback to empty maps - will use basic encoding
                                (std::collections::HashMap::new(), std::collections::HashMap::new())
                            }
                        }
                    } else {
                        (std::collections::HashMap::new(), std::collections::HashMap::new())
                    };

                    // Convert decoded calldata to raw calldata with ABI information
                    let raw_calldata = match encode_decoded_calldata_with_abi(&args.decoded_calldata, &structs, &enums) {
                        Ok(calldata) => calldata,
                        Err(e) => {
                            return Err((StatusCode::BAD_REQUEST, format!("Failed to encode decoded calldata: {}", e)));
                        }
                    };

                    info!("Raw calldata: {:?}", raw_calldata);
                    // Create SimulationRawArgs from decoded args
                    let raw_args = SimulationRawArgs {
                        chain_id: args.chain_id,
                        rpc_url: args.rpc_url,
                        block_number: args.block_number,
                        nonce: args.nonce,
                        sender_address: args.sender_address,
                        calldata: raw_calldata,
                        transaction_version: args.transaction_version,
                        transaction_signature: args.transaction_signature.map(|sig| {
                            sig.into_iter().filter_map(|s| s.parse().ok()).collect()
                        }),
                    };

                    // Parse calldata args
                    let simulation_args: SimulationArgs =
                        match SimulationArgs::try_from_raw_args(raw_args).await {
                            Ok(args) => args,
                            Err(e) => {
                                return Err((StatusCode::BAD_REQUEST, e.to_string()));
                            }
                        };

                    // Check cache first
                    let cache_key = CacheKey::from_simulation_args(&simulation_args);
                    if let Some(cached_result) = state.simulation_cache.get(&cache_key).await {
                        info!("Cache hit! Returning cached result");
                        return Ok((StatusCode::OK, cached_result));
                    }
                    info!("Cache miss, proceeding with simulation");

                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) =
                            send_telegram_notification_calldata(&simulation_args).await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    // Run simulation
                    let result = simulate_by_calldata(&db_pool, &s3_client, simulation_args).await;
                    match result {
                        Ok(sim_info) => {
                            // Wrap in Arc and cache the result
                            let sim_info_arc = Arc::new(sim_info);
                            state.simulation_cache.set(&cache_key, sim_info_arc.clone()).await;
                            info!("Cached simulation result");
                            
                            Ok((StatusCode::OK, sim_info_arc))
                        },
                        Err(e) => {
                            info!("Simulation failed after: {}", e);
                            Err((StatusCode::BAD_REQUEST, e.to_string()))
                        }
                    }
                }

                SimulationPayload::WithTxHash(args) => {
                    // Check cache first using tx hash
                    let cache_key = CacheKey::from_tx_hash(&args.tx_hash, "starknet");
                    
                    if let Some(cached_result) = state.simulation_cache.get(&cache_key).await {
                        info!("Cache hit for tx hash! Returning cached result");
                        return Ok((StatusCode::OK, cached_result));
                    }
                    info!("Cache miss for tx hash, proceeding with simulation");
                    // Telegram notification
                    if !skip_tracking.as_deref().unwrap_or("").eq("true") {
                        if let Err(err) = send_telegram_notification_custom_rpc(
                            args.tx_hash.as_str(),
                            args.rpc_url.as_str(),
                        )
                        .await
                        {
                            error!("Failed to send Telegram notification. Error: {:?}", err);
                        }
                    }

                    let starknet_rpc_url = match url::Url::parse(&args.rpc_url) {
                        Ok(url) => url,
                        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
                    };

                    match simulate_transaction_by_hash(
                        &db_pool,
                        &s3_client,
                        Some(starknet_rpc_url),
                        None,
                        &args.tx_hash,
                        None,
                        &ENetwork::Starknet,
                    )
                    .await
                    {
                        Ok(sim_info) => {
                            // Wrap in Arc and cache the result
                            let sim_info_arc = Arc::new(sim_info);
                            state.simulation_cache.set(&cache_key, sim_info_arc.clone()).await;
                            info!("Cached tx hash simulation result");
                            
                            Ok((StatusCode::OK, sim_info_arc))
                        },
                        Err(e) => {
                            info!("Tx hash simulation failed after: {}", e);
                            Err((StatusCode::BAD_REQUEST, e.to_string()))
                        }
                    }
                }
            }
        })
    });
    
    match timeout(Duration::from_secs(900), simulation_task).await {
        Ok(Ok(Ok((status, sim_info)))) => (status, Json(sim_info)).into_response(),
        Ok(Ok(Err((status, message)))) => (status, Json(message)).into_response(),
        Ok(Err(join_err)) => {
            error!("Simulation task panicked: {:?}", join_err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulate transaction. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
        Err(_) => {
            error!("Simulation transaction request timed out");
            (
                StatusCode::REQUEST_TIMEOUT,
                "The server timed out. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
    }
}

pub async fn simulate_transaction_by_hash_handler(
    State(state): State<Arc<AppState>>,
    Path((chain_id, tx_hash)): Path<(String, String)>,
    Query(query_params): Query<QueryParams>,
) -> Response {
    // don't sent Telegram notification if query param skip_tg_notification=true (it set in URLs sent to tg bot)
    if !query_params
        .skip_tracking
        .as_deref()
        .unwrap_or("")
        .eq("true")
    {
        if let Err(err) =
            send_telegram_notification_tx_id(tx_hash.as_str(), chain_id.as_str()).await
        {
            error!("Failed to send Telegram notification. Error: {:?}", err);
        }
    }

    let (e_chain_id, network) = match extract_chain_id(chain_id.as_str()) {
        Ok((chain_id, network)) => (chain_id, network),
        Err(e) => return (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
    };

    let (starknet_rpc_url, etherem_rpc_url) = get_rpc_urls(&e_chain_id);

    let db_pool = state.db_pool.clone();
    let s3_client = state.s3_client.clone();
    let tx_hash = tx_hash.clone();
    let payload_tx_hash = tx_hash.clone();
    let network = network.clone();
    let e_chain_id = Some(e_chain_id);

    let cache = state.simulation_cache.clone();
    let chain_id_clone = chain_id.clone();
    let simulation_task = task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            // Check cache first
            let cache_key = CacheKey::from_tx_hash(&tx_hash, &chain_id_clone);
            
            if let Some(cached_result) = cache.get(&cache_key).await {
                info!("Cache hit for tx hash handler! Returning cached result");
                return Ok(cached_result);
            }
            info!("Cache miss for tx hash handler, proceeding with simulation");
            
            let result = simulate_transaction_by_hash(
                &db_pool,
                &s3_client,
                starknet_rpc_url,
                etherem_rpc_url,
                &tx_hash,
                e_chain_id,
                &network,
            )
            .await;
            
            match result {
                Ok(sim_info) => {
                    // Wrap in Arc and cache the result
                    let sim_info_arc = Arc::new(sim_info);
                    cache.set(&cache_key, sim_info_arc.clone()).await;
                    info!("Cached simulation by hash result");
                    
                    Ok(sim_info_arc)
                },
                Err(e) => {
                    info!("Simulation by hash failed after: {}", e);
                    Err(e)
                }
            }
        })
    });

    // Wait for simulation with timeout
    match timeout(Duration::from_secs(900), simulation_task).await {
        Ok(Ok(Ok(simulation_info))) => (StatusCode::OK, Json(simulation_info)).into_response(),
        Ok(Ok(Err(e))) => (StatusCode::BAD_REQUEST, Json(e.to_string())).into_response(),
        Ok(Err(join_err)) => {
            error!("Simulation of tx panicked: {:?}", join_err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to simulate transaction. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
        Err(_) => {
            error!(
                "Simulation transaction request by tx hash  timed out {}: {}",
                &chain_id, &payload_tx_hash
            );
            (
                StatusCode::REQUEST_TIMEOUT,
                "The server timed out. Reach out to us for assistance.".to_string(),
            )
                .into_response()
        }
    }
}

// Commented out cache stats endpoint - using logging instead
// pub async fn cache_stats_handler(State(state): State<Arc<AppState>>) -> Response {
//     match serde_json::to_value(state.simulation_cache.stats().await) {
//         Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
//         Err(e) => (
//             StatusCode::INTERNAL_SERVER_ERROR,
//             Json(format!("Failed to get cache stats: {}", e)),
//         )
//             .into_response(),
//     }
// }
