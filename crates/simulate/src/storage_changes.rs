use blockifier::state::cached_state::StorageEntry;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use internal_tracing::{
    call_trace::ESysCall,
    utils::{
        compile_sierra_contract_class, get_pc_sys_call_mappings, get_system_call_at_trace_step,
    },
};
use starknet::core::types::Felt;
use starknet_api::{core::ContractAddress, state::StorageKey};
use starknet_old::core::types as starknet_old_types;
use starknet_providers::{
    jsonrpc::{HttpTransport, JsonRpcClient},
    Provider,
};
use std::collections::HashMap;
use tracing::warn;
use walnut_shared::{felt_to_field_element, field_element_to_felt, hex_string_to_field_element};

use crate::contract_calls_map::ContractCallsMap;

pub fn get_storage_changes(
    contract_class: starknet::core::types::ContractClass,
    vm_trace: &[RelocatedTraceEntry],
    vm_memory: &[Option<Felt>],
    storage_view: &HashMap<StorageEntry, Felt>,
    contract_address: ContractAddress,
) -> Option<HashMap<String, (Option<String>, String)>> {
    match contract_class {
        starknet::core::types::ContractClass::Sierra(ref class) => {
            let cloned_class = class.clone();
            let contract_class = cairo_lang_starknet_classes::contract_class::ContractClass {
                sierra_program: cloned_class
                    .sierra_program
                    .iter()
                    .map(|felt| felt.to_biguint().into())
                    .collect(),
                sierra_program_debug_info: None,
                contract_class_version: cloned_class.contract_class_version,
                entry_points_by_type:
                    cairo_lang_starknet_classes::contract_class::ContractEntryPoints {
                        external: cloned_class
                            .entry_points_by_type
                            .external
                            .iter()
                            .map(|entry_point| {
                                cairo_lang_starknet_classes::contract_class::ContractEntryPoint {
                                    selector: entry_point.selector.to_biguint(),
                                    function_idx: entry_point.function_idx as usize,
                                }
                            })
                            .collect(),
                        l1_handler: cloned_class
                            .entry_points_by_type
                            .l1_handler
                            .iter()
                            .map(|entry_point| {
                                cairo_lang_starknet_classes::contract_class::ContractEntryPoint {
                                    selector: entry_point.selector.to_biguint(),
                                    function_idx: entry_point.function_idx as usize,
                                }
                            })
                            .collect(),
                        constructor: cloned_class
                            .entry_points_by_type
                            .constructor
                            .iter()
                            .map(|entry_point| {
                                cairo_lang_starknet_classes::contract_class::ContractEntryPoint {
                                    selector: entry_point.selector.to_biguint(),
                                    function_idx: entry_point.function_idx as usize,
                                }
                            })
                            .collect(),
                    },
                abi: None,
            };
            let casm_program = compile_sierra_contract_class(contract_class, usize::MAX)
                .map_err(|e| {
                    warn!("Failed to compile sierra contract class: {:?}", e);
                    e
                })
                .unwrap();

            let pc_to_ptr_sys_calls =
                get_pc_sys_call_mappings(vm_trace, &casm_program.instructions);

            let mut storage_changes: HashMap<String, (Option<String>, String)> = HashMap::new();
            for trace_entry in vm_trace {
                let syscall = get_system_call_at_trace_step(
                    &pc_to_ptr_sys_calls,
                    vm_memory,
                    trace_entry,
                    None,
                    None,
                );
                match syscall {
                    Some(ESysCall::StorageWrite(storage_write)) => {
                        let before = storage_view
                            .get(&(
                                contract_address,
                                StorageKey(storage_write.address.try_into().unwrap()),
                            ))
                            .cloned();
                        storage_changes.insert(
                            storage_write.address.to_hex_string(),
                            (
                                before.map(|before| before.to_hex_string()),
                                storage_write.value.to_hex_string(),
                            ),
                        );
                    }
                    _ => {}
                }
            }

            Some(storage_changes)
        }
        _ => None,
    }
}

pub async fn fetch_before_storage_changes(
    mut storage_changes: HashMap<u32, HashMap<String, (Option<String>, String)>>,
    contract_calls_map: &ContractCallsMap,
    provider_client: &JsonRpcClient<HttpTransport>,
    block_number: u64,
) -> HashMap<u32, HashMap<String, (String, String)>> {
    let mut filtered_storage_changes = HashMap::new();
    for (call_id, changes) in storage_changes.iter_mut() {
        for (address, (before, _after)) in changes.iter_mut() {
            if before.is_none() {
                let contract_address_felt = Felt::from(
                    contract_calls_map
                        .0
                        .get(call_id)
                        .unwrap()
                        .entry_point
                        .storage_address,
                );
                let before_from_provider = provider_client
                    .get_storage_at(
                        felt_to_field_element(contract_address_felt),
                        hex_string_to_field_element(address).unwrap(),
                        starknet_old_types::BlockId::Number(block_number),
                    )
                    .await
                    .ok();
                *before = before_from_provider
                    .map(|before| field_element_to_felt(before).to_hex_string());
            }
        }
        let mut filtered_contract_storage_changes = HashMap::new();
        for (address, (before, after)) in changes.iter() {
            if let Some(before) = before {
                if before != after {
                    filtered_contract_storage_changes
                        .insert(address.clone(), (before.clone(), after.clone()));
                }
            }
        }
        if !filtered_contract_storage_changes.is_empty() {
            filtered_storage_changes.insert(*call_id, filtered_contract_storage_changes);
        }
    }
    filtered_storage_changes
}
