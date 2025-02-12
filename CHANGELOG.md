# Changelog
### [0.0.31] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.31)
 
- fix in moving failed verifications to failed_tmp - now folder names are verification_ids

### [0.0.31] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.31)

- Inline strategy debug support

### [0.0.30] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.30)

- List of all emitted events, remove events from call trace

### [0.0.29] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.29)

- parsing DOJO version bug fix

### [0.0.28] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.28)

- decode event datas

### [0.0.27] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.27)

- add event to call trace

### [0.0.26] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.26)

- healthcheck endpoint added: `/health`

### [0.0.25] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.25)

- Automated downloader for new DOJO releases

### [0.0.24] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.24)

- verification build for all profiles

### [0.0.23] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.23)

- api key removed

### [0.0.22] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.22)

- rpc calls optimization

### [0.0.21] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.21)

- added support for `sozo v1.0.12`

### [0.0.20] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.20)

- upgrading Alchemy API key

### [0.0.19] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.19)

- moving failed verifications to `tmp/failed-verification` for further investigation

### [0.0.18] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.18)

- fixed support for cairo version 2.8.2 - now it's correctly supported

### [0.0.17] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.17)

- Provide paymaster_data and resource_bounds to transaction info for simulation

### [0.0.16] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.16)

- Decreased `tracing_subscriber` log level to `INFO` (was lower `tracing`) fix 2

### [0.0.15] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.15)

- Decreased `tracing_subscriber` log level to `INFO` (was lower `tracing`)

### [0.0.14] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.14)

- Improved handling for transactions that REVERT with “RunResources” error.

### [0.0.13] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.13)

- Sentry fixed - Sentry now correctly receives errors

### [0.0.12] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.12)

- added Scarb binary download scheduler - every given interval (~60 minutes) Github releases are checked
  for new Scarb versions and binaries are downloaded. It will not require to build Scarb versions manually.
- fixed choosing Scarb S3 folder based on the system architecture bug fix - using `ARCH` we get `arm` or `aarch64`
  values which can be possible returns for ARM architecture. However path to S3 is `arm64` so this mechanism could not
  work with this bug properly - it would not resolve s3 object path correctly (it's fixed now).
- changed SUPPORTED cairo versions management - due to "dynamic" support of Scarb versions, the list is not hardocded
  anymore.
  Server doesn't return now the error message with the list of supported cairo versions (because it's dynamic and can be
  long in the future).

### [0.0.11] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.11)

- added `scarb 2.8.5` support

### [0.0.10] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.10)

- Update the starknet-foundry commit per [PR](https://github.com/walnuthq/starknet-foundry/pull/3)

### [0.0.9] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.9)

- Added sierra-replace-ids in Scarb.toml to include sierra_program_debug_info in the contract class during the build
  process.

### [0.0.8] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.8)

- Reverted changes from version 0.0.3 that prevented call trace from displaying in the case of reverted transactions

### [0.0.7] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.7)

- fixed Telegram notifications to split to multiple messages when message is too long (longer than 4096 characters)
- fixed Telegram notifications when the block number is not provided
- added readonly S3 Scaleway API KEYs to .env.example

### [0.0.6] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.7)

- added missing `build-essential` in Dockerfile final stage

### [0.0.5] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.5)

- installed Rust (and cargo) in Dockerfile final stage - it is bugfix, missing cargo causes verification problem
  due to lack of `cargo` binary
