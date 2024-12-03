# Changelog
## [0.0.6] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.7)
- added missing `build-essential` in Dockerfile final stage
## [0.0.5] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.5)
- installed Rust (and cargo) in Dockerfile final stage - it is bugfix, missing cargo causes verification problem
due to lack of `cargo` binary