use anyhow::Result;
use cairo_lang_starknet_classes::contract_class::ContractClass;
use scarb_api::ScarbCommand;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::{env, fs};
use tracing::error;

use crate::utils::Manifest;
use crate::{ClassVerificationData, SUPPORTED_VERSIONS};

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
    class_verification_data: &mut ClassVerificationData,
) -> Result<()> {
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

    for (class_hash, class_result) in class_verification_data.iter_mut() {
        let (class_name, _, _, _, _, _) = match class_result.as_ref() {
            Ok(result) => result,
            Err(e) => {
                let error_message =
                    format!("Error in class result for hash {}: {:?}", class_hash, e);
                error!("{}", error_message);
                continue;
            }
        };

        let contract_class_path = tmp_dir.join("target/dev").join(format!(
            "{}_{}.contract_class.json",
            manifest.package_name, class_name
        ));

        let contract_class_file = match File::open(&contract_class_path) {
            Ok(file) => file,
            Err(e) => {
                let error_message = format!("Failed to open contract class file: {:?}", e);
                error!("{}", error_message);
                *class_result = Err(anyhow::anyhow!(error_message));
                continue;
            }
        };

        let contract_class_reader = BufReader::new(contract_class_file);
        let contract_class: ContractClass = match serde_json::from_reader(contract_class_reader) {
            Ok(class) => class,
            Err(e) => {
                let error_message = format!("Failed to deserialize contract class: {:?}", e);
                error!("{}", error_message);
                *class_result = Err(anyhow::anyhow!(error_message));
                continue;
            }
        };

        let cairo_debug_info_path: Option<PathBuf> = match starknet_version {
            version if SUPPORTED_VERSIONS.contains(&version) => {
                Some(tmp_dir.join("target/dev").join(format!(
                    "{}_{}.contract_class_debug.json",
                    manifest.package_name, class_name
                )))
            }
            _ => None,
        };

        if let Ok((
            _,
            _,
            _,
            ref mut existing_contract_class,
            ref mut existing_cairo_debug_info_path,
            _,
        )) = class_result
        {
            *existing_contract_class = Some(contract_class);
            *existing_cairo_debug_info_path = cairo_debug_info_path;
        }
    }

    Ok(())
}
