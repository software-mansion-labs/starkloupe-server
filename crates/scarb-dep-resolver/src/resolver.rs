use std::collections::{HashMap, HashSet};

use anyhow::Result;
use futures::future::join_all;
use reqwest::Client;
use semver::{Version, VersionReq};
use tracing::{debug, warn};

use crate::registry::fetch_versions;

/// Resolves the full transitive registry dependency tree for a Scarb project.
///
/// Algorithm (level-by-level parallel BFS):
///   1. Seed the first batch with direct dependencies from `[dependencies]` in Scarb.toml
///      (skip `starknet`, `core`, git deps, path deps).
///   2. For each batch, fetch all versions from scarbs.xyz **in parallel**.
///   3. For each package in the batch:
///      a. Keep only versions that satisfy the semver constraint AND have a
///      `starknet`/`core` dep requirement that allows `starknet_version`.
///      b. Pick the highest remaining version.
///      c. Record `name = "=chosen"` in the result map.
///      d. Queue the chosen version's normal (non-dev, non-builtin) deps as the next batch.
///   4. Return the map.
pub async fn resolve(scarb_toml: &str, starknet_version: &str) -> Result<HashMap<String, String>> {
    let starknet_ver = Version::parse(starknet_version)
        .map_err(|e| anyhow::anyhow!("Invalid starknet version '{}': {}", starknet_version, e))?;

    let client = Client::new();

    let mut visited: HashSet<String> = HashSet::new();
    let mut resolved: HashMap<String, String> = HashMap::new();

    // Seed with direct [dependencies] from Scarb.toml
    let toml: toml::Value = scarb_toml
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse Scarb.toml: {}", e))?;

    let mut current_batch: Vec<(String, String)> = Vec::new();
    if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
        for (name, value) in deps {
            if is_builtin(name) {
                continue;
            }
            if let Some(c) = dep_constraint(value) {
                current_batch.push((name.clone(), c));
            }
        }
    }

    while !current_batch.is_empty() {
        // Filter out already-visited packages and mark them visited now
        let to_process: Vec<(String, String)> = current_batch
            .drain(..)
            .filter(|(name, _)| !visited.contains(name))
            .collect();

        for (name, _) in &to_process {
            visited.insert(name.clone());
        }

        if to_process.is_empty() {
            break;
        }

        // Fetch all packages in this level in parallel
        let fetch_futures = to_process
            .iter()
            .map(|(name, _)| fetch_versions(&client, name));
        let fetch_results = join_all(fetch_futures).await;

        let mut next_batch: Vec<(String, String)> = Vec::new();

        for ((pkg_name, constraint), versions_result) in to_process.iter().zip(fetch_results) {
            let versions = match versions_result {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        "scarb-dep-resolver: could not fetch '{}' from registry: {}",
                        pkg_name, e
                    );
                    continue;
                }
            };

            let req = VersionReq::parse(constraint).unwrap_or(VersionReq::STAR);

            let chosen = versions
                .iter()
                .filter(|pv| {
                    let ver = match Version::parse(&pv.v) {
                        Ok(v) => v,
                        Err(_) => return false,
                    };
                    if !req.matches(&ver) {
                        return false;
                    }
                    for dep in &pv.deps {
                        if dep.name == "starknet" || dep.name == "core" {
                            if let Ok(dep_req) = VersionReq::parse(&dep.req) {
                                if !dep_req.matches(&starknet_ver) {
                                    return false;
                                }
                            }
                        }
                    }
                    true
                })
                .max_by_key(|pv| Version::parse(&pv.v).unwrap_or(Version::new(0, 0, 0)));

            if let Some(pv) = chosen {
                debug!(
                    "scarb-dep-resolver: {} = \"={}\" (constraint: {}, starknet: {})",
                    pkg_name, pv.v, constraint, starknet_version
                );
                resolved.insert(pkg_name.clone(), format!("={}", pv.v));

                for dep in &pv.deps {
                    if dep.kind == "dev" || is_builtin(&dep.name) {
                        continue;
                    }
                    if !visited.contains(&dep.name) {
                        next_batch.push((dep.name.clone(), dep.req.clone()));
                    }
                }
            } else {
                warn!(
                    "scarb-dep-resolver: no version of '{}' satisfies '{}' with starknet {}",
                    pkg_name, constraint, starknet_version
                );
            }
        }

        current_batch = next_batch;
    }

    Ok(resolved)
}

/// Returns true for Scarb built-ins that should never be fetched from the registry.
fn is_builtin(name: &str) -> bool {
    name == "starknet" || name == "core"
}

/// Extracts a version constraint string from a TOML dependency value.
/// Returns `None` for git / path deps (which we cannot resolve via the registry).
fn dep_constraint(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(t) => {
            if t.contains_key("git") || t.contains_key("path") {
                return None;
            }
            t.get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    }
}
