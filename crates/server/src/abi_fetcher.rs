use data_decoder::type_decoder::{
    expand_enums_recursively, expand_structs_recursively, TypeDecoder,
};
use starknet::core::types::{BlockId, BlockTag, ContractClass, Felt};
use starknet::providers::Provider;
use walnut_shared::abi::{get_enums, get_functions, get_structs, Item};
use walnut_shared::utils::simplify_type_name;
use walnut_shared::{extract_chain_id, get_rpc_urls};

/// Fetch contract ABI for encoding/decoding purposes
pub async fn fetch_contract_abi(
    contract_address: &str,
    chain_id: &str,
) -> Result<
    (
        Vec<walnut_shared::abi::Function>,
        TypeDecoder,
        std::collections::HashMap<String, walnut_shared::abi::Struct>,
        std::collections::HashMap<String, walnut_shared::abi::Enum>,
    ),
    String,
> {
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
                    let struct_map: std::collections::HashMap<String, walnut_shared::abi::Struct> =
                        structs
                            .into_iter()
                            .map(|s| (simplify_type_name(&s.name), s))
                            .collect();
                    let enum_map: std::collections::HashMap<String, walnut_shared::abi::Enum> =
                        enums
                            .into_iter()
                            .map(|e| (simplify_type_name(&e.name), e))
                            .collect();

                    // Recursively enhance all structs and enums with their members/variants
                    let expanded_struct_map = expand_structs_recursively(&struct_map, &enum_map);
                    let expanded_enum_map =
                        expand_enums_recursively(&enum_map, &expanded_struct_map);

                    // Create TypeDecoder with complete type information
                    let type_decoder =
                        TypeDecoder::new(expanded_struct_map.clone(), expanded_enum_map.clone());

                    Ok((
                        functions,
                        type_decoder,
                        expanded_struct_map,
                        expanded_enum_map,
                    ))
                }
                Err(err) => Err(format!("Failed to parse ABI: {}", err)),
            }
        }
        ContractClass::Legacy(_) => {
            Err("Legacy contracts not supported for ABI decoding".to_string())
        }
    }
}
