#!/bin/bash

# Check if the system is arm64
ARCH=$(uname -m)
echo "System architecture: $ARCH"  # Output the architecture to the console

if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    FOLDER="aarch64-apple-darwin"
else
    FOLDER="x86_64-unknown-linux-gnu"
fi

# List of versions to download
VERSIONS=("v2.8.1" "v2.8.3" "v2.8.4")  # Add more versions as needed

# Function to get the corresponding cairo version for a given scarb version
get_cairo_version() {
    local scarb_version=$1
    case $scarb_version in
        "v2.8.1")
            echo "v2.8.0"
            ;;
        "v2.8.3")
            echo "v2.8.2"
            ;;
        "v2.8.4")
            echo "v2.8.4"
            ;;
        *)
            echo "unknown"
            ;;
    esac
}

# Function to download and unpack binaries
download_and_unpack_scarb() {
    local base_dir=$1
    local version=$2
    local folder=$3
    local cairo_version=$(get_cairo_version $version)
    local url="https://github.com/software-mansion/scarb/releases/download/$version/scarb-$version-$folder.tar.gz"
    local file_name=$(basename $url)
    local file_path="$base_dir/$file_name"
    local attempt=1
    local max_attempts=3

    while [ $attempt -le $max_attempts ]; do
        echo "Downloading $url (Attempt $attempt/$max_attempts)..."
        curl -L $url -o $file_path
        if [ $? -eq 0 ]; then
            echo "Downloaded: $file_path"
            tar -xzf $file_path --strip-components=2 -C $base_dir "scarb-$version-$folder/bin/scarb"
            mv "$base_dir/scarb" "$base_dir/scarb_cairo_${cairo_version}"
            chmod +x "$base_dir/scarb_cairo_${version}_${cairo_version}"
            echo "Extracted and set executable: $base_dir/scarb_cairo_${cairo_version}"
            rm $file_path
            break
        else
            echo "Failed to download $url. Retrying..."
            attempt=$((attempt + 1))
            sleep 2
        fi
    done

    if [ $attempt -gt $max_attempts ]; then
        echo "Failed to download $url after $max_attempts attempts."
    fi
}

# Loop through each version and download/unpack
for VERSION in "${VERSIONS[@]}"; do
    download_and_unpack_scarb "binaries/scarb" "$VERSION" "$FOLDER"
done

echo "Scarb binaries downloaded, unpacked, and placed in the binaries/scarb/ folder with version in the filename."