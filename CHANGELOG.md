# Changelog

### [0.0.151] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.151)

- Add Apache-2.0 LICENSE and NOTICE, cargo-deny config and gitleaks scanning in CI
- Remove the unused batch-sim and team-onboarding crates and the old deploy scripts

### [0.0.150] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.150)

- Upgrade dependencies

### [0.0.149] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.149)

- Admin endpoints for shared API key lifecycle

### [0.0.148] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.148)

- Admin endpoints for tenant and member management

### [0.0.147] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.147)

- Extend OpenAPI spec with Simulation endpoints

### [0.0.146] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.146)

- Add deep link to the simulation response

### [0.0.145] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.145)

- Upgrade Rust, clean up clippy warnings across all crates and add format/clippy CI check
- Remove RLIMIT_AS

### [0.0.144] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.144)

- Add support for multiple sources on classes

### [0.0.143] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.143)

- Add X-API-KEY extractor with LRU cache and /health-check-api-key endpoint

### [0.0.142] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.142)

- Add token generation helper and X-Admin-Token extractor

### [0.0.141] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.141)

- Add tenants, tenant_members and api_keys schema and models

### [0.0.140] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.140)

- Update README and .env.example

### [0.0.139] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.139)

- Return class name on contract/class page

### [0.0.138] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.138)

- Save Voyager class to S3 and DB, with retry for transient failures
- Add background retry for timed-out Voyager compilations

### [0.0.137] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.137)

- Fix verification for scarb 2.16.1

### [0.0.136] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.136)

- Add ABI cache and step index for the debugger from the call trace
- Logging instrument

### [0.0.135] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.135)

- Initial step for debugging

### [0.0.134] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.134)

- Add Grafana and Sheets integration

### [0.0.133] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.133)

- Optimize mappings and pin scarb.xyz versions
- Reduce logging level

### [0.0.132] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.132)

- Pin scarb versions from the cache registry and update the Linux scarb cache path

### [0.0.131] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.131)

- Add execution resources to simulation
- Show L2 flamegraph when L2 gas is available

### [0.0.130] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.130)

- Split Voyager compilation into two phases
- Add function_calls to the trace for Voyager class
- Use inline hash for building the debugger trace

### [0.0.129] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.129)

- Add timeout for scarb build and build in parallel
- Reduce logs

### [0.0.128] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.128)

- Initial Voyager support
- Improve building workspace projects

### [0.0.127] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.127)

- Add type to the decoded calldata response

### [0.0.126] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.126)

- Check if the transfer contract is strk_fee_token_addr

### [0.0.125] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.125)

- Migrate from starknet-rs to starknet-rust

### [0.0.124] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.124)

- Normalize class hash by padding leading zeros to 66 characters

### [0.0.123] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.123)

- Add ABI enum parsing to use proper variant names during function decoding

### [0.0.122] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.122)

- Fix sozo downloader, handling both "v1.8.0" and "sozo/v1.8.1" formats

### [0.0.121] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.121)

- Update foudnry commit hash - relocation trace always present

### [0.0.120] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.120)

- Added new error message in case dojo version is not supported

### [0.0.119] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.119)

- Added verified cache invalidation logic

### [0.0.118] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.118)

- Added ABI to contract response

### [0.0.117] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.117)

- Better errror in case contract is not deployed

### [0.0.116] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.116)

- Fixed a circular recursion issue in data types (e.g., Layout → FieldLayout → Layout) that was preventing Contract pages from opening

### [0.0.115] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.115)

- Always return raw simulation args

### [0.0.114] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.114)

- Fetch all contracts for class

### [0.0.113] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.113)

- Fix the encoding for array of tuples

### [0.0.112] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.112)

- Upgrade rust to 1.89.0, upgrade cairo to 2.12.3, starknet-rs ro 0.17.0, blockifier to main-0.14.0 and foundry

### [0.0.111] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.111)

- Simplify type name for decoding calldata

### [0.0.110] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.110)

- Fixing for parent struct member decoding

### [0.0.109] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.109)

- Fixed sncast mainnet verification

### [0.0.108] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.108)

- Parse chain id for telegram bot

### [0.0.107] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.107)

- Rename variants -> enum_variants, members -> struct_members

### [0.0.106] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.106)

- Error message for DEPLOY and DEPLOY_ACCOUNT type 

### [0.0.105] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.105)

- Rename MAIN -> MAINNET

### [0.0.104] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.104)

- Upgrading Alchemy to 0.9

### [0.0.103] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.103)

- Fix for simple enum encode

### [0.0.102] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.102)

- Fix verification error message

### [0.0.101] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.101)

- Encode decode calldata

### [0.0.100] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.100)

- Fix test and reorganize type decoder code

### [0.0.99] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.99)

- Expand entrypoints response with full decoded data

### [0.0.98] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.98)

- Introduce caching

### [0.0.97] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.97)

- Remove core library from call trace

### [0.0.96] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.96)

- Fix double decoded type

### [0.0.95] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.95)

- Remove expresion [\d-\d] from call trace
  
### [0.0.94] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.94)

- Change sender_address type to ContractAddress
  
### [0.0.93] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.93)

- New verification error message in case of SIGKILL

### [0.0.92] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.92)

- Add decoded values(args and results) to the function call trace

### [0.0.91] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.91)

- Update the starknet-foundry - fix no trace case

### [0.0.90] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.90)

- Add contract type for l2 flamegraph

### [0.0.89] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.89)

- Refactore flamegraph code - move to separate module

### [0.0.88] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.88)

- L1 Data Flamegraph support

### [0.0.87] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.87)

- Fix failed to simulate class that have __validate__ delegate call

### [0.0.86] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.86)

- Fix not showing funcuntion calls, remove zip usage when filter for verified class

### [0.0.85] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.85)

- Actual fee support 

### [0.0.84] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.84)

- New tx hash not found message - Transaction hash {0} not found on {1}

### [0.0.83] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.83)

- Fix mixing loweer and upper cas in Flamegraph nodes

### [0.0.82] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.82)

- Move deubber data to new endpoint
  
### [0.0.81] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.81)

- Normalize gas value for the flamegraph
  
### [0.0.80] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.80)

- Reverted tx gas calculation

### [0.0.79] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.79)

- Reverted tx sierra gas support

### [0.0.78] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.78)

- Resources bounds for the ALLResources set to MAX instead of 0

### [0.0.77] (https://github.com/walnuthq/walnut-server/releases/tag/0.0.77)

- Initial gas fetch from versioned starknet constants

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
