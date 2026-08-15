#!/usr/bin/env bash
set -euo pipefail

TARGET="${TARGET:-aarch64-linux-android}"
ANDROID_API="${ANDROID_API:-35}"
RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-nightly-2026-01-01}"

case "${TARGET}" in
    aarch64-linux-android | armv7-linux-androideabi) ;;
    *)
        echo "ERROR: unsupported TARGET=${TARGET}" >&2
        echo "Supported targets: aarch64-linux-android, armv7-linux-androideabi" >&2
        exit 1
        ;;
esac

# build.rs reads PRPR_AVC_LIBS and appends /<TARGET> to find the static libs.
# The Dockerfile pre-downloaded them to /ffmpeg-libs/<TARGET>/.
export PRPR_AVC_LIBS="/ffmpeg-libs"

# All outputs land in /out/<TARGET>/ on the host.
OUT_DIR="/out/${TARGET}"
mkdir -p "${OUT_DIR}"

echo "=== Building libphira.so for ${TARGET} ==="
cargo +${RUST_TOOLCHAIN} ndk \
    -t ${TARGET} \
    --platform ${ANDROID_API} \
    build --release \
    -p phira

SO_SRC="/src/target/${TARGET}/release/libphira.so"
SO_OUT="${OUT_DIR}/libphira.so"
echo "=== Build complete: ${SO_SRC} ==="
cp "${SO_SRC}" "${SO_OUT}"
echo "=== Copied to ${SO_OUT} ==="

# ── APK repack + sign ─────────────────────────────────────────────────────────
# Activated when SIGN_APK=1.  Requires:
#   ${APK_INPUT:-/apk/input.apk} — base APK (auto-downloaded if absent)
#   /keystore/keystore.jks + keystore.env

