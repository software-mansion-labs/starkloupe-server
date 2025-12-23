use anyhow::Result;
use blockifier::execution::{
    deprecated_syscalls::DeprecatedSyscallSelector, syscalls::vm_syscall_utils::SyscallSelector,
};
use cairo_lang_casm::{
    ap_change::ApChange,
    cell_expression::CellExpression,
    hints::{Hint, StarknetHint},
    instructions::Instruction as CasmInstruction,
};
use cairo_lang_sierra::{
    extensions::gas::CostTokenType,
    ids::ConcreteTypeId,
    program::{Program, TypeDeclaration},
};
use cairo_lang_sierra_to_casm::{
    compiler::{CairoProgram, CairoProgramDebugInfo, SierraToCasmConfig},
    metadata::{calc_metadata, MetadataComputationConfig},
};
use cairo_lang_starknet_classes::abi::{Event, EventField};
use cairo_lang_starknet_classes::{abi::EventKind, compiler_version::VersionId};
use cairo_lang_starknet_classes::{
    casm_contract_class::ENTRY_POINT_COST, contract_class::ContractClass,
};
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use data_decoder::{event_decoder::decode_event_datas, DecodedValue, DecodedValueType};
use itertools::chain;
use itertools::Itertools;
use serde::Serialize;
use starknet_api::abi::abi_utils::selector_from_name;
use starknet_rust::core::types::Felt;
use std::collections::HashMap;
use std::collections::HashSet;
use tracing::error;
use walnut_shared::felt252_serde::sierra_from_felt252s;
use walnut_shared::utils::simplify_type_name;

use crate::{
    call_trace::{ContractCall, ESysCall, EventSysCall, StorageWrite},
    mappings::get_value_from_cell_expression,
};

