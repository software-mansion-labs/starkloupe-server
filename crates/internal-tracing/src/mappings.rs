use anyhow::Result;
use cairo_felt::Felt252;
use cairo_lang_sierra::{
    extensions::core::{CoreLibfunc, CoreType},
    ids::ConcreteTypeId,
    program::{GenFunction, Program, StatementIdx},
    program_registry::ProgramRegistry,
};
use cairo_lang_sierra_type_size::{get_type_size_map, TypeSizeMap};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::{HashMap, HashSet};
use verification::cairo_debug_info::{CodeLocation, SierraStatementToCairoDebugInfo};

use crate::utils::{compile_sierra_contract_class, get_pc_mappings, make_casm_to_sierra_map};

pub struct Mappings {
    pub pc_to_inst_indexes_map: HashMap<usize, usize>,
    pub casm_to_sierra_map: HashMap<usize, Vec<usize>>,
    pub sierra_program: Program,
    pub memory_map: HashMap<usize, BigInt>,
    pub type_sizes: TypeSizeMap,
    pub type_names: HashMap<ConcreteTypeId, SmolStr>,
}

impl Mappings {
    pub fn new(
        relocated_memory: &Vec<Option<Felt252>>,
        vm_trace: &Vec<TraceEntry>,
        contract_class: ContractClass,
    ) -> Result<Self> {
        let sierra_program = contract_class.extract_sierra_program()?;
        let type_names = contract_class
            .sierra_program_debug_info
            .clone()
            .unwrap()
            .type_names;
        let casm_program = compile_sierra_contract_class(contract_class, usize::MAX);
        let casm_to_sierra_map = make_casm_to_sierra_map(&casm_program.debug_info, 0);
        let (_pc_inst_map, pc_to_inst_indexes_map) = get_pc_mappings(relocated_memory, vm_trace);

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
            casm_to_sierra_map,
            sierra_program,
            memory_map,
            type_sizes,
            type_names,
        })
    }

    pub fn get_sierra_indexes_at_pc(&self, pc: &usize) -> Option<Vec<usize>> {
        let casm_index = self
            .pc_to_inst_indexes_map
            .get(pc)
            .expect("Failed to get casm index");
        self.casm_to_sierra_map.get(casm_index).cloned()
    }

    pub fn get_cairo_locations_at_pc(
        &self,
        pc: &usize,
        sierra_statements_to_cairo_info: &HashMap<usize, SierraStatementToCairoDebugInfo>,
    ) -> Vec<CodeLocation> {
        if let Some(sierra_indexes) = self.get_sierra_indexes_at_pc(pc) {
            let mut locations_set = HashSet::new();
            for sierra_index in sierra_indexes {
                if let Some(cairo_info) = sierra_statements_to_cairo_info.get(&sierra_index) {
                    locations_set.extend(cairo_info.cairo_locations.clone());
                }
            }
            let locations: Vec<_> = locations_set.into_iter().collect();
            return locations;
        }
        return vec![];
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

    pub fn get_sierra_function_at_pc<'a>(
        &'a self,
        pc: &'a usize,
    ) -> Option<&'a GenFunction<StatementIdx>> {
        let sierra_indexes = self.get_sierra_indexes_at_pc(pc);
        if let Some(sierra_indexes) = sierra_indexes {
            let first_sierra_index = sierra_indexes.first();
            if let Some(first_sierra_index) = first_sierra_index {
                let func = self
                    .sierra_program
                    .funcs
                    .iter()
                    .find(|&func| func.entry_point.0 == *first_sierra_index);
                return func;
            }
        }
        return None;
    }
}
