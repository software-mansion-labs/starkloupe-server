//! Naming of the Scarb and Sozo binaries kept on disk.
//!
//! A binary is stored as `<binaries dir>/<tool>/<tool>_v<major>.<minor>.<patch>`,
//! e.g. `scarb/scarb_cairo_v2.6.3` or `sozo/sozo_v1.0.1` — the same names the
//! binaries bucket uses. Everything that writes a binary (the download
//! schedulers) and everything that runs one (the verifier) goes through here,
//! so the two cannot drift apart.

use std::fmt::{self, Display};

/// A tool whose binaries the server keeps one version of per release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Scarb,
    Sozo,
}

impl Tool {
    /// The directory the binaries live in and the prefix their names carry.
    ///
    /// Scarb binaries are named after the Cairo version they build, not the
    /// Scarb tag they were released under, hence the different prefix.
    const fn directory_and_prefix(self) -> (&'static str, &'static str) {
        match self {
            Tool::Scarb => ("scarb", "scarb_cairo"),
            Tool::Sozo => ("sozo", "sozo"),
        }
    }

    /// The name the binary for `version` is stored under.
    ///
    /// `version` may be a parsed semver, a bare `2.6.3`, or a tag like `v1.0.1`
    /// as written in a `Scarb.toml`; the `v` ends up there exactly once.
    pub fn binary_name(self, version: impl Display) -> String {
        let (_, prefix) = self.directory_and_prefix();
        format!(
            "{prefix}_v{}",
            version.to_string().trim().trim_start_matches('v')
        )
    }

    /// The directory this tool's binaries live in under `binaries_dir`.
    pub fn binary_dir(self, binaries_dir: &str) -> String {
        let (directory, _) = self.directory_and_prefix();
        format!("{binaries_dir}/{directory}")
    }

    /// Where the binary for `version` lives under `binaries_dir`.
    pub fn binary_path(self, binaries_dir: &str, version: impl Display) -> String {
        format!(
            "{}/{}",
            self.binary_dir(binaries_dir),
            self.binary_name(version)
        )
    }

    /// Where the marker recording that release `tag` is installed lives.
    ///
    /// A Scarb binary is named after the Cairo version it ships, which is only
    /// known once the archive has been downloaded and the binary run. Keying
    /// the marker by the release tag instead lets an already installed release
    /// be recognised without downloading it again.
    ///
    /// The tag goes in verbatim except for the separators a Dojo tag carries
    /// (`sozo/v1.8.1`), which would otherwise open a subdirectory.
    pub fn installed_marker_path(self, binaries_dir: &str, tag: &str) -> String {
        let file_name = tag.trim().replace(['/', '\\'], "_");
        format!("{}/.installed/{file_name}", self.binary_dir(binaries_dir))
    }
}

impl Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (directory, _) = self.directory_and_prefix();
        f.write_str(directory)
    }
}

#[cfg(test)]
mod tests {
    use super::Tool;
    use semver::Version;

    #[test]
    fn names_a_scarb_binary_after_its_cairo_version() {
        assert_eq!(Tool::Scarb.binary_name("2.6.3"), "scarb_cairo_v2.6.3");
        assert_eq!(Tool::Scarb.binary_name("2.10.1"), "scarb_cairo_v2.10.1");
    }

    #[test]
    fn names_a_binary_the_same_from_a_parsed_version_as_from_a_tag() {
        // The verifier holds a Cairo version it read from a manifest; the
        // download scheduler holds one it parsed out of a release. Both have to
        // land on the same file name.
        assert_eq!(
            Tool::Scarb.binary_name(Version::parse("2.6.3").unwrap()),
            Tool::Scarb.binary_name("2.6.3")
        );
        assert_eq!(
            Tool::Sozo.binary_name(Version::parse("1.0.1").unwrap()),
            Tool::Sozo.binary_name("v1.0.1")
        );
    }

    #[test]
    fn names_a_sozo_binary_with_or_without_the_v_on_the_tag() {
        assert_eq!(Tool::Sozo.binary_name("v1.0.1"), "sozo_v1.0.1");
        assert_eq!(Tool::Sozo.binary_name("1.0.1"), "sozo_v1.0.1");
    }

    #[test]
    fn keeps_a_prerelease_suffix() {
        // Sozo ships prereleases; the tag in Scarb.toml carries them, so the
        // binary has to be found under the same name it was downloaded as.
        assert_eq!(
            Tool::Sozo.binary_name(Version::parse("1.6.0-alpha.2").unwrap()),
            "sozo_v1.6.0-alpha.2"
        );
    }

    #[test]
    fn puts_each_tool_in_its_own_directory() {
        assert_eq!(
            Tool::Scarb.binary_path("/opt/app/binaries", "2.6.3"),
            "/opt/app/binaries/scarb/scarb_cairo_v2.6.3"
        );
        assert_eq!(
            Tool::Sozo.binary_path("/opt/app/binaries", "v1.0.1"),
            "/opt/app/binaries/sozo/sozo_v1.0.1"
        );
    }

    #[test]
    fn keys_an_install_marker_by_the_release_tag() {
        // The tag is what a release gives us for free; the Cairo version in the
        // binary name is not derivable from it without downloading the binary.
        assert_eq!(
            Tool::Scarb.installed_marker_path("/opt/app/binaries", "v2.12.0"),
            "/opt/app/binaries/scarb/.installed/v2.12.0"
        );
    }

    #[test]
    fn keeps_a_tag_with_a_separator_out_of_a_subdirectory() {
        // Dojo tags its releases `sozo/v1.8.1`.
        assert_eq!(
            Tool::Sozo.installed_marker_path("/opt/app/binaries", "sozo/v1.8.1"),
            "/opt/app/binaries/sozo/.installed/sozo_v1.8.1"
        );
    }

    #[test]
    fn displays_as_the_name_the_logs_and_the_bucket_use() {
        assert_eq!(Tool::Scarb.to_string(), "scarb");
        assert_eq!(Tool::Sozo.to_string(), "sozo");
    }
}
