use crate::app_state::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
// Using our own DecodedValue struct
use data_decoder::type_decoder::{
    expand_enums_recursively, expand_structs_recursively, TypeDecoder,
};
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use starknet::core::types::{BlockId, BlockTag, ContractClass, Felt};
use starknet::providers::Provider;
use std::sync::Arc;
use tracing::{error, info};
use utoipa::ToSchema;
use walnut_shared::{
    create_rpc_client_from_url, extract_chain_id, get_rpc_urls,
};
use walnut_shared::abi::{get_enums, get_functions, get_structs, Item};
use walnut_shared::utils::simplify_type_name;

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct CalldataDecoderRequest {
    pub tx_hash: String,
    pub sender_address: String,
    pub calldata: String,
    pub transaction_version: u32,
    pub block_number: Option<u64>,
    pub chain_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct CalldataDecoderResponse {
    pub decoded_calldata: Vec<ContractCall>,
    pub raw_calldata: Vec<String>,
    pub num_calls: Option<u32>,
    pub transaction_version: u32,
    pub network: String,
    pub sender_address: String,
    pub block_number: Option<u64>,
}

#[derive(Serialize, ToSchema)]
pub struct ContractCall {
    pub contract_address: String,
    pub function_selector: String,
    pub function_name: Option<String>,
    pub parameters: Vec<DecodedValue>,
}

#[derive(Serialize, ToSchema, Debug)]
pub struct DecodedValue {
    pub name: Option<String>,
    pub type_name: String,
    pub value: DecodedValueType,
}

#[derive(Serialize, ToSchema, Debug)]
#[serde(untagged)]
pub enum DecodedValueType {
    String(String),
    Struct(Vec<DecodedValue>),
    Array(Vec<DecodedValueType>),
    Enum(String, Box<DecodedValue>),
}

// Use existing ABI types from walnut_shared

/// Convert data_decoder::DecodedValueType to our DecodedValueType
fn convert_decoded_value_type(value_type: &data_decoder::DecodedValueType) -> DecodedValueType {
    match value_type {
        data_decoder::DecodedValueType::String(s) => DecodedValueType::String(s.clone()),
        data_decoder::DecodedValueType::Single(v) => DecodedValueType::String(v.to_hex_string()),
        data_decoder::DecodedValueType::BigUint(v) => DecodedValueType::String(v.to_string()),
        data_decoder::DecodedValueType::BigInt(v) => DecodedValueType::String(v.to_string()),
        data_decoder::DecodedValueType::Bool(b) => DecodedValueType::String(b.to_string()),
        data_decoder::DecodedValueType::Array(items) => {
            DecodedValueType::Array(items.iter().map(convert_decoded_value_type).collect())
        }
        data_decoder::DecodedValueType::Struct(fields) => {
            DecodedValueType::Struct(fields.values().map(convert_decoded_value).collect())
        }
        data_decoder::DecodedValueType::Enum(variant, inner) => {
            DecodedValueType::Enum(variant.clone(), Box::new(convert_decoded_value(inner)))
        }
        data_decoder::DecodedValueType::None => DecodedValueType::String("None".to_string()),
    }
}

/// Convert data_decoder::DecodedValue to our DecodedValue
fn convert_decoded_value(value: &data_decoder::DecodedValue) -> DecodedValue {
    DecodedValue {
        name: value.name.clone(),
        type_name: value.type_name.clone(),
        value: convert_decoded_value_type(&value.value),
    }
}

/// Decode calldata from URL-encoded string format
fn parse_calldata_from_url(calldata_str: &str) -> Result<Vec<Felt>, String> {
    let calldata_parts: Vec<&str> = calldata_str.split(',').collect();
    
    let mut calldata_vec = Vec::new();
    for part in calldata_parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        
        // Remove 0x prefix if present
        let hex_str = if trimmed.starts_with("0x") {
            &trimmed[2..]
        } else {
            trimmed
        };
        
        // Parse as hex
        let felt = Felt::from_hex(hex_str)
            .map_err(|e| format!("Failed to parse calldata value '{}': {}", trimmed, e))?;
        calldata_vec.push(felt);
    }
    
    Ok(calldata_vec)
}

