use anyhow::Result;
use blockifier::execution::{
    deprecated_syscalls::DeprecatedSyscallSelector, syscalls::SyscallSelector,
};
use byteorder::{ByteOrder, LittleEndian};
use cairo_felt::Felt252;
use cairo_lang_casm::{
    ap_change::ApChange,
    cell_expression::{CellExpression, CellOperator},
    operand::{CellRef, DerefOrImmediate, Register},
};
use cairo_lang_sierra::{
    extensions::core::{CoreLibfunc, CoreType},
    ids::ConcreteTypeId,
    program::{GenFunction, Program, StatementIdx},
    program_registry::ProgramRegistry,
};
use cairo_lang_sierra_to_casm::compiler::{SierraStatementDebugInfo, StatementKindDebugInfo};
use cairo_lang_sierra_type_size::{get_type_size_map, TypeSizeMap};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use data_decoder::decode_datas;
use indexmap::IndexSet;
use num_bigint::BigInt;
use serde_json::json;
use serde_json::Value;
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use verification::{CodeLocation, SierraStatementToCairoDebugInfo};
use walnut_shared::{build_data_items_from_type_declaration, decode_felt252};

use crate::{
    call_trace::{ContractCall, ESysCall, InternalFnCallIO},
    utils::{
        compile_sierra_contract_class, felt_to_stark_felt, get_pc_mappings,
        get_pc_to_ptr_sys_call_mappings, is_panic_result, make_casm_to_sierra_map,
    },
};

pub struct Mappings {
    pub pc_to_inst_indexes_map: HashMap<usize, usize>,
    pub pc_to_ptr_sys_calls: HashMap<usize, CellExpression>,
    pub casm_to_sierra_map: HashMap<usize, Vec<usize>>,
    pub sierra_program: Program,
    pub memory_map: HashMap<usize, BigInt>,
    pub type_sizes: TypeSizeMap,
    pub type_names: HashMap<ConcreteTypeId, SmolStr>,
    pub sierra_statement_info: Vec<SierraStatementDebugInfo>,
}

impl Mappings {
    pub fn new(
        relocated_memory: &Vec<Option<Felt252>>,
        vm_trace: &Vec<TraceEntry>,
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
            .filter_map(|x| x.as_ref().map(|_| x.clone().unwrap()))
            .map(|x| x.to_bigint())
            .enumerate()
            .map(|(i, v)| (i + 1, v))
            .collect();

        let sierra_program_registry: ProgramRegistry<CoreType, CoreLibfunc> =
            ProgramRegistry::<CoreType, CoreLibfunc>::new(&sierra_program).unwrap();
        let type_sizes =
            get_type_size_map(&sierra_program, &sierra_program_registry).unwrap_or_default();
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
        let locations: Vec<_> = locations_set.into_iter().collect();
        return locations;
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
        let locations: Vec<_> = locations_set.into_iter().collect();
        return locations;
    }

