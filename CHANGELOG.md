# Changelog

### [0.0.77] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.77)

- Initial gass fetch from versioned starknet constants

### [0.0.76] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.76)

- L2->L1 Consumed on L1 event support

### [0.0.75] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.75)

- Gas calculation for tx v3 and sierra >= 1.7.0

### [0.0.74] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.74)

- Max fee versus actual fee check

### [0.0.73] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.73)

- Fix the usage of Sierra version from sierra program instead of LATEST constant

### [0.0.72] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.72)

- Fix the revert tx error messages parsing

### [0.0.71] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.71)

- Storage changes optimization

### [0.0.70] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.70)

- Array allocation size check

### [0.0.69] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.69)

- Upgrade to latest starknet::providers - 0.14.0, remove old starknet providers - 0.10.0

### [0.0.68] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.68)

- Storage changes

### [0.0.67] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.67)

- Add timeout duration for transaction simulation request

### [0.0.66] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.66)

- Fix the pc to casm instraction mapping in case casm instraction is system call

### [0.0.65] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.65)

- Upgrade foundry, blockifier, cairo and staknet dependencies

### [0.0.64] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.64)

- Change scarb output from inherted() to piped()

### [0.0.63] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.63)

- Remove use of expect() in getting data from relocated memory

### [0.0.62] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.62)

- Dockerfile upgrade rust version in final stage

### [0.0.61] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.61)

- Support for cairo 2.9.4

### [0.0.60] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.60)

- Remove to_fixed_string in the contract get entrypoint endpoint

### [0.0.59] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.59)

- Support to show searching on etehreum network

### [0.0.58] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.58)

- Upgrade rust version to 1.85.1

### [0.0.57] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.57)

- Optimize decode calldata

### [0.0.56] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.56)

- CAIRO 1 ABI format support

### [0.0.55] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.55)

- L1<>L2 transaction support

### [0.0.54] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.54)

- Contract handler return networks and cairo version

### [0.0.53] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.53)

- Decode calldata to native types

### [0.0.52] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.52)

- Remove contract state

### [0.0.51] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.51)

- Same as tag 50, push mistake

### [0.0.50] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.50)

- Code cleanup, contract entrypoints endpoint

### [0.0.49] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.49)

- 1:1 showing values to types in internal functions

### [0.0.48] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.48)

- Return tx index and total number of tx in block

### [0.0.47] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.47)

- fix decoding bug for byte array

### [0.0.46] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.46)

- installing rust 1.80.0 in Dockerfile

### [0.0.45] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.45)

- no changes version

### [0.0.44] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.44)

- API for verification dashboard

### [0.0.42] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.42)

- Decode event in internal function

### [0.0.41] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.41)

- Internal function enum decoded

### [0.0.40] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.40)

- Dev profile is default, it is not possible to define custom dev profile

### [0.0.39] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.39)

- Events additional information

### [0.0.38] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.38)

- Place events inside the function call trace

### [0.0.37] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.37)

- compilation error fix (no changes)

- ### [0.0.36] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.36)

- CPU limit for scarb/sozo - processes are killed if they run longer than allowed time (300 secs)
- verification folders tmp name fix - now verification tmp folders have the name of verification id (easier to identify)
- added running sozo/scarb Github downloader after server startup
-

### [0.0.35] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.35)

- Sequential project build per profile

### [0.0.34] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.34)

- Fix for non showing verified classes

### [0.0.33] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.33)

- Inline strategy debug support

### [0.0.32] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.32)

- Show error stack trace in case of project build fail

### [0.0.31] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.31)

- fix in moving failed verifications to failed_tmp - now folder names are verification_ids

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