pub fn compile_sierra_contract_class(
    contract_class: &ContractClass,
    max_bytecode_size: usize,
) -> Result<(CairoProgram, VersionId, VersionId)> {
    let (sierra_version, compiler_version, program) =
        sierra_from_felt252s(&contract_class.sierra_program)
            .map_err(|e| anyhow::anyhow!("Failed to parse Sierra program: {:?}", e))?;

    let entrypoint_function_indices = chain!(
        &contract_class.entry_points_by_type.constructor,
        &contract_class.entry_points_by_type.external,
        &contract_class.entry_points_by_type.l1_handler,
    )
    .map(|entrypoint| entrypoint.function_idx);

    let entrypoint_ids: Vec<_> = entrypoint_function_indices
        .map(|idx| program.funcs[idx].id.clone())
        .collect();

    let no_eq_solver = sierra_version.minor >= 4;

    let metadata_computation_config = MetadataComputationConfig {
        function_set_costs: entrypoint_ids
            .into_iter()
            .map(|id| (id, [(CostTokenType::Const, ENTRY_POINT_COST)].into()))
            .collect(),
        linear_gas_solver: no_eq_solver,
        linear_ap_change_solver: no_eq_solver,
        skip_non_linear_solver_comparisons: false,
        compute_runtime_costs: false,
    };

    let metadata = calc_metadata(&program, metadata_computation_config)
        .map_err(|e| anyhow::anyhow!("Failed to calculate metadata: {:?}", e))?;

    let compiled_program = cairo_lang_sierra_to_casm::compiler::compile(
        &program,
        &metadata,
        SierraToCasmConfig {
            gas_usage_check: true,
            max_bytecode_size,
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to compile Sierra to Casm: {:?}", e))?;

    Ok((compiled_program, sierra_version, compiler_version))
}

pub fn make_casm_to_sierra_map(debug_info: &CairoProgramDebugInfo) -> HashMap<usize, Vec<usize>> {
    let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, sierra_info) in debug_info.sierra_statement_info.iter().enumerate() {
        let key = sierra_info.instruction_idx;
        map.entry(key).or_insert_with(Vec::new).push(i);
    }
    map
}

pub fn get_pc_mappings(instructions: &[CasmInstruction]) -> HashMap<usize, usize> {
    let mut pc_to_inst_indexes_map = HashMap::new();
    let mut offset = 1;
    for (i, inst) in instructions.iter().enumerate() {
        pc_to_inst_indexes_map.insert(offset, i);
        offset += inst.body.op_size();
    }
    pc_to_inst_indexes_map
}

// Generates mappings based on the given starknet foundry relocated trace entries and compiled CASM instructions:
// A mapping from PC values (from the starkent relocated trace) to the instruction system call expression.
pub fn get_pc_sys_call_mappings(
    relocated_trace: &[RelocatedTraceEntry],
    casm_instructions: &[CasmInstruction],
) -> HashMap<usize, CellExpression> {
    let mut pc_to_syscalls = HashMap::new();

    // Compute cumulative offsets for each instruction based on its operational size.
    let mut offset = 0;
    let mut offsets = vec![offset];
    for inst in casm_instructions.iter() {
        offset += inst.body.op_size();
        offsets.push(offset);
    }

    for trace_entry in relocated_trace {
        let pc = trace_entry.pc;

        if let Some(index) = find_instruction_index(pc, &offsets) {
            // Map sys calls
            let inst = &casm_instructions[index];
            for hint in &inst.hints {
                // If a system call hint is found, map the trace PC to system call CellExpression.
                if let Hint::Starknet(StarknetHint::SystemCall { system }) = hint {
                    pc_to_syscalls.insert(pc, CellExpression::from_res_operand(system.clone()));
                }
            }
        }
    }

    pc_to_syscalls
}

// Finds the index of the instruction corresponding to a given PC value using the offsets vector.
pub fn find_instruction_index(pc: usize, offsets: &[usize]) -> Option<usize> {
    let index = offsets.partition_point(|&x| x <= pc).saturating_sub(1);

    if index < offsets.len() - 1 && pc < offsets[index + 1] {
        Some(index)
    } else {
        None
    }
}

#[derive(Serialize, Debug)]
pub struct SierraFormattedProgram {
    pub type_declarations: Vec<String>,
    pub libfunc_declarations: Vec<String>,
    pub statements: Vec<String>,
    pub funcs: Vec<String>,
}

pub fn format_sierra_program(sierra_program: Program) -> SierraFormattedProgram {
    SierraFormattedProgram {
        type_declarations: sierra_program
            .type_declarations
            .iter()
            .map(|type_decl| type_decl.to_string())
            .collect(),
        libfunc_declarations: sierra_program
            .libfunc_declarations
            .iter()
            .map(|libfunc_decl| libfunc_decl.to_string())
            .collect(),
        statements: sierra_program
            .statements
            .iter()
            .enumerate()
            .map(|(index, statement)| format!("{} // {}", statement.to_string(), index))
            .collect(),
        funcs: sierra_program
            .funcs
            .iter()
            .map(|func| func.to_string())
            .collect(),
    }
}

pub fn is_panic_result(return_type: Option<&str>) -> bool {
    return_type.map_or(false, |result_type| result_type.contains("PanicResult"))
}

pub fn get_raw_function_name(fn_name: &str) -> Option<String> {
    if fn_name.is_empty() {
        return None;
    }

    let mut raw_fn_name = String::new();
    let mut inside_generics = 0;
    let mut segment = String::new();

    for char in fn_name.chars() {
        match char {
            '<' => {
                inside_generics += 1;
            }
            '>' => {
                if inside_generics > 0 {
                    inside_generics -= 1;
                }
            }
            ':' if inside_generics == 0 => {
                if !segment.is_empty() {
                    if !raw_fn_name.is_empty() {
                        raw_fn_name.push_str("::");
                    }
                    raw_fn_name.push_str(&segment);
                    segment.clear();
                }
            }
            _ if inside_generics == 0 => {
                segment.push(char);
            }
            _ => {}
        }
    }

    if !segment.is_empty() {
        if !raw_fn_name.is_empty() {
            raw_fn_name.push_str("::");
        }
        raw_fn_name.push_str(&segment);
    }

    Some(raw_fn_name)
}

pub fn find_event_by_selector(
    events: &HashSet<Event>,
    selector: Felt,
) -> (Option<String>, Vec<EventField>) {
    for event in events {
        if let EventKind::Enum { variants } = &event.kind {
            for variant in variants {
                if selector_from_name(&variant.name).0 == selector {
                    return find_struct_event_members(events, &variant.ty)
                        .map(|members| (Some(variant.name.clone()), members))
                        .unwrap_or_default();
                }
            }
        }
    }
    (None, Vec::new())
}

fn find_struct_event_members(events: &HashSet<Event>, type_name: &str) -> Option<Vec<EventField>> {
    events.iter().find_map(|struct_ev| {
        if struct_ev.name == type_name {
            if let EventKind::Struct { members } = &struct_ev.kind {
                return Some(
                    members
                        .iter()
                        .map(|field| EventField {
                            name: field.name.clone(),
                            ty: simplify_type_name(&field.ty),
                            kind: field.kind,
                        })
                        .collect(),
                );
            }
        }
        None
    })
}

pub fn flatten_event_data_struct(decoded_values: Vec<DecodedValue>) -> Vec<DecodedValue> {
    decoded_values
        .into_iter()
        .flat_map(|decoded_value| {
            if let DecodedValueType::Struct(fields) = decoded_value.value {
                let flattened_values: Vec<DecodedValue> = fields
                    .into_iter()
                    .sorted_by_key(|(key, _)| *key)
                    .map(|(_, value)| value)
                    .collect();
                flattened_values
            } else {
                vec![decoded_value]
            }
        })
        .collect()
}

pub fn get_system_call_at_trace_step(
    pc_to_ptr_sys_calls: &HashMap<usize, CellExpression>,
    relocated_memory: &[Option<Felt>],
    trace_entry: &RelocatedTraceEntry,
    type_declaration_map: Option<&HashMap<ConcreteTypeId, TypeDeclaration>>,
    events: Option<&HashSet<Event>>,
) -> Option<ESysCall> {
    fn read_felt(memory: &[Option<Felt>], ptr: usize) -> Option<Felt> {
        memory.get(ptr).and_then(|v| *v)
    }

    let pc = trace_entry.pc;
    let ptr_sys_call = pc_to_ptr_sys_calls.get(&pc)?;

    let value = get_value_from_cell_expression(
        relocated_memory,
        trace_entry,
        ptr_sys_call,
        &ApChange::Known(0),
    )
    .ok()?;
    let ptr = value.to_string().parse::<usize>().ok()?;

    let felt_value = read_felt(relocated_memory, ptr)?;
    let selector = SyscallSelector::try_from(felt_value).ok()?;

    match selector {
        DeprecatedSyscallSelector::CallContract => {
            let contract_address = read_felt(relocated_memory, ptr + 2)?.to_fixed_hex_string();
            let function_selector = read_felt(relocated_memory, ptr + 3)?.to_fixed_hex_string();

            Some(ESysCall::ContractCall(ContractCall {
                contract_address,
                function_selector,
            }))
        }

        DeprecatedSyscallSelector::EmitEvent => {
            let (type_declaration_map, events) = (type_declaration_map?, events?);

            let id = read_felt(relocated_memory, ptr + 2)?
                .to_string()
                .parse::<usize>()
                .ok()?;

            let event_selector = read_felt(relocated_memory, id)?;
            let (event_name, event_members) = find_event_by_selector(events, event_selector);

            let concrete_type_id = type_declaration_map.keys().find_map(|concrete_id| {
                match (concrete_id.debug_name.as_ref(), event_name.as_ref()) {
                    (Some(name), Some(event_name)) if simplify_type_name(name) == *event_name => {
                        Some(concrete_id.clone())
                    }
                    _ => None,
                }
            })?;

            let values: Vec<Felt> = relocated_memory
                .iter()
                .skip(id + 1)
                .filter_map(|x| *x)
                .collect();

            let decoded_event =
                decode_event_datas(&concrete_type_id, type_declaration_map, &values, &mut 0)?;

            Some(ESysCall::EventCall(EventSysCall {
                event_selector,
                event_name,
                event_members,
                event_datas: vec![decoded_event],
            }))
        }

        DeprecatedSyscallSelector::StorageWrite => {
            let address = read_felt(relocated_memory, ptr + 3)?;
            let value = read_felt(relocated_memory, ptr + 4)?;
            Some(ESysCall::StorageWrite(StorageWrite { address, value }))
        }

        _ => None,
    }
}

pub fn get_system_call_at_trace_step_(
    pc_to_ptr_sys_calls: &HashMap<usize, CellExpression>,
    relocated_memory: &[Option<Felt>],
    trace_entry: &RelocatedTraceEntry,
    type_declaration_map: Option<&HashMap<ConcreteTypeId, TypeDeclaration>>,
    events: Option<&HashSet<Event>>,
) -> Option<ESysCall> {
    let pc = trace_entry.pc;
    let ptr_sys_call = pc_to_ptr_sys_calls.get(&pc);
    match ptr_sys_call {
        Some(ptr_sys_call) => {
            let value = get_value_from_cell_expression(
                relocated_memory,
                trace_entry,
                ptr_sys_call,
                &ApChange::Known(0),
            )
            .unwrap();
            let mut ptr = value.to_string().parse::<usize>().unwrap();
            let felt_value = relocated_memory.get(ptr).unwrap();
            ptr += 1;
            let selector = SyscallSelector::try_from(felt_value.unwrap());
            match selector {
                Ok(selector) => {
                    match selector {
                        DeprecatedSyscallSelector::CallContract => {
                            // https://github.com/starkware-libs/blockifier/blob/9bfb3d4c8bf1b68a0c744d1249b32747c75a4d87/crates/blockifier/src/execution/syscalls/hint_processor.rs#L302C13-L306C15
                            let _felt_value = relocated_memory.get(ptr).unwrap();
                            ptr += 1;
                            let felt_value: Felt = (*relocated_memory.get(ptr).unwrap()).unwrap();
                            let contract_address = felt_value.to_fixed_hex_string();
                            ptr += 1;
                            let felt_value = (*relocated_memory.get(ptr).unwrap()).unwrap();
                            let function_selector = felt_value.to_fixed_hex_string();
                            let contract_call = ContractCall {
                                contract_address,
                                function_selector,
                            };
                            Some(ESysCall::ContractCall(contract_call))
                        }
                        DeprecatedSyscallSelector::EmitEvent => {
                            if let (Some(type_declaration_map), Some(events)) =
                                (type_declaration_map, events)
                            {
                                let _felt_value = relocated_memory.get(ptr).unwrap();

                                ptr += 1;
                                let felt_value: Felt =
                                    (*relocated_memory.get(ptr).unwrap()).unwrap();

                                let id = match felt_value.to_string().parse::<usize>() {
                                    Ok(parsed_id) => parsed_id,
                                    Err(_) => {
                                        error!("Error: id is None, skipping event processing.");
                                        return None;
                                    }
                                };
                                let event_selector = relocated_memory
                                    .get(id)
                                    .and_then(|v| v.as_ref().copied())
                                    .or_else(|| {
                                        error!("Error: event_selector not found at index");
                                        None
                                    });
                                if let Some(event_selector) = event_selector {
                                    let (event_name, event_members) =
                                        find_event_by_selector(&events, event_selector);
                                    let conrete_type_id =
                                        type_declaration_map.keys().find_map(|concrete_id| {
                                            if let Some(name) = concrete_id.debug_name.as_ref() {
                                                if let Some(event_name) = event_name.as_ref() {
                                                    if &simplify_type_name(name.as_str())
                                                        == event_name
                                                    {
                                                        return Some(concrete_id.clone());
                                                    }
                                                }
                                            }
                                            None
                                        });
                                    if let Some(conrete_type_id) = conrete_type_id {
                                        let values: Vec<Felt> = relocated_memory
                                            .iter()
                                            .skip(id + 1)
                                            .filter_map(|x| *x)
                                            .collect();

                                        if let Some(decoded_event) = decode_event_datas(
                                            &conrete_type_id,
                                            &type_declaration_map,
                                            &values,
                                            &mut 0,
                                        ) {
                                            let event_sys_call = EventSysCall {
                                                event_selector,
                                                event_name,
                                                event_members,
                                                event_datas: vec![decoded_event],
                                            };

                                            return Some(ESysCall::EventCall(event_sys_call));
                                        }
                                    }
                                }
                            }
                            None
                        }
                        DeprecatedSyscallSelector::StorageWrite => {
                            let _felt_value: Felt = (*relocated_memory.get(ptr).unwrap()).unwrap();
                            ptr += 1;
                            let _felt_value: Felt = (*relocated_memory.get(ptr).unwrap()).unwrap(); // Should be 0
                            ptr += 1;
                            let address = (*relocated_memory.get(ptr).unwrap()).unwrap();
                            ptr += 1;
                            let value = (*relocated_memory.get(ptr).unwrap()).unwrap();
                            let storage_write = StorageWrite { address, value };
                            Some(ESysCall::StorageWrite(storage_write))
                        }
                        _ => None,
                    }
                }
                Err(e) => {
                    dbg!(e);
                    None
                }
            }
        }

        None => None,
    }
}
