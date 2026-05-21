#!/usr/bin/env bash
# build_android.sh — build phira for Android
# Usage:
#   ./build_android.sh                        # build arm64 .so + repack + sign APK (default)
#   SIGN_APK=0 ./build_android.sh             # build .so only
#   TARGET=armv7-linux-androideabi ./build_android.sh
#   TARGET=ALL ./build_android.sh             # build arm64 + arm32 APKs
#
# Outputs land in:
#   target/android-<arch>/libphira.so
#   target/android-<arch>/phira-unsigned.apk
#   target/android-<arch>/phira-signed.apk
#
# On first run a signing key is auto-generated in the "phira-keystore" volume.
# If ./apk/input.apk is absent, single-target builds download the base APK automatically.
# TARGET=ALL uses per-ABI caches: ./apk/input-arm64.apk and ./apk/input-arm32.apk.
set -euo pipefail

TARGET="${TARGET:-aarch64-linux-android}"
SIGN_APK="${SIGN_APK:-1}"
PKG_NAME="${PKG_NAME:-i7.axycr.phiradbg}"

IMAGE="phira-android-builder"
CONTAINER_NAME="phira-android-build"
SUPPORTED_TARGETS=(
    "aarch64-linux-android"
    "armv7-linux-androideabi"
)

arch_label_for_target() {
    case "$1" in
        aarch64-linux-android)   echo "arm64" ;;
        armv7-linux-androideabi) echo "arm32" ;;
        *)
            echo "ERROR: unsupported TARGET=$1" >&2
            echo "Supported targets:" >&2
            echo "  aarch64-linux-android" >&2
            echo "  armv7-linux-androideabi" >&2
            echo "  ALL" >&2
            exit 1
            ;;
    esac
}

if [[ "${TARGET}" == "ALL" ]]; then
    BUILD_TARGETS=("${SUPPORTED_TARGETS[@]}")
else
    # Validate early and normalize to an array so the rest of the script has one path.
    arch_label_for_target "${TARGET}" > /dev/null
    BUILD_TARGETS=("${TARGET}")
fi

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

run_android_build() {
    local build_target="$1"
    local arch_label
    local out_dir
    local apk_input

    arch_label="$(arch_label_for_target "${build_target}")"
    out_dir="$(pwd)/target/android-${arch_label}"
    mkdir -p "${out_dir}"

    DOCKER_ARGS=(
        --rm
        --name "${CONTAINER_NAME}-${arch_label}"
        --platform linux/amd64
        -e "TARGET=${build_target}"
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
        -v "${out_dir}:/out/${build_target}"
    )

    if [[ "${SIGN_APK}" == "1" ]]; then
        mkdir -p "$(pwd)/apk"
        if [[ "${TARGET}" == "ALL" ]]; then
            apk_input="/apk/input-${arch_label}.apk"
        else
            apk_input="/apk/input.apk"
        fi
        DOCKER_ARGS+=(
            -e "APK_INPUT=${apk_input}"
            -v "$(pwd)/apk:/apk"
        )
    fi

    echo "=== Starting build container for ${build_target} (${arch_label}) ==="
    docker run "${DOCKER_ARGS[@]}" "${IMAGE}"

    echo ""
    echo "=== Done: ${build_target} ==="
    echo "    .so          → target/android-${arch_label}/libphira.so"
    if [[ "${SIGN_APK}" == "1" ]]; then
        echo "    unsigned APK → target/android-${arch_label}/phira-unsigned.apk"
        echo "    signed APK   → target/android-${arch_label}/phira-signed.apk"
    fi
}

for build_target in "${BUILD_TARGETS[@]}"; do
    run_android_build "${build_target}"
done

if [[ "${TARGET}" == "ALL" ]]; then
    echo ""
    echo "=== All Android targets done ==="
fi
