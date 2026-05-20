#!/usr/bin/env bash
# setup-keystore.sh — one-time setup: generate a signing key and store it in
# the persistent Docker volume so it survives image rebuilds.
#
# Run this ONCE before your first signed build.
# The key lives in the Docker volume "phira-keystore" and is never written
# to the source tree.
set -euo pipefail

VOL_KEYSTORE="phira-keystore"
KEYSTORE_PASS="${KEYSTORE_PASS:-changeme}"
KEY_ALIAS="${KEY_ALIAS:-phira}"
KEY_PASS="${KEY_PASS:-changeme}"

docker volume inspect "${VOL_KEYSTORE}" > /dev/null 2>&1 || docker volume create "${VOL_KEYSTORE}"

docker run --rm \
    --platform linux/amd64 \
    -v "${VOL_KEYSTORE}:/keystore" \
    -e "KEYSTORE_PASS=${KEYSTORE_PASS}" \
    -e "KEY_ALIAS=${KEY_ALIAS}" \
    -e "KEY_PASS=${KEY_PASS}" \
    eclipse-temurin:17-jdk-jammy \
    bash -c '
        set -e
        if [ -f /keystore/keystore.jks ]; then
            echo "Keystore already exists — skipping generation."
        else
            keytool -genkeypair \
                -keystore /keystore/keystore.jks \
                -alias "${KEY_ALIAS}" \
                -keyalg RSA -keysize 2048 -validity 10000 \
                -storepass "${KEYSTORE_PASS}" \
                -keypass "${KEY_PASS}" \
                -dname "CN=Phira Build, O=Self-Signed, C=US"
            echo "Keystore generated."
        fi
        # Write env file so the build entrypoint can source it
        cat > /keystore/keystore.env <<EOF
export KEYSTORE_PASS="${KEYSTORE_PASS}"
export KEY_ALIAS="${KEY_ALIAS}"
export KEY_PASS="${KEY_PASS}"
EOF
        echo "keystore.env written."
    '

echo "=== Keystore ready in volume: ${VOL_KEYSTORE} ==="
echo "    To use a different password, set KEYSTORE_PASS / KEY_PASS / KEY_ALIAS"
echo "    before running this script."