    pub fn get_sierra_execution_trace(&self, vm_trace: &Vec<TraceEntry>) -> Vec<Vec<usize>> {
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
        relocated_memory: &Vec<Option<Felt252>>,
        sierra_index: usize,
        trace_entry: &TraceEntry,
    ) -> (Vec<InternalFnCallIO>, Vec<Value>) {
        let mut arguments: Vec<InternalFnCallIO> = Vec::new();
        let mut arguments_decoded: Vec<Value> = Vec::new();
        let type_declarations = &self.sierra_program.type_declarations;
        if let Some(sierra_statement_info) = self.sierra_statement_info.get(sierra_index) {
            match &sierra_statement_info.additional_kind_info {
                StatementKindDebugInfo::Invoke(invoke_info) => {
                    for (_invoke_ref_index, invoke_ref) in invoke_info.ref_values.iter().enumerate()
                    {
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
                            .unwrap_or_else(|| "".to_string());

                        let type_id = &invoke_ref.ty;
                        if let Some(type_declaration) = type_declarations
                            .iter()
                            .find(|type_decl| type_decl.id.id == type_id.id)
                            .cloned()
                        {
                            let (enum_items, struct_items) = build_data_items_from_type_declaration(
                                &Some(type_declaration.clone()),
                                &type_declarations,
                            );
                            let mut data_index = 0;
                            let argument_decoded = decode_datas(
                                &values,
                                &vec![type_names.clone()],
                                &vec![],
                                struct_items.as_ref(),
                                enum_items.as_ref(),
                                &mut data_index,
                                false,
                            );
                            arguments.push(InternalFnCallIO {
                                type_name: Some(type_names.clone()),
                                value: values,
                            });
                            if argument_decoded.is_empty() {
                                arguments_decoded.push(json!({
                                    "type_name": type_names,
                                    "value": []
                                }));
                            } else {
                                argument_decoded
                                    .get(0)
                                    .map(|value| arguments_decoded.push(json!(value)));
                            }
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
                }
                _ => {} // StatementKindDebugInfo::Return(return_info) => {
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
        relocated_memory: &Vec<Option<Felt252>>,
        sierra_index: usize,
        trace_entry: &TraceEntry,
    ) -> (Vec<InternalFnCallIO>, Vec<Value>) {
        let mut results: Vec<InternalFnCallIO> = Vec::new();
        let mut results_decoded: Vec<Value> = Vec::new();
        let type_declarations = &self.sierra_program.type_declarations;
        if let Some(sierra_statement_info) = self.sierra_statement_info.get(sierra_index) {
            match &sierra_statement_info.additional_kind_info {
                StatementKindDebugInfo::Return(return_info) => {
                    for (_return_ref_index, return_ref) in return_info.ref_values.iter().enumerate()
                    {
                        let mut values = get_values_from_cell_expressions(
                            relocated_memory,
                            trace_entry,
                            &return_ref.expression.cells,
                            &ApChange::Known(0),
                        );

                        let result_type = self
                            .type_names
                            .get(&return_ref.ty)
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "".to_string());

                        if is_panic_result(&Some(result_type.clone())) && values[0] == "1" {
                            // Handle panic case
                            let panic_reason: Felt252 = relocated_memory
                                .get(values[1].parse::<usize>().unwrap())
                                .unwrap()
                                .clone()
                                .unwrap();
                            let panic_reason_decoded = decode_felt252(vec![panic_reason]).unwrap();
                            values = vec!["1".to_string(), panic_reason_decoded];
                        }

                        if let Some(type_declaration) = type_declarations
                            .iter()
                            .find(|type_decl| type_decl.id.id == return_ref.ty.id)
                            .cloned()
                        {
                            let (enum_items, struct_items) = build_data_items_from_type_declaration(
                                &Some(type_declaration.clone()),
                                &type_declarations,
                            );
                            let mut data_index = 0;
                            let result_decoded = decode_datas(
                                &values,
                                &vec![result_type.clone()],
                                &vec![],
                                struct_items.as_ref(),
                                enum_items.as_ref(),
                                &mut data_index,
                                false,
                            );

                            results.push(InternalFnCallIO {
                                type_name: Some(result_type.clone()),
                                value: values,
                            });
                            if result_decoded.is_empty() {
                                results_decoded.push(json!({
                                    "type_name": result_type,
                                    "value": []
                                }));
                            } else {
                                result_decoded
                                    .get(0)
                                    .map(|value| results_decoded.push(json!(value)));
                            }
                        } else {
                            results.push(InternalFnCallIO {
                                type_name: Some(result_type),
                                value: values,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        (results, results_decoded)
    }

    pub fn get_system_call_at_trace_step(
        &self,
        relocated_memory: &Vec<Option<Felt252>>,
        trace_entry: &TraceEntry,
    ) -> Option<ESysCall> {
        let pc = trace_entry.pc as usize;
        let fp = trace_entry.fp as usize;
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
                let mut ptr = value.parse::<usize>().unwrap();
                let felt_value = relocated_memory.get(ptr).unwrap();
                ptr += 1;
                let starkfelt = felt_to_stark_felt(&felt_value.as_ref().unwrap());
                let selector = SyscallSelector::try_from(starkfelt);
                match selector {
                    Ok(selector) => {
                        match selector {
                            DeprecatedSyscallSelector::CallContract => {
                                // https://github.com/starkware-libs/blockifier/blob/9bfb3d4c8bf1b68a0c744d1249b32747c75a4d87/crates/blockifier/src/execution/syscalls/hint_processor.rs#L302C13-L306C15
                                let _felt_value = relocated_memory.get(ptr).unwrap();
                                ptr += 1;
                                let felt_value: Felt252 =
                                    relocated_memory.get(ptr).unwrap().clone().unwrap();
                                let contract_address =
                                    format!("0x{:0>64}", felt_value.to_str_radix(16));
                                ptr += 1;
                                let felt_value =
                                    relocated_memory.get(ptr).unwrap().clone().unwrap();
                                let function_selector =
                                    format!("0x{:0>64}", felt_value.to_str_radix(16));
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
    memory: &Vec<Option<Felt252>>,
    trace_entry: &TraceEntry,
    cell_expressions: &Vec<CellExpression>,
    ap_change: &ApChange,
) -> Vec<String> {
    let mut value_vec: Vec<String> = Vec::new();
    for cell_expression in cell_expressions {
        let value =
            get_value_from_cell_expression(&memory, &trace_entry, &cell_expression, &ap_change);
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
    memory: &Vec<Option<Felt252>>,
    trace_entry: &TraceEntry,
    cell_ref: &CellRef,
    ap_change: &ApChange,
) -> Result<Felt252, GetCellRefValueError> {
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
    memory: &Vec<Option<Felt252>>,
    trace_entry: &TraceEntry,
    cell_expression: &CellExpression,
    ap_change: &ApChange,
) -> Result<String, GetCellRefValueError> {
    match cell_expression {
        CellExpression::Deref(cell_ref) => {
            get_cell_ref_value(memory, trace_entry, cell_ref, ap_change)
                .map(|value| value.to_string())
        }
        CellExpression::Immediate(imm) => Ok(format!("0x{:x}", imm)),
        CellExpression::DoubleDeref(cell_ref, offset) => {
            match get_cell_ref_value(memory, trace_entry, cell_ref, ap_change) {
                Ok(cell_ref_value_felt) => {
                    let cell_ref_value_bytes_le = cell_ref_value_felt.to_bytes_be();
                    let cell_ref_value =
                        LittleEndian::read_u128(&extend_to_16_bytes(cell_ref_value_bytes_le)[..])
                            as i128;
                    let addr = cell_ref_value + offset.clone() as i128;
                    let value = memory.get(addr as usize).cloned();
                    if let Some(Some(value)) = value {
                        Ok(value.to_string())
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
                        DerefOrImmediate::Immediate(x) => Ok(Felt252::from(&x.value)),
                    };

                    match b {
                        Ok(b) => {
                            let value: Result<Felt252, GetCellRefValueError> = match op {
                                CellOperator::Add => Ok(a + b),
                                CellOperator::Mul => Ok(a * b),
                                CellOperator::Div => match b.try_into() {
                                    Ok(b) => Ok(a / b),
                                    Err(_) => Err(GetCellRefValueError::OtherError(
                                        "Division by zero".to_string(),
                                    )),
                                },
                                CellOperator::Sub => Ok(a - b),
                            };
                            match value {
                                Ok(value) => Ok(value.to_string()),
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

fn extend_to_16_bytes(mut buf: Vec<u8>) -> Vec<u8> {
    if buf.len() < 16 {
        buf.resize(16, 0);
    }
    buf
}
