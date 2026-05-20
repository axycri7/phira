#!/usr/bin/env bash
# build-android.sh — build phira for Android
# Usage:
#   ./build-android.sh                        # build .so + repack + sign APK (default)
#   SIGN_APK=0 ./build-android.sh             # build .so only
#   TARGET=armv7-linux-androideabi ./build-android.sh
#
# Outputs land in:
#   target/android-<arch>/libphira.so
#   target/android-<arch>/phira-unsigned.apk
#   target/android-<arch>/phira-signed.apk
#
# On first run a signing key is auto-generated in the "phira-keystore" volume.
# If ./apk/input.apk is absent the base APK is downloaded automatically.
set -euo pipefail

TARGET="${TARGET:-aarch64-linux-android}"
SIGN_APK="${SIGN_APK:-1}"
PKG_NAME="${PKG_NAME:-i7.axycr.phiradbg}"

IMAGE="phira-android-builder"
CONTAINER_NAME="phira-android-build"

# Derive a short arch label for the output directory (android-arm64, etc.)
case "${TARGET}" in
    aarch64-linux-android)   ARCH_LABEL="arm64" ;;
    armv7-linux-androideabi) ARCH_LABEL="arm32" ;;
    x86_64-linux-android)   ARCH_LABEL="x86_64" ;;
    *)                       ARCH_LABEL="${TARGET}" ;;
esac
OUT_DIR="$(pwd)/target/android-${ARCH_LABEL}"
mkdir -p "${OUT_DIR}"

# ── Volume names (persist across builds) ─────────────────────────────────────
VOL_CARGO_REGISTRY="phira-cargo-registry"
VOL_CARGO_GIT="phira-cargo-git"
VOL_RUST_TARGET="phira-rust-target"
VOL_ANDROID_SDK="phira-android-sdk"
VOL_ANDROID_NDK="phira-android-ndk"
VOL_KEYSTORE="phira-keystore"

# ── Build the Docker image ────────────────────────────────────────────────────
echo "=== Building Docker image (linux/amd64) ==="
docker buildx build \
    --platform linux/amd64 \
    --load \
    -f Dockerfile.android \
    -t "${IMAGE}" \
    .

# ── Auto-generate keystore on first run ──────────────────────────────────────
if [[ "${SIGN_APK}" == "1" ]]; then
    docker volume inspect "${VOL_KEYSTORE}" > /dev/null 2>&1 || docker volume create "${VOL_KEYSTORE}"
    KEYSTORE_EXISTS=$(docker run --rm --platform linux/amd64 \
        -v "${VOL_KEYSTORE}:/keystore" \
        --entrypoint bash \
        "${IMAGE}" \
        -c '[[ -f /keystore/keystore.jks ]] && echo yes || echo no')
    if [[ "${KEYSTORE_EXISTS}" != "yes" ]]; then
        echo "=== Generating signing keystore (first run) ==="
        docker run --rm --platform linux/amd64 \
            -v "${VOL_KEYSTORE}:/keystore" \
            --entrypoint bash \
            "${IMAGE}" \
            -c '
                set -e
                keytool -genkeypair \
                    -keystore /keystore/keystore.jks \
                    -alias phira \
                    -keyalg RSA -keysize 2048 -validity 10000 \
                    -storepass phira-build \
                    -keypass phira-build \
                    -dname "CN=Phira Build, O=Self-Signed, C=US"
                printf "export KEYSTORE_PASS=\"phira-build\"\nexport KEY_ALIAS=\"phira\"\nexport KEY_PASS=\"phira-build\"\n" \
                    > /keystore/keystore.env
                echo "Keystore generated."
            '
    fi
fi

# ── Create named volumes if they don't exist ──────────────────────────────────
for vol in \
    "${VOL_CARGO_REGISTRY}" \
    "${VOL_CARGO_GIT}" \
    "${VOL_RUST_TARGET}" \
    "${VOL_ANDROID_SDK}" \
    "${VOL_ANDROID_NDK}" \
    "${VOL_KEYSTORE}"; do
    docker volume inspect "${vol}" > /dev/null 2>&1 || docker volume create "${vol}"
done

# ── Run the build ─────────────────────────────────────────────────────────────
DOCKER_ARGS=(
    --rm
    --name "${CONTAINER_NAME}"
    --platform linux/amd64
    -e "TARGET=${TARGET}"
    -e "SIGN_APK=${SIGN_APK}"
    -e "PKG_NAME=${PKG_NAME}"

    -v "$(pwd):/src"
    -v "${VOL_CARGO_REGISTRY}:/cargo/registry"
    -v "${VOL_CARGO_GIT}:/cargo/git"
    -v "${VOL_RUST_TARGET}:/src/target"
    -v "${VOL_ANDROID_SDK}:/opt/android-sdk"
    -v "${VOL_ANDROID_NDK}:/opt/android-ndk-r27c"
    -v "${VOL_KEYSTORE}:/keystore"

    # Output dir: host target/android-<arch>/ ↔ container /out/<TARGET>/
    -v "${OUT_DIR}:/out/${TARGET}"
)

if [[ "${SIGN_APK}" == "1" ]]; then
    # ./apk/input.apk is optional — entrypoint downloads it if absent
    mkdir -p "$(pwd)/apk"
    DOCKER_ARGS+=(-v "$(pwd)/apk:/apk")
fi

echo "=== Starting build container ==="
docker run "${DOCKER_ARGS[@]}" "${IMAGE}"

echo ""
echo "=== Done ==="
echo "    .so          → target/android-${ARCH_LABEL}/libphira.so"
if [[ "${SIGN_APK}" == "1" ]]; then
    echo "    unsigned APK → target/android-${ARCH_LABEL}/phira-unsigned.apk"
    echo "    signed APK   → target/android-${ARCH_LABEL}/phira-signed.apk"
fi