if [[ "${SIGN_APK:-0}" == "1" ]]; then
    echo "=== Repacking and signing APK ==="
    APK_INPUT="${APK_INPUT:-/apk/input.apk}"

    # Auto-download the latest official release APK if not provided by the host.
    if [[ ! -f "${APK_INPUT}" ]]; then
        echo "=== ${APK_INPUT} not found — downloading latest release APK ==="
        # Pick the APK asset matching the current TARGET architecture.
        case "${TARGET}" in
            aarch64-linux-android)  APK_ARCH="arm64-v8a" ;;
            armv7-linux-androideabi) APK_ARCH="armeabi-v7a" ;;
            *)                      APK_ARCH="arm64-v8a" ;;
        esac
        RELEASE_JSON=$(mktemp)
        APK_TMP="${APK_INPUT}.tmp"
        trap 'rm -f "${RELEASE_JSON}" "${APK_TMP}"' EXIT
        curl --no-alpn -fsSL --retry 5 --retry-all-errors \
            -o "${RELEASE_JSON}" \
            "https://api.github.com/repos/TeamFlos/phira/releases/latest"
        LATEST_APK_URL=$(jq -r --arg arch "${APK_ARCH}" '
            first(
                .assets[]
                | select(.name | test("^Phira-android-" + $arch + "-.*[.]apk$"))
                | .browser_download_url
            ) // empty
        ' "${RELEASE_JSON}")
        rm -f "${RELEASE_JSON}"
        [[ -n "${LATEST_APK_URL}" ]] || { echo "ERROR: could not find ${APK_ARCH} APK in latest release"; exit 1; }
        echo "    URL: ${LATEST_APK_URL}"
        curl --no-alpn -fL --retry 5 --retry-all-errors \
            -o "${APK_TMP}" "${LATEST_APK_URL}"
        mv "${APK_TMP}" "${APK_INPUT}"
        trap - EXIT
        echo "=== Downloaded to ${APK_INPUT} ==="
    fi

    [[ -f "${APK_INPUT}" ]]        || { echo "ERROR: ${APK_INPUT} not found"; exit 1; }
    [[ -f /keystore/keystore.jks ]] || { echo "ERROR: /keystore/keystore.jks not found"; exit 1; }
    [[ -f /keystore/keystore.env ]] || { echo "ERROR: /keystore/keystore.env not found"; exit 1; }

    # shellcheck source=/dev/null
    source /keystore/keystore.env

    WORK_DIR=$(mktemp -d)
    cleanup() {
        rm -rf "${WORK_DIR}"
    }
    trap cleanup EXIT

    # Determine the ABI directory name inside the APK.
    case "${TARGET}" in
        aarch64-linux-android)   ABI_DIR="arm64-v8a" ;;
        armv7-linux-androideabi) ABI_DIR="armeabi-v7a" ;;
        *)                       ABI_DIR="arm64-v8a" ;;
    esac

    # ── Decode APK with apktool ───────────────────────────────────────────────
    DECODE_DIR="${WORK_DIR}/decoded"
    apktool d -f -o "${DECODE_DIR}" "${APK_INPUT}"

    # ── Replace native library ────────────────────────────────────────────────
    mkdir -p "${DECODE_DIR}/lib/${ABI_DIR}"
    cp "${SO_SRC}" "${DECODE_DIR}/lib/${ABI_DIR}/libphira.so"

    # ── Rename package ────────────────────────────────────────────────────────
    PKG_NEW="${PKG_NAME:-org.flos.phira}"
    PKG_OLD=$(grep -m1 'package=' "${DECODE_DIR}/AndroidManifest.xml" \
        | sed 's/.*package="\([^"]*\)".*/\1/')
    echo "=== Renaming package: ${PKG_OLD} → ${PKG_NEW} ==="

    # Change the app identity so this build coexists with the official one.
    # 1. Patch the package= attribute (app identity / install slot).
    # 2. Patch custom <permission> and <uses-permission> declarations that embed
    #    the old package name — Android rejects installing two apps that declare
    #    the same permission name.
    # Activity android:name values are intentionally left as org.flos.phira.*
    # because those are the actual DEX class names; changing them causes
    # ClassNotFoundException at launch.
    sed -i "s|package=\"${PKG_OLD}\"|package=\"${PKG_NEW}\"|g" \
        "${DECODE_DIR}/AndroidManifest.xml"
    sed -i "/<[/a-z-]*permission/s|android:name=\"${PKG_OLD}\.|android:name=\"${PKG_NEW}.|g" \
        "${DECODE_DIR}/AndroidManifest.xml"
    sed -i "s|android:authorities=\"${PKG_OLD}\.|android:authorities=\"${PKG_NEW}.|g" \
        "${DECODE_DIR}/AndroidManifest.xml"

    sed -i "s|renameManifestPackage:.*|renameManifestPackage: null|" \
        "${DECODE_DIR}/apktool.yml" 2>/dev/null || true

    # ── Rebuild APK with apktool ──────────────────────────────────────────────
    apktool b "${DECODE_DIR}" -o "${WORK_DIR}/repacked.apk"

    UNSIGNED_OUT="${OUT_DIR}/phira-unsigned.apk"
    SIGNED_OUT="${OUT_DIR}/phira-signed.apk"

    # Align (4-byte boundary required for resources.arsc and .so on R+).
    zipalign -f 4 "${WORK_DIR}/repacked.apk" "${WORK_DIR}/aligned.apk"

    # Produce unsigned (aligned but not signed) for reference.
    cp "${WORK_DIR}/aligned.apk" "${UNSIGNED_OUT}"
    echo "=== Unsigned APK → ${UNSIGNED_OUT} ==="

    # Sign.
    apksigner sign \
        --ks /keystore/keystore.jks \
        --ks-pass "pass:${KEYSTORE_PASS}" \
        --ks-key-alias "${KEY_ALIAS}" \
        --key-pass "pass:${KEY_PASS}" \
        --out "${SIGNED_OUT}" \
        "${WORK_DIR}/aligned.apk"

    apksigner verify --verbose "${SIGNED_OUT}"
    echo "=== Signed APK   → ${SIGNED_OUT} ==="
fi
