use anyhow::Result;
use std::collections::{HashMap, HashSet};
use tracing::{error, info};
use walnut_shared::parse_version_string_to_tuple;

use crate::scarb::is_cairo_version_supported;

/// Find Cairo version from workspace member Scarb.toml files
fn find_cairo_version_from_workspace_members(
    source_code: &HashMap<String, String>,
) -> Option<(u32, u32, u32)> {
    // Look for any Scarb.toml that has starknet dependency
    for (path, content) in source_code {
        if path.ends_with("Scarb.toml") && path != "Scarb.toml" {
            if let Ok(toml_value) = content.parse::<toml::Value>() {
                if let Some(version_str) = toml_value
                    .get("dependencies")
                    .and_then(|d| d.get("starknet"))
                    .and_then(toml::Value::as_str)
                {
                    if let Ok(version) = parse_version_string_to_tuple(version_str) {
                        info!(
                            "Found Cairo version {} from workspace member: {}",
                            version_str, path
                        );
                        return Some(version);
                    }
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub package_name: String,
    pub is_workspace: bool,
    pub dojo_version: Option<String>,
    pub dojo_namespace_name: Option<String>,
    pub cairo_version: (u32, u32, u32),
    pub profiles: HashSet<String>,
    pub profile_with_inline_strategy: HashMap<String, bool>,
}

impl Manifest {
    pub fn new(
        source_code: &mut HashMap<String, String>,
        cairo_version: Option<(u32, u32, u32)>,
    ) -> Result<Self> {
        Self::new_with_verified_name(source_code, cairo_version, None)
    }

    pub fn new_with_verified_name(
        source_code: &mut HashMap<String, String>,
        cairo_version: Option<(u32, u32, u32)>,
        verified_name: Option<&str>,
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

        fn insert_cairo_debug_info(cairo_table: &mut toml::map::Map<String, toml::Value>) {
            cairo_table.insert("sierra-replace-ids".to_string(), toml::Value::Boolean(true));
            cairo_table.insert(
                "unstable-add-statements-code-locations-debug-info".to_string(),
                toml::Value::Boolean(true),
            );
        }

        fn is_cairo_inline_strategy_on(cairo_table: &toml::map::Map<String, toml::Value>) -> bool {
            match cairo_table.get("inlining-strategy") {
                Some(toml::Value::String(value)) => "avoid" == value,
                _ => false,
            }
        }

        let mut profile_with_inline_strategy = HashMap::new();
        let mut profiles: HashSet<String> =
            HashSet::from(["release".to_string(), "dev".to_string()]);
        if let Some(profile_table) = scarb_config_toml
            .get_mut("profile")
            .and_then(toml::Value::as_table_mut)
        {
            for (profile_name, profile_value) in profile_table.iter_mut() {
                profiles.insert(profile_name.to_string());
                if let Some(cairo_table) = profile_value
                    .as_table_mut()
                    .and_then(|table| table.get_mut("cairo").and_then(toml::Value::as_table_mut))
                {
                    insert_cairo_debug_info(cairo_table);
                    if is_cairo_inline_strategy_on(cairo_table) {
                        profile_with_inline_strategy.insert(profile_name.to_string(), true);
                    }
                } else if let Some(profile_table) = profile_value.as_table_mut() {
                    let mut cairo_table = toml::map::Map::new();
                    insert_cairo_debug_info(&mut cairo_table);
                    profile_table.insert("cairo".to_string(), toml::Value::Table(cairo_table));
                }
            }
        }
        // Detect if this is a workspace project (need to check before adding [cairo] section)
        let is_workspace = scarb_config_toml.get("workspace").is_some();

        // Add the sierra-replace-ids and unstable-add-statements-code-locations-debug-info under [cairo]
        // But only for non-workspace projects. Workspace projects should only use profile-level config.
        if let Some(cairo_table) = scarb_config_toml
            .get_mut("cairo")
            .and_then(toml::Value::as_table_mut)
        {
            insert_cairo_debug_info(cairo_table);
            if is_cairo_inline_strategy_on(cairo_table) && !profiles.contains("dev") {
                profile_with_inline_strategy.insert("dev".to_string(), true);
            }
        } else if !is_workspace {
            // Only add [cairo] section for non-workspace projects
            let mut cairo_table = toml::map::Map::new();
            insert_cairo_debug_info(&mut cairo_table);
            scarb_config_toml
                .as_table_mut()
                .unwrap()
                .insert("cairo".to_string(), toml::Value::Table(cairo_table));
        }

        // If we did not found the inlining-strategy as build confirgurtion,
        // the  profile_with_inline_strategy is empty, so we can create the new profile
        // walnut-debug with inining strategy flag on
        if profile_with_inline_strategy.is_empty() {
            let mut walnut_profile = toml::map::Map::new();
            let mut cairo_table = toml::map::Map::new();
            insert_cairo_debug_info(&mut cairo_table);
            cairo_table.insert(
                "inlining-strategy".to_string(),
                toml::Value::String("avoid".to_string()),
            );
            walnut_profile.insert("cairo".to_string(), toml::Value::Table(cairo_table));

            let profile_table = scarb_config_toml
                .as_table_mut()
                .unwrap()
                .entry("profile")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

            if let toml::Value::Table(profile_table) = profile_table {
                profile_table.insert(
                    "walnut-debug".to_string(),
                    toml::Value::Table(walnut_profile),
                );
            }
            profiles.insert("walnut-debug".to_string());
            profile_with_inline_strategy.insert("walnut-debug".to_string(), true);
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

        tracing::info!("Package toml {:?}", scarb_config_toml);
        let package_name = match scarb_config_toml
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
        {
            Some(name) => name.to_string(),
            None => match verified_name {
                Some(name) => {
                    tracing::info!(
                        "Package name not found in Scarb.toml, using verifiedName from Voyager: {}",
                        name
                    );
                    name.to_string()
                }
                None => {
                    error!("Package name not found in Scarb.toml");
                    return Err(anyhow::anyhow!("No package name found"));
                }
            },
        };

        // Get Cairo version - either from parameter, root Scarb.toml, or workspace member
        let cairo_version = match cairo_version {
            Some(version) => version,
            None => {
                // First try root Scarb.toml
                let version_from_root = scarb_config_toml
                    .get("dependencies")
                    .and_then(|d| d.get("starknet"))
                    .and_then(toml::Value::as_str)
                    .and_then(|v| parse_version_string_to_tuple(v).ok());

                match version_from_root {
                    Some(version) => version,
                    None => {
                        // For workspace projects, try to find version from member Scarb.toml
                        if is_workspace {
                            match find_cairo_version_from_workspace_members(source_code) {
                                Some(version) => version,
                                None => {
                                    error!("Starknet version not found in workspace members");
                                    return Err(anyhow::anyhow!(
                                        "Starknet version not found in workspace"
                                    ));
                                }
                            }
                        } else {
                            error!("Starknet version not found in Scarb.toml");
                            return Err(anyhow::anyhow!("Starknet version not found"));
                        }
                    }
                }
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

        // TODO Check if we need this, inside Scarb.toml we have the name of dojo_project
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
            is_workspace,
            dojo_version: dojo_tag,
            dojo_namespace_name,
            cairo_version,
            profiles,
            profile_with_inline_strategy,
        })
    }
}
