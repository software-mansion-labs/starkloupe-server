use anyhow::Result;
use blockifier::execution::{
    deprecated_syscalls::DeprecatedSyscallSelector, syscalls::SyscallSelector,
};
use cairo_lang_casm::{
    ap_change::ApChange,
    cell_expression::{CellExpression, CellOperator},
    hints::{Hint, StarknetHint},
    operand::{CellRef, DerefOrImmediate, Register},
};
use cairo_lang_sierra::{
    extensions::core::{CoreLibfunc, CoreType},
    ids::ConcreteTypeId,
    program::{GenFunction, Program, StatementIdx, TypeDeclaration},
    program_registry::ProgramRegistry,
};
use cairo_lang_sierra_to_casm::compiler::{SierraStatementDebugInfo, StatementKindDebugInfo};
use cairo_lang_sierra_type_size::{get_type_size_map, TypeSizeMap};
use cairo_lang_starknet_classes::{
    abi::{Event, Item},
    contract_class::ContractClass,
};
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use data_decoder::internal_function_decoder::decode_internal_datas;
use data_decoder::utils::skip_builtin_type_declaration;
use data_decoder::{create_decoded_value_by_type, event_decoder::decode_event_datas};
use data_decoder::{DecodedValue, DecodedValueType};
use indexmap::IndexSet;
use num_bigint::BigInt;
use num_traits::cast::ToPrimitive;
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use tracing::error;
use tracing::{debug, warn};
use verification::{CodeLocation, SierraStatementToCairoDebugInfo};
use walnut_shared::felts_to_string;
use walnut_shared::utils::simplify_type_name;

use crate::{
    call_trace::{ContractCall, ESysCall, EventSysCall, InternalFnCallIO},
    utils::{
        compile_sierra_contract_class, find_event_by_selector, get_pc_mappings, is_panic_result,
        make_casm_to_sierra_map,
    },
};
use starknet_types_core::felt::{Felt, NonZeroFelt};

#[derive(Debug)]
pub struct Mappings {
    pub pc_to_inst_indexes_map: HashMap<usize, usize>,
    pub pc_to_ptr_sys_calls: HashMap<usize, CellExpression>,
    pub casm_to_sierra_map: HashMap<usize, Vec<usize>>,
    pub sierra_program: Program,
    pub memory_map: HashMap<usize, BigInt>,
    pub type_sizes: TypeSizeMap,
    pub type_names: HashMap<ConcreteTypeId, SmolStr>,
    pub sierra_statement_info: Vec<SierraStatementDebugInfo>,
    pub type_declaration_map: HashMap<ConcreteTypeId, TypeDeclaration>,
    pub events: HashSet<Event>,
}

