use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

/// One version entry returned by the scarbs.xyz index API.
#[derive(Debug, Deserialize)]
pub struct PackageVersion {
    /// Version string, e.g. "2.0.0"
    pub v: String,
    /// Dependencies declared by this version
    pub deps: Vec<Dep>,
}

#[derive(Debug, Deserialize)]
pub struct Dep {
    pub name: String,
    /// Semver requirement string, e.g. "^2.0.0"
    pub req: String,
    /// "normal", "dev", or "build"
    pub kind: String,
}

/// Returns the scarbs.xyz sparse-index URL for `package_name`.
///
/// URL pattern (mirrors Cargo sparse index):
///   4+ chars → `…/index/{name[0..2]}/{name[2..4]}/{name}.json`
///   3 chars  → `…/index/3/{name[0]}/{name}.json`
///   2 chars  → `…/index/2/{name}.json`
///   1 char   → `…/index/1/{name}.json`
pub fn index_url(name: &str) -> String {
    let base = "https://scarbs.xyz/api/v1/index";
    match name.len() {
        0 => unreachable!("empty package name"),
        1 => format!("{}/1/{}.json", base, name),
        2 => format!("{}/2/{}.json", base, name),
        3 => format!("{}/3/{}/{}.json", base, &name[..1], name),
        _ => format!("{}/{}/{}/{}.json", base, &name[..2], &name[2..4], name),
    }
}

/// Fetches all published versions of `package_name` from the scarbs.xyz index.
/// Returns the list sorted by version (arbitrary order from registry).
pub async fn fetch_versions(client: &Client, package_name: &str) -> Result<Vec<PackageVersion>> {
    let url = index_url(package_name);
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .header("User-Agent", "walnut-scarb-dep-resolver/0.1")
        .send()
        .await?
        .error_for_status()?;

    let versions: Vec<PackageVersion> = response.json().await?;
    Ok(versions)
}
