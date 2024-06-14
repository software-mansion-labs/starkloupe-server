use anyhow::{anyhow, Result};
use cairo_felt::Felt252;
use cairo_lang_sierra::{
    extensions::core::{CoreLibfunc, CoreType},
    ids::ConcreteTypeId,
    program::Program,
    program_registry::ProgramRegistry,
};
use cairo_lang_sierra_type_size::{get_type_size_map, TypeSizeMap};
use cairo_lang_starknet_classes::contract_class::ContractClass;
use cairo_vm::vm::trace::trace_entry::TraceEntry;
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use starknet_api::core::ClassHash;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::BufReader,
    path::Path,
};

use crate::utils::{compile_sierra_contract_class, get_pc_mappings, make_casm_to_sierra_map};

#[derive(Debug, Serialize, Deserialize)]
pub struct SierraToCairoDebugInfo {
    pub sierra_statements_to_cairo_info: HashMap<usize, SierraStatementToCairoDebugInfo>,
}

/// Human readable position inside a file, in lines and characters.
#[derive(Debug, Serialize, Clone, Deserialize, Hash, Eq, PartialEq)]
pub struct TextPosition {
    /// Line index, 0 based.
    pub line: usize,
    /// Character index inside the line, 0 based.
    pub col: usize,
}

#[derive(Debug, Serialize, Clone, Deserialize, Hash, Eq, PartialEq)]
pub struct CodeLocation {
    pub start: TextPosition,
    pub end: TextPosition,
    pub file_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SierraStatementToCairoDebugInfo {
    pub cairo_locations: Vec<CodeLocation>,
}

pub struct CairoDebugData {
    pub debug_info: SierraToCairoDebugInfo,
}

pub struct CommonDebugData {
    pub pc_to_inst_indexes_map: HashMap<usize, usize>,
    pub casm_to_sierra_map: HashMap<usize, Vec<usize>>,
    pub sierra_program: Program,
    pub memory_map: HashMap<usize, BigInt>,
    pub type_sizes: TypeSizeMap,
    pub type_names: HashMap<ConcreteTypeId, SmolStr>,
}
// pub type TypeSizeMap = UnorderedHashMap<ConcreteTypeId, i16>;
pub struct SourceCodeMappings {
    pub class_hash: ClassHash,
    pub cairo_debug_data: Option<CairoDebugData>,
    pub common_debug_data: CommonDebugData,
}

impl SourceCodeMappings {
    pub fn new(
        class_hash: ClassHash,
        relocated_memory: &Vec<Option<Felt252>>,
        vm_trace: &Vec<TraceEntry>,
    ) -> Result<Self> {
        let class_hash_str = class_hash.to_string();

        let sierra_class = load_contract_class(&class_hash_str)?;
        let sierra_program = sierra_class.extract_sierra_program()?;
        let type_names = sierra_class
            .sierra_program_debug_info
            .clone()
            .unwrap()
            .type_names;
        let casm_program = compile_sierra_contract_class(sierra_class, usize::MAX);
        let casm_to_sierra_map = make_casm_to_sierra_map(&casm_program.debug_info, 0);
        let (pc_inst_map, pc_to_inst_indexes_map) = get_pc_mappings(relocated_memory, vm_trace);

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

        let cairo_debug_data = load_cairo_debug_data(&class_hash_str).ok();

        Ok(SourceCodeMappings {
            class_hash,
            cairo_debug_data,
            common_debug_data: CommonDebugData {
                pc_to_inst_indexes_map,
                casm_to_sierra_map,
                sierra_program,
                memory_map,
                type_sizes,
                type_names,
            },
        })
    }

    pub fn get_sierra_indexes_at_pc(&self, pc: &usize) -> Option<Vec<usize>> {
        let casm_index = self
            .common_debug_data
            .pc_to_inst_indexes_map
            .get(pc)
            .expect("Failed to get casm index");
        self.common_debug_data
            .casm_to_sierra_map
            .get(casm_index)
            .cloned()
    }

    pub fn get_cairo_locations_at_pc(&self, pc: &usize) -> Vec<CodeLocation> {
        if let Some(cairo_debug_data) = self.cairo_debug_data.as_ref() {
            if let Some(sierra_indexes) = self.get_sierra_indexes_at_pc(pc) {
                let mut locations_set = HashSet::new();
                for sierra_index in sierra_indexes {
                    if let Some(cairo_info) = cairo_debug_data
                        .debug_info
                        .sierra_statements_to_cairo_info
                        .get(&sierra_index)
                    {
                        locations_set.extend(cairo_info.cairo_locations.clone());
                    }
                }
                let locations: Vec<_> = locations_set.into_iter().collect();
                return locations;
            }
        }
        return vec![];
    }
}

fn load_contract_class(class_hash: &String) -> Result<ContractClass> {
    let file_path_str = format!("precompiled/{}/contract_class.json", class_hash);
    let file_path = Path::new(&file_path_str);
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let contract_class: ContractClass = serde_json::from_reader(reader)?;
    Ok(contract_class)
}

fn load_cairo_debug_data(class_hash: &String) -> Result<CairoDebugData> {
    let debug_info = load_cairo_debug_info(class_hash)?;
    Ok(CairoDebugData { debug_info })
}

fn load_cairo_debug_info(class_hash: &String) -> Result<SierraToCairoDebugInfo> {
    let debug_info_file_path_str = format!("precompiled/{}/debug_info.json", class_hash);
    let debug_info_file_path = Path::new(&debug_info_file_path_str);
    let debug_info_file = File::open(debug_info_file_path)?;
    let reader = BufReader::new(debug_info_file);
    let sierra_to_cairo_debug_info = serde_json::from_reader(reader)?;
    Ok(sierra_to_cairo_debug_info)
}

// fn load_contract_class()
