use crate::app_state::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use starknet::core::types::Felt;
// Using our own DecodedValue struct
use crate::abi_fetcher::fetch_contract_abi;
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};
use utoipa::ToSchema;

#[derive(Deserialize, Debug, Serialize, ToSchema)]
pub struct CalldataDecoderRequest {
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
    pub parameters: Vec<data_decoder::DecodedValue>,
}

// Use existing ABI types from walnut_shared

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

    for _call_idx in 0..num_calls {
        if offset + 2 >= calldata.len() {
            break;
        }

        let contract_address = calldata[offset].to_hex_string();
        let function_selector = calldata[offset + 1].to_hex_string();

        // The next element should be the calldata length for this call
        let call_calldata_len = match calldata[offset + 2].to_usize() {
            Some(len) => len,
            None => {
                break;
            }
        };

        // Check if we have enough data for this call's calldata
        if offset + 3 + call_calldata_len > calldata.len() {
            break;
        }

        // Extract the actual calldata for this call
        let call_calldata = calldata[offset + 3..offset + 3 + call_calldata_len].to_vec();

        calls.push((contract_address, function_selector, call_calldata));

        // Move offset to next call: contract_address + function_selector + calldata_length + actual_calldata
        offset += 3 + call_calldata_len;

        if offset >= calldata.len() {
            break;
        }
    }

    calls
}

/// Decode calldata using ABI and TypeDecoder
fn decode_calldata_with_abi(
    calldata: &[Felt],
    function_selector: &str,
    functions: &[walnut_shared::abi::Function],
    structs: &std::collections::HashMap<String, walnut_shared::abi::Struct>,
    enums: &std::collections::HashMap<String, walnut_shared::abi::Enum>,
) -> Result<(Vec<data_decoder::DecodedValue>, Option<String>), String> {
    // Find function by selector
    let function = functions
        .iter()
        .find(|func| {
            use starknet_api::abi::abi_utils::selector_from_name;
            let calculated_selector = selector_from_name(&func.name).0.to_hex_string();
            calculated_selector == function_selector
        })
        .ok_or_else(|| {
            format!(
                "Function with selector {} not found in ABI",
                function_selector
            )
        })?;

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
    let types_cow: Vec<std::borrow::Cow<str>> = types
        .iter()
        .map(|s| std::borrow::Cow::Borrowed(s.as_str()))
        .collect();
    let names_cow: Vec<std::borrow::Cow<str>> = names
        .iter()
        .map(|s| std::borrow::Cow::Borrowed(s.as_str()))
        .collect();

    // Convert structs and enums to data-decoder format
    let structs_vec: Vec<walnut_shared::abi::Struct> = structs.values().cloned().collect();
    let enums_vec: Vec<walnut_shared::abi::Enum> = enums.values().cloned().collect();

    // Use data-decoder to decode with proper types
    use data_decoder::calldata_decoder::decode_calldata;

    let decoded_values =
        match decode_calldata(calldata, &types_cow, &names_cow, &structs_vec, &enums_vec) {
            Some(values) => values,
            None => {
                error!("decode_calldata returned None - decoding failed");
                return Err("Failed to decode calldata with ABI types".to_string());
            }
        };

    info!("decoded_values: {:?}", decoded_values);

    Ok((decoded_values, Some(function.name.clone())))
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
    let (num_calls, _contract_address, _function_selector) = extract_call_info(&calldata_felts);

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

        for (call_idx, (contract_addr, func_selector, call_calldata)) in
            calls.into_iter().enumerate()
        {
            // Try to fetch ABI and decode with it
            match fetch_contract_abi(&contract_addr, &request.chain_id).await {
                Ok((functions, _type_decoder, structs, enums)) => {
                    match decode_calldata_with_abi(
                        &call_calldata,
                        &func_selector,
                        &functions,
                        &structs,
                        &enums,
                    ) {
                        Ok((decoded, func_name)) => {
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

    (StatusCode::OK, Json(response)).into_response()
}
