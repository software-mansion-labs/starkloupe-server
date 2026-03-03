mod registry;
mod resolver;

use std::collections::HashMap;

/// Returns true if the error message indicates a Scarb dependency resolution failure
/// that the scarbs.xyz fallback can potentially fix.
pub fn is_dep_resolution_error(error: &str) -> bool {
    error.contains("cannot get dependencies of")
        || error.contains("cannot find package")
        || error.contains("failed to select a version")
        || error.contains("no matching version found")
}

/// Resolves all registry dependencies for a Scarb project by querying the scarbs.xyz
/// registry API.  Returns a map of `{ package_name → "=exact.version" }` entries
/// that pin every registry transitive dep to the highest version compatible with the
/// project's starknet version.
///
/// Only packages from `registry+https://scarbs.xyz/` are resolved; git and path
/// dependencies are left untouched.
pub async fn resolve_registry_deps(
    scarb_toml: &str,
    starknet_version: &str,
) -> anyhow::Result<HashMap<String, String>> {
    resolver::resolve(scarb_toml, starknet_version).await
}
