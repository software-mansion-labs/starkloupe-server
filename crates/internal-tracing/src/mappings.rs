use anyhow::Result;
use blockifier::execution::{
    deprecated_syscalls::DeprecatedSyscallSelector, syscalls::SyscallSelector,
};
use cairo_lang_casm::{
    ap_change::ApChange,
    cell_expression::{CellExpression, CellOperator},
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
use cairo_lang_starknet_classes::contract_class::ContractClass;
use cairo_vm::vm::trace::trace_entry::RelocatedTraceEntry;
use data_decoder::internal_function_decoder::decode_internal_datas;
use data_decoder::utils::{simplify_type_name, skip_builtin_type_declaration};
use data_decoder::{create_decoded_value, DecodedValue, DecodedValueType};
use indexmap::IndexSet;
use num_bigint::BigInt;
use num_traits::cast::ToPrimitive;
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use verification::{CodeLocation, SierraStatementToCairoDebugInfo};
use walnut_shared::decode_felt;

use crate::{
    call_trace::{ContractCall, ESysCall, InternalFnCallIO},
    utils::{
        compile_sierra_contract_class, get_pc_mappings, get_pc_to_ptr_sys_call_mappings,
        is_panic_result, make_casm_to_sierra_map,
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
}

impl Mappings {
    pub fn new(
        relocated_memory: &Vec<Option<Felt>>,
        vm_trace: &Vec<RelocatedTraceEntry>,
        contract_class: ContractClass,
    ) -> Result<Self> {
        let sierra_program = contract_class.extract_sierra_program()?;
        //dbg!(&sierra_program.type_declarations);
        //dbg!(format_sierra_program(sierra_program.clone()));
        let type_names = contract_class
            .sierra_program_debug_info
            .clone()
            .unwrap()
            .type_names;
        let casm_program = compile_sierra_contract_class(contract_class, usize::MAX)?;
        let casm_to_sierra_map = make_casm_to_sierra_map(&casm_program.debug_info);
        let (_pc_inst_map, pc_to_inst_indexes_map) = get_pc_mappings(relocated_memory, vm_trace)?;

        let pc_to_ptr_sys_calls =
            get_pc_to_ptr_sys_call_mappings(&casm_program.instructions, &pc_to_inst_indexes_map);
        let memory_map: HashMap<usize, BigInt> = relocated_memory
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_ref().map(|v| (i + 1, v.to_bigint())))
            .collect();
        let sierra_program_registry: ProgramRegistry<CoreType, CoreLibfunc> =
            ProgramRegistry::<CoreType, CoreLibfunc>::new(&sierra_program).unwrap();
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
        })
    }

    pub fn get_sierra_indexes_at_pc(&self, pc: &usize) -> Option<Vec<usize>> {
        let casm_index = self
            .pc_to_inst_indexes_map
            .get(pc)
            .expect("Failed to get casm index");
        self.casm_to_sierra_map.get(casm_index).cloned()
    }

    pub fn get_first_sierra_index_at_pc(&self, pc: &usize) -> Option<usize> {
        let casm_index = self
            .pc_to_inst_indexes_map
            .get(pc)
            .expect("Failed to get casm index");
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
        relocated_memory: &Vec<Option<Felt>>,
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
        relocated_memory: &Vec<Option<Felt>>,
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
                    if !skip_builtin_type_declaration(simplified_type_name.as_str()) {
                        results.push(InternalFnCallIO {
                            type_name: Some(simplified_type_name.clone()),
                            value: values.iter().map(|v| v.to_string()).collect(),
                        });
                    }

                    if is_panic_result(&Some(result_type.clone())) && values[0] == Felt::ONE {
                        let index = values[1].to_string().parse::<usize>().unwrap();
                        let panic_reason = relocated_memory
                            .get(index)
                            .and_then(|opt| *opt)
                            .expect("Failed to get panic reason from relocated_memory");
                        let panic_reason_decoded = decode_felt(vec![panic_reason]).unwrap();
                        results_decoded.push(create_decoded_value(
                            None,
                            "Panic",
                            DecodedValueType::String(panic_reason_decoded),
                        ));
                    } else {
                        let type_id = &return_ref.ty;
                        let mut data_index: usize = 0;
                        if let Some(decoded_element) = decode_internal_datas(
                            &values,
                            type_id,
                            &self.type_declaration_map,
                            relocated_memory,
                            &mut data_index,
                        ) {
                            results_decoded.push(decoded_element);
                        }
                    }
                }
            }
        }
        (results, results_decoded)
    }

    pub fn get_system_call_at_trace_step(
        &self,
        relocated_memory: &Vec<Option<Felt>>,
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

pub fn get_values_from_cell_expressions(
    memory: &Vec<Option<Felt>>,
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
    memory: &Vec<Option<Felt>>,
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
    memory: &Vec<Option<Felt>>,
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
