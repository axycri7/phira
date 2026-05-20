#!/usr/bin/env bash
set -euo pipefail

TARGET="${TARGET:-aarch64-linux-android}"
ANDROID_API="${ANDROID_API:-35}"
RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-nightly-2026-01-01}"
ANGLE="${ANGLE:-1}"
ANGLE_RELEASE_REPO="${ANGLE_RELEASE_REPO:-axycri7/build-angle}"

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

ANGLE_LIBS=(
    libEGL_angle.so
    libGLESv2_angle.so
    libGLESv1_CM_angle.so
    libEGL.so
    libGLESv2.so
    libGLESv1_CM.so
)

patch_angle_smali() {
    local decode_dir="$1"
    local prefer_angle="$2"
    local angle_bool="0x0"
    if [[ "${prefer_angle}" == "1" ]]; then
        angle_bool="0x1"
    fi

    local quad_native_smali
    quad_native_smali=$(find "${decode_dir}" -path "*/quad_native/QuadNative.smali" | head -n 1)
    [[ -n "${quad_native_smali}" ]] || { echo "ERROR: quad_native/QuadNative.smali not found"; exit 1; }

    local main_activity_smali
    main_activity_smali=$(find "${decode_dir}" -path "*/MainActivity.smali" -print0 \
        | xargs -0 -r grep -l 'Lquad_native/QuadNative;->initializeContext(Landroid/content/Context;)V' \
        | head -n 1 || true)
    [[ -n "${main_activity_smali}" ]] || {
        echo "ERROR: MainActivity smali initializeContext(Context) call not found"
        exit 1
    }

    sed -i \
        's|initializeContext(Landroid/content/Context;)V|initializeContext(Landroid/content/Context;Z)V|g' \
        "${quad_native_smali}"

    sed -i \
        "s|invoke-static {p0}, Lquad_native/QuadNative;->initializeContext(Landroid/content/Context;)V|const/4 v0, ${angle_bool}\\n\\n    invoke-static {p0, v0}, Lquad_native/QuadNative;->initializeContext(Landroid/content/Context;Z)V|" \
        "${main_activity_smali}"

    grep -q 'initializeContext(Landroid/content/Context;Z)V' "${quad_native_smali}" || {
        echo "ERROR: failed to patch QuadNative.initializeContext signature"
        exit 1
    }
    grep -q "const/4 v0, ${angle_bool}" "${main_activity_smali}" || {
        echo "ERROR: failed to patch MainActivity ANGLE preference"
        exit 1
    }

    echo "=== Patched Java smali ANGLE preference: ${prefer_angle} ==="
}

download_angle_libs_from_release() {
    local abi_dir="$1"
    local out_dir="$2"
    local asset_arch

    case "${abi_dir}" in
        arm64-v8a) asset_arch="arm64-v8a" ;;
        *)
            echo "ERROR: bundled ANGLE release only supports arm64-v8a for now, got ${abi_dir}"
            return 1
            ;;
    esac

    mkdir -p "${out_dir}"

    local zip_path="${out_dir}/angle.zip"
    local extract_dir="${out_dir}/extract"

    echo "=== Downloading latest ANGLE release from ${ANGLE_RELEASE_REPO} ==="
    local latest_url
    latest_url=$(curl -fsSL --retry 3 \
        -A "phira-android-build" \
        -o /dev/null \
        -w "%{url_effective}" \
        "https://github.com/${ANGLE_RELEASE_REPO}/releases/latest")

    local latest_tag="${latest_url##*/}"
    [[ -n "${latest_tag}" && "${latest_tag}" != "latest" ]] || {
        echo "ERROR: could not resolve latest release tag from ${ANGLE_RELEASE_REPO}"
        return 1
    }

    local asset_name="angle-android-${asset_arch}-${latest_tag}.zip"
    local asset_url="https://github.com/${ANGLE_RELEASE_REPO}/releases/latest/download/${asset_name}"
    echo "    Release: ${latest_tag}"
    echo "    URL: ${asset_url}"
    curl -fL --retry 3 \
        -A "phira-android-build" \
        -o "${zip_path}" \
        "${asset_url}"

    rm -rf "${extract_dir}"
    mkdir -p "${extract_dir}"
    unzip -q "${zip_path}" -d "${extract_dir}"

    local lib_dir="${extract_dir}/angle-android-${asset_arch}/lib"
    [[ -d "${lib_dir}" ]] || {
        echo "ERROR: ANGLE zip missing angle-android-${asset_arch}/lib"
        return 1
    }

    cp "${lib_dir}/libEGL.so" "${out_dir}/libEGL.so"
    cp "${lib_dir}/libEGL.so" "${out_dir}/libEGL_angle.so"
    cp "${lib_dir}/libGLESv2.so" "${out_dir}/libGLESv2.so"
    cp "${lib_dir}/libGLESv2.so" "${out_dir}/libGLESv2_angle.so"
    cp "${lib_dir}/libGLESv1_CM.so" "${out_dir}/libGLESv1_CM.so"
    cp "${lib_dir}/libGLESv1_CM.so" "${out_dir}/libGLESv1_CM_angle.so"

    for lib in "${ANGLE_LIBS[@]}"; do
        [[ -s "${out_dir}/${lib}" ]] || {
            echo "ERROR: missing or empty ANGLE lib ${out_dir}/${lib}"
            return 1
        }
    done

    echo "=== Using ANGLE libraries from ${ANGLE_RELEASE_REPO} latest release ==="
}

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
        LATEST_APK_URL=$(curl -fsSL "https://api.github.com/repos/TeamFlos/phira/releases/latest" \
            | jq -r --arg arch "${APK_ARCH}" '
                first(
                    .assets[]
                    | select(.browser_download_url | test($arch + "\\.apk$"))
                    | .browser_download_url
                ) // empty
            ')
        [[ -n "${LATEST_APK_URL}" ]] || { echo "ERROR: could not find ${APK_ARCH} APK in latest release"; exit 1; }
        echo "    URL: ${LATEST_APK_URL}"
        curl -fL --retry 3 -o "${APK_INPUT}" "${LATEST_APK_URL}"
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

    # ── Patch Java layer smali ────────────────────────────────────────────────
    # The current flow repacks an existing APK, so Java sources are not compiled.
    # Patch the decoded DEX smali to call the updated native initializeContext
    # hook with the build-time ANGLE preference.
    patch_angle_smali "${DECODE_DIR}" "${ANGLE}"

    # ── Replace native library ────────────────────────────────────────────────
    mkdir -p "${DECODE_DIR}/lib/${ABI_DIR}"
    cp "${SO_SRC}" "${DECODE_DIR}/lib/${ABI_DIR}/libphira.so"

    if [[ "${ANGLE}" == "1" ]]; then
        echo "=== Injecting bundled ANGLE libraries ==="
        ANGLE_SRC_DIR="${WORK_DIR}/angle-libs"
        download_angle_libs_from_release "${ABI_DIR}" "${ANGLE_SRC_DIR}"
        for lib in "${ANGLE_LIBS[@]}"; do
            [[ -f "${ANGLE_SRC_DIR}/${lib}" ]] || { echo "ERROR: missing ANGLE lib ${ANGLE_SRC_DIR}/${lib}"; exit 1; }
            [[ -s "${ANGLE_SRC_DIR}/${lib}" ]] || { echo "ERROR: empty ANGLE lib ${ANGLE_SRC_DIR}/${lib}"; exit 1; }
            cp "${ANGLE_SRC_DIR}/${lib}" "${DECODE_DIR}/lib/${ABI_DIR}/${lib}"
        done
    fi

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
