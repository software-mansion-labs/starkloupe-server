use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing::error;
use walkdir::WalkDir;

use crate::utils::Manifest;

fn run_sozo_build(tmp_dir: &PathBuf, sozo_path: &str) -> Result<()> {
    let absolute_path = fs::canonicalize(sozo_path)?;
    let status = Command::new(absolute_path)
        .arg("build")
        .current_dir(tmp_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        error!("`sozo` exited with error: {:?}", status);
        Err(anyhow::anyhow!("Failed to compile the contract class"))
    }
}

pub fn compile_with_sozo(
    _starknet_version: (u32, u32, u32),
    manifest: Manifest,
    tmp_dir: &PathBuf,
    class_name: String,
) -> Result<(ContractClass, Option<PathBuf>)> {
    let sozo_path = "binaries/sozo/sozo";

    run_sozo_build(tmp_dir, sozo_path)?;

    let file_name = format!("{}-{}.json", manifest.package_name, class_name);

    let contract_class_path = WalkDir::new(tmp_dir.join("target"))
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy() == file_name)
        .map(|e| e.into_path())
        .ok_or_else(|| {
            error!("Failed to find contract class file in target directory");
            anyhow::anyhow!("Failed to find contract class file in target directory")
        })?;

    let contract_class_file = File::open(&contract_class_path).map_err(|e| {
        error!("Failed to open contract class file: {:?}", e);
        e
    })?;
    let contract_class_reader = BufReader::new(contract_class_file);
    let contract_class: ContractClass =
        serde_json::from_reader(contract_class_reader).map_err(|e| {
            error!("Failed to deserialize contract class: {:?}", e);
            e
        })?;

    // Assume debug info file is in the same folder with a different name format
    let debug_file_name = format!("{}-{}.debug.json", manifest.package_name, class_name);
    let cairo_debug_info_path = Some(contract_class_path.parent().unwrap().join(debug_file_name));

    Ok((contract_class, cairo_debug_info_path))
}
