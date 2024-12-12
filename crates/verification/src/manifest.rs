use anyhow::Result;
use std::collections::HashMap;
use tracing::error;
use walnut_shared::parse_version_string_to_tuple;

use crate::scarb::{is_cairo_version_supported};

#[derive(Debug)]
pub struct Manifest {
    pub package_name: String,
    pub dojo_version: Option<String>,
    pub dojo_namespace_name: Option<String>,
    pub cairo_version: (u32, u32, u32),
}

impl Manifest {
    pub fn new(
        source_code: &mut HashMap<String, String>,
        cairo_version: Option<(u32, u32, u32)>,
    ) -> Result<Self> {
        let scarb_config_contents = match source_code.get("Scarb.toml") {
            Some(contents) => contents,
            None => {
                error!("Scarb.toml not found in source code map");
                return Err(anyhow::anyhow!("Scarb.toml not found"));
            }
        };

        let mut scarb_config_toml = match scarb_config_contents.parse::<toml::Value>() {
            Ok(parsed) => parsed,
            Err(e) => {
                error!("Failed to parse Scarb.toml: {}", e);
                return Err(anyhow::anyhow!("Failed to parse Scarb.toml: {}", e));
            }
        };

        // Add the sierra-replace-ids and unstable-add-statements-code-locations-debug-info under [cairo]
        if let Some(cairo_table) = scarb_config_toml
            .get_mut("cairo")
            .and_then(toml::Value::as_table_mut)
        {
            cairo_table.insert("sierra-replace-ids".to_string(), toml::Value::Boolean(true));
            cairo_table.insert(
                "unstable-add-statements-code-locations-debug-info".to_string(),
                toml::Value::Boolean(true),
            );
        } else {
            let mut cairo_table = toml::map::Map::new();
            cairo_table.insert("sierra-replace-ids".to_string(), toml::Value::Boolean(true));
            cairo_table.insert(
                "unstable-add-statements-code-locations-debug-info".to_string(),
                toml::Value::Boolean(true),
            );
            scarb_config_toml
                .as_table_mut()
                .unwrap()
                .insert("cairo".to_string(), toml::Value::Table(cairo_table));
        }

        // Remove the "license-file" field under the "package" section if it exists
        if let Some(package_table) = scarb_config_toml
            .get_mut("package")
            .and_then(toml::Value::as_table_mut)
        {
            package_table.remove("license-file");
        }

        // Convert the modified TOML back to a string and update the source_code
        let updated_scarb_config = toml::to_string(&scarb_config_toml).map_err(|e| {
            error!("Failed to serialize Scarb.toml: {}", e);
            anyhow::anyhow!("Failed to serialize Scarb.toml: {}", e)
        })?;
        source_code.insert("Scarb.toml".to_string(), updated_scarb_config);

        let package_name = match scarb_config_toml
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
        {
            Some(name) => name,
            None => {
                error!("Package name not found in Scarb.toml");
                return Err(anyhow::anyhow!("No package name found"));
            }
        };

        // TODO
        let cairo_version = match cairo_version {
            Some(version) => version,
            None => {
                let cairo_version = match scarb_config_toml
                    .get("dependencies")
                    .and_then(|d| d.get("starknet"))
                    .and_then(toml::Value::as_str)
                {
                    Some(version_str) => parse_version_string_to_tuple(version_str)?,
                    None => {
                        error!("Starknet version not found in Scarb.toml");
                        return Err(anyhow::anyhow!("Starknet version not found"));
                    }
                };
                cairo_version
            }
        };

        if !is_cairo_version_supported(cairo_version) {
            let error_message = format!(
                "Unsupported Cairo version {}.{}.{}. Contact us if you need support for a different version: https://t.me/walnuthq",
                cairo_version.0, cairo_version.1, cairo_version.2,
            );
            error!("{}", error_message);
            return Err(anyhow::anyhow!(error_message));
        }

        // Search for dojo dependency and get its tag
        let dojo_tag = scarb_config_toml
            .get("dependencies")
            .and_then(|d| d.get("dojo"))
            .and_then(toml::Value::as_table)
            .and_then(|dojo_table| dojo_table.get("tag"))
            .and_then(toml::Value::as_str)
            .map(|s| s.to_string());

        let dojo_namespace_name = match source_code.get("dojo_dev.toml") {
            Some(contents) => {
                let dojo_config_toml = match contents.parse::<toml::Value>() {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        error!("Failed to parse dojo_dev.toml: {}", e);
                        return Err(anyhow::anyhow!("Failed to parse dojo_dev.toml: {}", e));
                    }
                };

                dojo_config_toml
                    .get("namespace")
                    .and_then(|ns| ns.get("default"))
                    .and_then(toml::Value::as_str)
                    .map(|s| s.to_string())
            }
            None => None,
        };

        Ok(Self {
            package_name: package_name.to_string(),
            dojo_version: dojo_tag,
            dojo_namespace_name,
            cairo_version,
        })
    }
}
