#!/bin/bash

# Check if the system is arm64
ARCH=$(uname -m)
echo "System architecture: $ARCH"  # Output the architecture to the console
if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    FOLDER="arm64"
else
    FOLDER="x86_64"
fi

SOZO_URLS=(
    "https://walnut-public-deployment-binaries.s3.fr-par.scw.cloud/sozo/$FOLDER/sozo_v1.0.1"
)
SCARB_URLS=(
    "https://walnut-public-deployment-binaries.s3.fr-par.scw.cloud/scarb/$FOLDER/scarb_cairo_v_2_6_3"
    "https://walnut-public-deployment-binaries.s3.fr-par.scw.cloud/scarb/$FOLDER/scarb_cairo_v_2_6_4"
    "https://walnut-public-deployment-binaries.s3.fr-par.scw.cloud/scarb/$FOLDER/scarb_cairo_v_2_7_0"
    "https://walnut-public-deployment-binaries.s3.fr-par.scw.cloud/scarb/$FOLDER/scarb_cairo_v2.8.2"
    "https://walnut-public-deployment-binaries.s3.fr-par.scw.cloud/scarb/$FOLDER/scarb_cairo_v2.8.4"
)


# Create base directories if they don't exist
mkdir -p binaries/sozo
mkdir -p binaries/scarb

# Function to download binaries
download_binaries() {
    local base_dir=$1
    shift
    local urls=("$@")
    for url in "${urls[@]}"; do
        local file_path="$base_dir/$(basename $url)"
        local attempt=1
        local max_attempts=3

        while [ $attempt -le $max_attempts ]; do
            curl -L $url -H "Authorization: Bearer $SCW_SECRET_KEY" -o $file_path
            if [ $? -eq 0 ]; then
                chmod +x $file_path
                echo "Downloaded and set executable: $file_path"
                break
            else
                echo "Failed to download $url. Retrying..."
                attempt=$((attempt + 1))
                sleep 2
            fi
        done

        if [ $attempt -gt $max_attempts ]; then
            echo "Failed to download $url after $max_attempts attempts."
            return 1
        fi
    done
}

# Download sozo binaries
download_binaries "binaries/sozo" "${SOZO_URLS[@]}" || exit 1

# Download scarb binaries
download_binaries "binaries/scarb" "${SCARB_URLS[@]}" || exit 1

echo "Binaries downloaded and placed in respective folders."
