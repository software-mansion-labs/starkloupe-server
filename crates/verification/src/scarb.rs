use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use scarb_api::ScarbCommand;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::{env, fs};
use tracing::error;

use crate::utils::Manifest;
use crate::SUPPORTED_VERSIONS;

pub fn run_scarb_build(tmp_dir: &PathBuf, scarb_path: &str) -> Result<()> {
    let mut cmd = ScarbCommand::new();
    cmd.current_dir(tmp_dir);
    let absolute_path = fs::canonicalize(scarb_path)?;
    cmd.scarb_path(absolute_path);
    cmd.arg("build");
    let scarb_cache_dir = env::current_dir()?.join(".cache/scarb");
    let scarb_cache_dir_str = scarb_cache_dir.to_str().ok_or_else(|| {
        error!("Error converting cache directory to string");
        anyhow::anyhow!("Failed to convert cache dir to string")
    })?;
    cmd.env("SCARB_CACHE", scarb_cache_dir_str);
    let mut process_cmd = cmd.command();
    if process_cmd.status()?.success() {
        Ok(())
    } else {
        let output = match process_cmd.output() {
            Ok(output) => output,
            Err(e) => {
                error!("Failed to execute `scarb`, failed to get output: {:?}", e);
                return Err(anyhow::anyhow!("Failed to compile the contract class"));
            }
        };
        error!("`scarb` exited with error: {:?}", output);
        Err(anyhow::anyhow!("Failed to compile the contract class"))
    }
}

pub fn compile_with_scarb(
    starknet_version: (u32, u32, u32),
    manifest: Manifest,
    tmp_dir: &PathBuf,
    class_name: String,
) -> Result<(ContractClass, Option<PathBuf>)> {
    match starknet_version {
        version if SUPPORTED_VERSIONS.contains(&version) => {
            let scarb_path = match version {
                (2, 6, 3) => "binaries/scarb/scarb_cairo_v_2_6_3",
                (2, 6, 4) => "binaries/scarb/scarb_cairo_v_2_6_4",
                (2, 7, 0) => "binaries/scarb/scarb_cairo_v_2_7_0",
                _ => unreachable!(),
            };
            run_scarb_build(tmp_dir, scarb_path)?;
        }
        _ => {
            error!(
                "Unsupported Cairo version {}.{}.{}",
                starknet_version.0, starknet_version.1, starknet_version.2
            );
            return Err(anyhow::anyhow!("Unsupported Cairo version. Currently, we support versions 2.6.3, 2.6.4, 2.7.0 and will add support for more versions soon. Contact us if you need support for a different version: https://t.me/walnuthq"));
        }
    };

    let contract_class_path = tmp_dir.join("target/dev").join(format!(
        "{}_{}.contract_class.json",
        manifest.package_name, class_name
    ));
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

    let cairo_debug_info_path: Option<PathBuf> = match starknet_version {
        version if SUPPORTED_VERSIONS.contains(&version) => {
            Some(tmp_dir.join("target/dev").join(format!(
                "{}_{}.contract_class_debug.json",
                manifest.package_name, class_name
            )))
        }
        _ => None,
    };

    Ok((contract_class, cairo_debug_info_path))
}
