#!/bin/bash

REPO="https://github.com/software-mansion/universal-sierra-compiler"
BINARY_NAME="universal-sierra-compiler"
INSTALL_DIR="$(pwd)"
TEMP_DIR="$(pwd)/universal-sierra-compiler-tmp"

main () {
  check_cmd curl
  check_cmd tar

  version=${1:-latest}
  release_tag=$(curl -# --fail -Ls -H 'Accept: application/json' "${REPO}/releases/{$version}" | sed -e 's/.*"tag_name":"\([^"]*\)".*/\1/')

  if [ -z "$release_tag" ]; then
    echo "No such version $version, please pass correct one (e.g. v1.0.0)"
    exit 1
  fi

  download_and_extract_binary "$release_tag"

  echo "${BINARY_NAME} (${release_tag}) has been installed successfully in ${INSTALL_DIR}."
}

check_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
      echo "$1 is not available"
      echo "Please install $1 and run the script again"
      exit 1
  fi
}

download_and_extract_binary() {
  release_tag=$1

  # Define the operating system and architecture
  get_architecture
  _arch="$RETVAL"

  artifact_name=${BINARY_NAME}-${release_tag}-${_arch}

  echo "Downloading and extracting ${artifact_name}..."
  # Create the temporary directory
  mkdir -p "${TEMP_DIR}"

  # Download and extract the archive
  curl -L "${REPO}/releases/download/${release_tag}/${artifact_name}.tar.gz" | tar -xz -C "${TEMP_DIR}"

  # Move the binary to the installation directory
  mkdir -p "${INSTALL_DIR}"
  mv "${TEMP_DIR}/${artifact_name}/bin/${BINARY_NAME}" "${INSTALL_DIR}"

  # Clean up temporary files
  rm -rf "${TEMP_DIR}"
}

get_architecture() {
  local _ostype _cputype _arch
  _ostype="$(uname -s)"
  _cputype="$(uname -m)"

  case "$_ostype" in
  Linux)
    _ostype=unknown-linux-gnu
    ;;

  Darwin)
    _ostype=apple-darwin
    ;;
  *)
    err "unsupported OS type: $_ostype"
    ;;
  esac

  case "$_cputype" in
  aarch64 | arm64)
    _cputype=aarch64
    ;;

  x86_64 | x86-64 | x64 | amd64)
    _cputype=x86_64
    ;;
  *)
    err "unknown CPU type: $_cputype"
    ;;
  esac

  _arch="${_cputype}-${_ostype}"

  RETVAL="$_arch"
}

set -e
main "$@"