impl Mappings {
    pub fn new(
        relocated_trace_entry: &[RelocatedTraceEntry],
        relocated_memory: &[Option<Felt>],
        contract_class: ContractClass,
    ) -> Result<Self> {
        let sierra_program = contract_class.extract_sierra_program().map_err(|e| {
            warn!("Failed to extract sierra program: {:?}", e);
            e
        })?;
        //dbg!(&sierra_program.type_declarations);
        //dbg!(format_sierra_program(sierra_program.clone()));
        let type_names = contract_class
            .sierra_program_debug_info
            .clone()
            .unwrap()
            .type_names;

        let events: HashSet<Event> = contract_class
            .abi
            .as_ref()
            .map(|contract| {
                contract
                    .clone()
                    .into_iter()
                    .filter_map(|item| match item {
                        Item::Event(event) => Some(event),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let casm_program =
            compile_sierra_contract_class(contract_class, usize::MAX).map_err(|e| {
                warn!("Failed to compile sierra contract class: {:?}", e);
                e
            })?;

        let casm_to_sierra_map = make_casm_to_sierra_map(&casm_program.debug_info);
        let (pc_to_inst_indexes_map, pc_to_ptr_sys_calls) =
            get_pc_mappings(relocated_trace_entry, &casm_program.instructions);
        let memory_map: HashMap<usize, BigInt> = relocated_memory
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_ref().map(|v| (i + 1, v.to_bigint())))
            .collect();
        let sierra_program_registry: ProgramRegistry<CoreType, CoreLibfunc> =
            ProgramRegistry::<CoreType, CoreLibfunc>::new(&sierra_program).map_err(|e| {
                warn!("Failed to create sierra program registry: {:?}", e);
                e
            })?;
        let type_sizes =
            get_type_size_map(&sierra_program, &sierra_program_registry).unwrap_or_default();
        let type_declaration_map: HashMap<ConcreteTypeId, TypeDeclaration> = sierra_program
            .type_declarations
            .iter()
            .map(|declaration| (declaration.id.clone(), declaration.clone()))
            .collect();
        // // Print relocated memory
        // let mut ordered_map: BTreeMap<usize, BigInt> = BTreeMap::new();
        // for (k, v) in &memory_map {
        //     ordered_map.insert(*k, v.clone());
        // }

        // for (k, v) in &ordered_map {
        //     println!("{}: {}", k, v);
        // }
        // // Print VM trace
        // dbg!(&vm_trace);

        Ok(Mappings {
            pc_to_inst_indexes_map,
            pc_to_ptr_sys_calls,
            casm_to_sierra_map,
            sierra_program,
            memory_map,
            type_sizes,
            type_names,
            sierra_statement_info: casm_program.debug_info.sierra_statement_info,
            type_declaration_map,
            events,
        })
    }

    pub fn get_sierra_indexes_at_pc(&self, pc: &usize) -> Option<Vec<usize>> {
        let casm_index = match self.pc_to_inst_indexes_map.get(pc) {
            Some(index) => index,
            None => {
                debug!("Failed to get casm index for pc: {}", pc);
                return None;
            }
        };
        self.casm_to_sierra_map.get(casm_index).cloned()
    }

    pub fn get_first_sierra_index_at_pc(&self, pc: &usize) -> Option<usize> {
        let casm_index = match self.pc_to_inst_indexes_map.get(pc) {
            Some(index) => index,
            None => {
                debug!("Failed to get casm index for pc: {}", pc);
                return None;
            }
        };
        self.casm_to_sierra_map
            .get(casm_index)
            .map_or(None, |sierra_indexes| sierra_indexes.first().cloned())
    }

    pub fn get_cairo_locations_at_sierra_indexes(
        &self,
        sierra_statements_to_cairo_info: &HashMap<usize, SierraStatementToCairoDebugInfo>,
        sierra_indexes: &Vec<usize>,
    ) -> Vec<CodeLocation> {
        let mut locations_set = HashSet::new();
        for sierra_index in sierra_indexes {
            if let Some(cairo_info) = sierra_statements_to_cairo_info.get(&sierra_index) {
                locations_set.extend(cairo_info.cairo_locations.clone());
            }
        }
        locations_set.into_iter().collect()
    }

    pub fn get_cairo_locations_at_sierra_index(
        &self,
        sierra_statements_to_cairo_info: &HashMap<usize, SierraStatementToCairoDebugInfo>,
        sierra_index: usize,
    ) -> Vec<CodeLocation> {
        let mut locations_set = IndexSet::new();
        if let Some(cairo_info) = sierra_statements_to_cairo_info.get(&sierra_index) {
            locations_set.extend(cairo_info.cairo_locations.clone());
        }
        locations_set.into_iter().collect()
    }

    pub fn get_sierra_execution_trace(
        &self,
        vm_trace: &Vec<RelocatedTraceEntry>,
    ) -> Vec<Vec<usize>> {
        let mut sierra_trace: Vec<Vec<usize>> = vec![];
        for trace_entry in vm_trace {
            if let Some(sierra_indexes) = self.get_sierra_indexes_at_pc(&trace_entry.pc) {
                sierra_trace.push(sierra_indexes);
            }
        }
        sierra_trace
    }

    pub fn get_sierra_function_at_sierra_index<'a>(
        &'a self,
        sierra_index: &usize,
    ) -> Option<&'a GenFunction<StatementIdx>> {
        self.sierra_program
            .funcs
            .iter()
            .find(|&func| func.entry_point.0 == *sierra_index)
    }

    pub fn get_arguments_at_trace_step(
        &self,
        relocated_memory: &[Option<Felt>],
        sierra_index: usize,
        trace_entry: &RelocatedTraceEntry,
    ) -> (Vec<InternalFnCallIO>, Vec<DecodedValue>) {
        let mut arguments: Vec<InternalFnCallIO> = vec![];
        let mut arguments_decoded: Vec<DecodedValue> = Vec::new();
        if let Some(sierra_statement_info) = self.sierra_statement_info.get(sierra_index) {
            if let StatementKindDebugInfo::Invoke(invoke_info) =
                &sierra_statement_info.additional_kind_info
            {
                for invoke_ref in invoke_info.ref_values.iter() {
                    let values = get_values_from_cell_expressions(
                        relocated_memory,
                        trace_entry,
                        &invoke_ref.expression.cells,
                        &ApChange::Known(0),
                    );
                    let type_names = self
                        .type_names
                        .get(&invoke_ref.ty)
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    let simplified_type_name = simplify_type_name(type_names.as_str());
                    if !skip_builtin_type_declaration(simplified_type_name.as_str()) {
                        arguments.push(InternalFnCallIO {
                            type_name: Some(simplified_type_name.clone()),
                            value: values.iter().map(|v| v.to_string()).collect(),
                        });
                    }
                    let type_id = &invoke_ref.ty;
                    let mut data_index: usize = 0;
                    if let Some(argument_decoded) = decode_internal_datas(
                        &values,
                        type_id,
                        &self.type_declaration_map,
                        &self.type_sizes,
                        relocated_memory,
                        &mut data_index,
                    ) {
                        arguments_decoded.push(argument_decoded);
                    }
                }

                // for (branch_index, branch_change) in
                //     invoke_info.result_branch_changes.iter().enumerate()
                // {
                //     for (output_reference_index, output_reference_value) in
                //         branch_change.refs.iter().enumerate()
                //     {
                //         let values = get_values_from_cell_expressions(
                //             relocated_memory,
                //             trace_entry,
                //             &output_reference_value.expression.cells,
                //             &branch_change.ap_change,
                //         );
                //         dbg!(values);
                //     }
                // }
                // StatementKindDebugInfo::Return(return_info) => {
                //     for (_return_ref_index, return_ref) in return_info.ref_values.iter().enumerate()
                //     {
                //         let values = get_values_from_cell_expressions(
                //             relocated_memory,
                //             trace_entry,
                //             &return_ref.expression.cells,
                //             &ApChange::Known(0),
                //         );
                //         results.push(InternalFnCallIO {
                //             type_name: self
                //                 .type_names
                //                 .get(&return_ref.ty)
                //                 .clone()
                //                 .map(|n| n.to_string()),
                //             value: values,
                //         })
                //     }
                // }
            }
        }
        (arguments, arguments_decoded)
    }

    pub fn get_results_at_trace_step(
        &self,
        relocated_memory: &[Option<Felt>],
        sierra_index: usize,
        trace_entry: &RelocatedTraceEntry,
    ) -> (Vec<InternalFnCallIO>, Vec<DecodedValue>) {
        let mut results: Vec<InternalFnCallIO> = vec![];
        let mut results_decoded: Vec<DecodedValue> = Vec::new();
        if let Some(sierra_statement_info) = self.sierra_statement_info.get(sierra_index) {
            if let StatementKindDebugInfo::Return(return_info) =
                &sierra_statement_info.additional_kind_info
            {
                for return_ref in return_info.ref_values.iter() {
                    let values = get_values_from_cell_expressions(
                        relocated_memory,
                        trace_entry,
                        &return_ref.expression.cells,
                        &ApChange::Known(0),
                    );

                    let result_type = self
                        .type_names
                        .get(&return_ref.ty)
                        .map(|n| n.to_string())
                        .unwrap_or_default();

                    let simplified_type_name = simplify_type_name(result_type.as_str());

                    if is_panic_result(Some(&simplified_type_name)) && values[0] == Felt::ONE {
                        let index = match values[1].to_string().parse::<usize>() {
                            Ok(i) => i,
                            Err(e) => {
                                error!("Failed to parse panic index: {}", e);
                                return (results, results_decoded);
                            }
                        };

                        match relocated_memory[index] {
                            Some(panic_data) => {
                                let decoded = felts_to_string(&[panic_data]);
                                let panic_str = decoded.first().cloned().unwrap_or_default();

                                results.push(InternalFnCallIO {
                                    type_name: Some(simplified_type_name.clone()),
                                    value: vec!["1".to_string(), panic_str.clone()],
                                });

                                results_decoded.push(create_decoded_value_by_type(
                                    None,
                                    "Panic",
                                    DecodedValueType::String(panic_str),
                                ));
                            }
                            None => {
                                error!(
                                    "Missing panic data at index {} (memory value: {})",
                                    index,
                                    relocated_memory[index].unwrap_or(Felt::ZERO)
                                );
                            }
                        }
                    } else {
                        let type_id = &return_ref.ty;
                        let mut data_index: usize = 0;
                        if let Some(decoded_element) = decode_internal_datas(
                            &values,
                            type_id,
                            &self.type_declaration_map,
                            &self.type_sizes,
                            relocated_memory,
                            &mut data_index,
                        ) {
                            if let Some(adjusted_element) = adjust_decoded_element(decoded_element)
                            {
                                results_decoded.push(adjusted_element);
                            }
                        }
                        if !skip_builtin_type_declaration(simplified_type_name.as_str()) {
                            results.push(InternalFnCallIO {
                                type_name: Some(simplified_type_name.clone()),
                                value: values.iter().map(|v| v.to_string()).collect(),
                            });
                        }
                    }
                }
            }
        }
        (results, results_decoded)
    }

    pub fn get_system_call_at_trace_step(
        &self,
        relocated_memory: &[Option<Felt>],
        trace_entry: &RelocatedTraceEntry,
    ) -> Option<ESysCall> {
        let pc = trace_entry.pc;
        let ptr_sys_call = self.pc_to_ptr_sys_calls.get(&pc);
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
                                let felt_value: Felt =
                                    (*relocated_memory.get(ptr).unwrap()).unwrap();
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
                                        find_event_by_selector(&self.events, event_selector);
                                    let conrete_type_id =
                                        self.type_declaration_map.keys().find_map(|concrete_id| {
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
                                            &self.type_declaration_map,
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
                                None
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
}

pub fn adjust_decoded_element(mut decoded_element: DecodedValue) -> Option<DecodedValue> {
    while let DecodedValueType::Enum(_, inner) = decoded_element.value {
        decoded_element = *inner;
    }
    match &decoded_element.value {
        // Remove "Unit", "ContractState", or "ComponentState" elements
        DecodedValueType::Struct(fields) if decoded_element.type_name.starts_with('(') => {
            let mut new_fields = HashMap::new();
            for (key, value) in fields {
                // Exclude fields with type_name "Unit", "ContractState", or starting with "ComponentState"
                if value.type_name != "Unit"
                    && value.type_name != "ContractState"
                    && !value.type_name.contains("ComponentState")
                {
                    new_fields.insert(*key, value.clone());
                }
            }

            if new_fields.len() == 1 {
                return new_fields.into_values().next();
            }
            if new_fields.is_empty() {
                return None;
            }
            return Some(DecodedValue {
                name: decoded_element.name.clone(),
                type_name: decoded_element.type_name.clone(),
                value: DecodedValueType::Struct(new_fields),
            });
        }

        // Unwrap nested structures with the same type name
        DecodedValueType::Struct(fields) if fields.len() == 1 => {
            let (_, nested_value) = fields.iter().next().unwrap();
            if nested_value.type_name == decoded_element.type_name {
                return Some(nested_value.clone());
            }
        }

        // Exclude ComponentState<ContractState> from results
        _ if decoded_element.type_name.contains("ComponentState") => {
            return None;
        }

        // If the type_name is "Unit" and it's not part of a tuple, exclude it
        _ if decoded_element.type_name == "Unit" => {
            return None;
        }

        // Otherwise, return the original decoded_element
        _ => {}
    }

    Some(decoded_element)
}

pub fn get_values_from_cell_expressions(
    memory: &[Option<Felt>],
    trace_entry: &RelocatedTraceEntry,
    cell_expressions: &Vec<CellExpression>,
    ap_change: &ApChange,
) -> Vec<Felt> {
    let mut value_vec: Vec<Felt> = Vec::new();
    for cell_expression in cell_expressions {
        let value = get_value_from_cell_expression(memory, trace_entry, cell_expression, ap_change);
        match value {
            Ok(value) => {
                value_vec.push(value);
            }
            Err(e) => match e {
                GetCellRefValueError::UnknownApChange => {}
                _ => {
                    dbg!(e);
                }
            },
        }
    }
    value_vec
}

#[derive(Debug)]
pub enum GetCellRefValueError {
    UnknownApChange,
    MemoryAddressNotFound,
    OtherError(String),
}

pub fn get_cell_ref_value(
    memory: &[Option<Felt>],
    trace_entry: &RelocatedTraceEntry,
    cell_ref: &CellRef,
    ap_change: &ApChange,
) -> Result<Felt, GetCellRefValueError> {
    match cell_ref.register {
        Register::AP => match ap_change {
            ApChange::Known(ap_change_value) => {
                let ap: i32 = trace_entry.ap as i32;
                let addr = ap + cell_ref.offset as i32 + *ap_change_value as i32;
                memory[addr as usize]
                    .clone()
                    .ok_or(GetCellRefValueError::MemoryAddressNotFound)
            }
            ApChange::Unknown => Err(GetCellRefValueError::UnknownApChange),
        },
        Register::FP => {
            let fp: i32 = trace_entry.fp as i32;
            let addr = fp + cell_ref.offset as i32;
            memory[addr as usize]
                .clone()
                .ok_or(GetCellRefValueError::MemoryAddressNotFound)
        }
    }
}

pub fn get_value_from_cell_expression(
    memory: &[Option<Felt>],
    trace_entry: &RelocatedTraceEntry,
    cell_expression: &CellExpression,
    ap_change: &ApChange,
) -> Result<Felt, GetCellRefValueError> {
    match cell_expression {
        CellExpression::Deref(cell_ref) => {
            get_cell_ref_value(memory, trace_entry, cell_ref, ap_change)
        }
        CellExpression::Immediate(imm) => Ok(Felt::from(imm.clone())),
        CellExpression::DoubleDeref(cell_ref, offset) => {
            match get_cell_ref_value(memory, trace_entry, cell_ref, ap_change) {
                Ok(cell_ref_value_felt) => {
                    let cell_ref_value =
                        cell_ref_value_felt
                            .to_i128()
                            .ok_or(GetCellRefValueError::OtherError(
                                "Failed to convert cell_ref_value_felt to i128".to_string(),
                            ))?;
                    let addr = cell_ref_value + *offset as i128;
                    let value = memory.get(addr as usize).cloned();
                    if let Some(Some(value)) = value {
                        Ok(value)
                    } else {
                        Err(GetCellRefValueError::MemoryAddressNotFound)
                    }
                }
                Err(e) => Err(e),
            }
        }
        CellExpression::BinOp { op, a, b } => {
            let a = get_cell_ref_value(memory, trace_entry, a, ap_change);
            match a {
                Ok(a) => {
                    let b = match b {
                        DerefOrImmediate::Deref(cell) => {
                            get_cell_ref_value(memory, trace_entry, cell, ap_change)
                        }
                        DerefOrImmediate::Immediate(x) => Ok(Felt::from(&x.value)),
                    };

                    match b {
                        Ok(b) => {
                            let value: Result<Felt, GetCellRefValueError> = match op {
                                CellOperator::Add => Ok(a + b),
                                CellOperator::Mul => Ok(a * b),
                                CellOperator::Div => {
                                    let non_zero_b: Result<NonZeroFelt, _> = b.try_into();
                                    match non_zero_b {
                                        Ok(non_zero_b) => Ok(a.div_rem(&non_zero_b).0),
                                        Err(_) => Err(GetCellRefValueError::OtherError(
                                            "Division by zero".to_string(),
                                        )),
                                    }
                                }
                                CellOperator::Sub => Ok(a - b),
                            };
                            match value {
                                Ok(value) => Ok(value),
                                Err(e) => Err(e),
                            }
                        }
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e),
            }
        }
    }
}