/// Extract call information from calldata
fn extract_call_info(calldata: &[Felt]) -> (Option<u32>, Option<String>, Option<String>) {
    if calldata.len() < 3 {
        return (None, None, None);
    }
    
    // First element: number of calls
    let num_calls = calldata[0].to_usize().map(|n| n as u32);
    
    // Second element: contract address
    let contract_address = Some(calldata[1].to_hex_string());
    
    // Third element: function selector
    let function_selector = Some(calldata[2].to_hex_string());
    
    (num_calls, contract_address, function_selector)
}

/// Parse all calls from multi-call calldata
fn parse_multicall_calls(calldata: &[Felt]) -> Vec<(String, String, Vec<Felt>)> {
    let mut calls = Vec::new();
    
    if calldata.len() < 3 {
        return calls;
    }
    
    let num_calls = match calldata[0].to_usize() {
        Some(n) => n,
        None => return calls,
    };
    
    let mut offset = 1; // Skip num_calls
    
    for call_idx in 0..num_calls {
        if offset + 2 >= calldata.len() {
            info!("Not enough data for call {} at offset {}", call_idx, offset);
            break; 
        }
        
        let contract_address = calldata[offset].to_hex_string();
        let function_selector = calldata[offset + 1].to_hex_string();
        
        // The next element should be the calldata length for this call
        let call_calldata_len = match calldata[offset + 2].to_usize() {
            Some(len) => len,
            None => {
                info!("Invalid calldata length for call {} at offset {}", call_idx, offset + 2);
                break;
            }
        };
        
        // Check if we have enough data for this call's calldata
        if offset + 3 + call_calldata_len > calldata.len() {
            info!("Not enough calldata for call {}: need {} elements, have {} remaining", 
                  call_idx, call_calldata_len, calldata.len() - offset - 3);
            break;
        }
        
        // Extract the actual calldata for this call
        let call_calldata = calldata[offset + 3..offset + 3 + call_calldata_len].to_vec();
        
        info!("Call {}: contract={}, selector={}, calldata_len={}, actual_calldata={:?}", 
              call_idx, contract_address, function_selector, call_calldata_len, 
              call_calldata.iter().map(|f| f.to_hex_string()).collect::<Vec<_>>());
        
        calls.push((contract_address, function_selector, call_calldata));
        
        // Move offset to next call: contract_address + function_selector + calldata_length + actual_calldata
        offset += 3 + call_calldata_len;
        
        if offset >= calldata.len() {
            break;
        }
    }
    
    calls
}

