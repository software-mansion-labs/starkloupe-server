use serde::Deserialize;
use std::collections::HashMap;
use walnut_shared::voyager_api_key;

/// Configuration for Voyager API client
#[derive(Clone, Debug)]
pub struct VoyagerConfig {
    pub api_key: String,
    pub base_url: String,
    pub enabled: bool,
    pub timeout_secs: u64,
}

impl VoyagerConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.voyager.online/beta".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    pub fn disabled() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            enabled: false,
            timeout_secs: 30,
        }
    }

    /// Config built from `VOYAGER_API_KEY`. Without the key Voyager lookups are
    /// turned off rather than failing.
    pub fn get_voyager_config() -> Self {
        match voyager_api_key() {
            Some(api_key) => Self::new(api_key),
            None => Self::disabled(),
        }
    }
}

/// Response from Voyager API for class source code
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VoyagerSourceResponse {
    pub class_hash: String,
    pub source_code: HashMap<String, String>,
    pub compiler_version: String,
    pub verified_timestamp: u64,
    pub verified_name: String,
}

/// Parsed compiler version
#[derive(Debug, Clone, Copy)]
pub struct CompilerVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl CompilerVersion {
    /// Parse version string like "2.14.0" into tuple
    pub fn parse(version_str: &str) -> anyhow::Result<Self> {
        let parts: Vec<&str> = version_str.split('.').collect();
        if parts.len() != 3 {
            anyhow::bail!("Invalid compiler version format: {}", version_str);
        }

        Ok(Self {
            major: parts[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid major version: {}", parts[0]))?,
            minor: parts[1]
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid minor version: {}", parts[1]))?,
            patch: parts[2]
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid patch version: {}", parts[2]))?,
        })
    }

    pub fn as_tuple(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_compiler_version() {
        let version = CompilerVersion::parse("2.14.0").unwrap();
        assert_eq!(version.major, 2);
        assert_eq!(version.minor, 14);
        assert_eq!(version.patch, 0);
        assert_eq!(version.as_tuple(), (2, 14, 0));
    }

    #[test]
    fn test_parse_compiler_version_invalid() {
        assert!(CompilerVersion::parse("2.14").is_err());
        assert!(CompilerVersion::parse("invalid").is_err());
    }
}