/// Fetch contract ABI using existing logic from contracts handler
async fn fetch_contract_abi(
    contract_address: &str,
    chain_id: &str,
) -> Result<(Vec<walnut_shared::abi::Function>, TypeDecoder, std::collections::HashMap<String, walnut_shared::abi::Struct>, std::collections::HashMap<String, walnut_shared::abi::Enum>), String> {
    let (e_chain_id, _network) = extract_chain_id(chain_id)
        .map_err(|e| format!("Invalid chain ID: {}", e))?;
    
    let (starknet_rpc_url, _) = get_rpc_urls(&e_chain_id);
    let rpc_url = starknet_rpc_url.ok_or("No RPC URL available for chain")?;
    
    let provider = create_rpc_client_from_url(rpc_url);
    
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
                        info!("Successfully parsed ABI with {} items", parsed_abi.len());
                        
                        // Pre-extract structs and enums once, create lookup maps
                        let structs = get_structs(&parsed_abi);
                        let enums = get_enums(&parsed_abi);
                        let functions = get_functions(&parsed_abi);
                        
                        info!("Extracted from ABI: {} functions, {} structs, {} enums", functions.len(), structs.len(), enums.len());

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

/// Decode calldata using ABI and TypeDecoder
fn decode_calldata_with_abi(
    calldata: &[Felt],
    function_selector: &str,
    functions: &[walnut_shared::abi::Function],
    _type_decoder: &TypeDecoder,
    structs: &std::collections::HashMap<String, walnut_shared::abi::Struct>,
    enums: &std::collections::HashMap<String, walnut_shared::abi::Enum>,
) -> Result<(Vec<DecodedValue>, Option<String>), String> {
    // Find function by selector
    let function = functions.iter()
        .find(|func| {
            use starknet_api::abi::abi_utils::selector_from_name;
            let calculated_selector = selector_from_name(&func.name).0.to_hex_string();
            calculated_selector == function_selector
        })
        .ok_or_else(|| format!("Function with selector {} not found in ABI", function_selector))?;
    
    // Convert function inputs to data-decoder format, unpacking structures using TypeDecoder
    let mut types: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    
    for input in &function.inputs {
        let simplified_type = walnut_shared::utils::simplify_type_name(&input.ty);
        
        // Check if this is a struct that needs unpacking
        if let Some(struct_def) = structs.get(&simplified_type) {
            // Unpack struct into its primitive components
            for member in &struct_def.members {
                types.push(member.ty.clone());
                names.push(format!("{}_{}", input.name, member.name));
            }
        } else if enums.contains_key(&simplified_type) {
            // For enums, use simplified type name - data-decoder expects simplified names
            types.push(simplified_type.clone());
            names.push(input.name.clone());
        } else {
            // Keep as is for primitive types
            types.push(input.ty.clone());
            names.push(input.name.clone());
        }
    }
    
    // Convert to Cow<str> for data-decoder
    let types_cow: Vec<std::borrow::Cow<str>> = types.iter().map(|s| std::borrow::Cow::Borrowed(s.as_str())).collect();
    let names_cow: Vec<std::borrow::Cow<str>> = names.iter().map(|s| std::borrow::Cow::Borrowed(s.as_str())).collect();
    
    // Convert structs and enums to data-decoder format
    let structs_vec: Vec<walnut_shared::abi::Struct> = structs.values().cloned().collect();
    let enums_vec: Vec<walnut_shared::abi::Enum> = enums.values().cloned().collect();
    
    // Log struct details
    for (i, struct_def) in structs_vec.iter().enumerate() {
        info!("  Struct[{}]: {} with {} members", i, struct_def.name, struct_def.members.len());
        for (j, member) in struct_def.members.iter().enumerate() {
            info!("    Member[{}]: {} ({})", j, member.name, member.ty);
        }
    }
    
    // Log before decode_calldata call
    info!("Decoding calldata with ABI:");
    info!("  Function: {}", function.name);
    info!("  Calldata length: {}", calldata.len());
    info!("  Unpacked types: {:?}", types);
    info!("  Unpacked names: {:?}", names);
    info!("  Structs count: {}", structs_vec.len());
    info!("  Enums count: {}", enums_vec.len());
    info!("  Calldata values: {:?}", calldata.iter().map(|f| f.to_hex_string()).collect::<Vec<_>>());
    
    // Use data-decoder to decode with proper types
    use data_decoder::calldata_decoder::decode_calldata;
    info!("Calling decode_calldata with:");
    info!("  calldata: {:?}", calldata.iter().map(|f| f.to_hex_string()).collect::<Vec<_>>());
    info!("  types_cow: {:?}", types_cow);
    info!("  names_cow: {:?}", names_cow);
    info!("  structs_vec: {} structs", structs_vec.len());
    info!("  enums_vec: {} enums", enums_vec.len());
    
    let decoded_values = match decode_calldata(
        calldata,
        &types_cow,
        &names_cow,
        &structs_vec,
        &enums_vec,
    ) {
        Some(values) => {
            info!("decode_calldata returned {} values", values.len());
            values
        }
        None => {
            error!("decode_calldata returned None - decoding failed");
            error!("Debug info:");
            error!("  calldata length: {}", calldata.len());
            error!("  types: {:?}", types_cow);
            error!("  names: {:?}", names_cow);
            error!("  structs: {} items", structs_vec.len());
            error!("  enums: {} items", enums_vec.len());
            for (i, enum_def) in enums_vec.iter().enumerate() {
                error!("    Enum[{}]: {} with {} variants", i, enum_def.name, enum_def.variants.len());
                for (j, variant) in enum_def.variants.iter().enumerate() {
                    error!("      Variant[{}]: {} ({})", j, variant.name, variant.ty);
                }
            }
            return Err("Failed to decode calldata with ABI types".to_string());
        }
    };
    
    // Convert to our DecodedValue format
    let converted_values: Vec<DecodedValue> = decoded_values.iter().map(convert_decoded_value).collect();
    
    // Log after decode_calldata call
    info!("Decoded values count: {}", converted_values.len());
    for (i, decoded) in converted_values.iter().enumerate() {
        info!("  [{}] {} ({}): {:?}", i, decoded.name.as_deref().unwrap_or("unnamed"), decoded.type_name, decoded.value);
    }
    
    Ok((converted_values, Some(function.name.clone())))
}


#[utoipa::path(
    post,
    path = "/v1/decode-calldata",
    request_body = CalldataDecoderRequest,
    responses(
        (status = 200, description = "Successfully decoded calldata", body = CalldataDecoderResponse),
        (status = 400, description = "Invalid request parameters"),
        (status = 500, description = "Internal server error")
    ),
    tag = "Calldata Decoder"
)]
pub async fn decode_calldata_handler(
    State(_state): State<Arc<AppState>>,
    Json(request): Json<CalldataDecoderRequest>,
) -> Response {
    info!("Received calldata decode request for tx: {}", request.tx_hash);
    // Parse calldata from URL-encoded string
    let calldata_felts = match parse_calldata_from_url(&request.calldata) {
        Ok(felts) => felts,
        Err(e) => {
            error!("Failed to parse calldata: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(CalldataDecoderResponse {
                    decoded_calldata: vec![],
                    raw_calldata: vec![],
                    num_calls: None,
                    transaction_version: request.transaction_version,
                    network: request.chain_id,
                    sender_address: request.sender_address,
                    block_number: request.block_number,
                }),
            )
                .into_response();
        }
    };
    
    // Extract multi-call information
    let (num_calls, contract_address, function_selector) = extract_call_info(&calldata_felts);
    
    // Convert to hex strings for raw_calldata
    let raw_calldata: Vec<String> = calldata_felts
        .iter()
        .map(|felt| felt.to_hex_string())
        .collect();
    
    // Decode calldata based on call structure with ABI
    let mut contract_calls = Vec::new();
    if calldata_felts.len() >= 3 {
        // Parse all calls from call calldata
        let calls = parse_multicall_calls(&calldata_felts);
        info!("Parsed {} calls from multi-call calldata", calls.len());
        
        for (call_idx, (contract_addr, func_selector, call_calldata)) in calls.into_iter().enumerate() {
            info!("Processing call {}: contract={}, selector={}, calldata_len={}", 
                  call_idx, contract_addr, func_selector, call_calldata.len());
            
            // Try to fetch ABI and decode with it
            match fetch_contract_abi(&contract_addr, &request.chain_id).await {
                Ok((functions, type_decoder, structs, enums)) => {
                    match decode_calldata_with_abi(&call_calldata, &func_selector, &functions, &type_decoder, &structs, &enums) {
                        Ok((decoded, func_name)) => {
                            info!("Successfully decoded call {} with ABI", call_idx);
                            contract_calls.push(ContractCall {
                                contract_address: contract_addr,
                                function_selector: func_selector,
                                function_name: func_name,
                                parameters: decoded,
                            });
                        }
                        Err(e) => {
                            error!("Failed to decode call {} with ABI: {}", call_idx, e);
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(CalldataDecoderResponse {
                                    decoded_calldata: vec![],
                                    raw_calldata: vec![],
                                    num_calls: None,
                                    transaction_version: request.transaction_version,
                                    network: request.chain_id,
                                    sender_address: request.sender_address,
                                    block_number: request.block_number,
                                }),
                            )
                                .into_response();
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to fetch ABI for call {}: {}", call_idx, e);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(CalldataDecoderResponse {
                            decoded_calldata: vec![],
                            raw_calldata: vec![],
                            num_calls: None,
                            transaction_version: request.transaction_version,
                            network: request.chain_id,
                            sender_address: request.sender_address,
                            block_number: request.block_number,
                        }),
                    )
                        .into_response();
                }
            }
        }
    }
    
    let response = CalldataDecoderResponse {
        decoded_calldata: contract_calls,
        raw_calldata,
        num_calls,
        transaction_version: request.transaction_version,
        network: request.chain_id,
        sender_address: request.sender_address,
        block_number: request.block_number,
    };
    
    info!("Successfully processed calldata decode request");
    (StatusCode::OK, Json(response)).into_response()
}
