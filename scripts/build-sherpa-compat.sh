#!/usr/bin/env bash
# Build the macOS 11-compatible sherpa-onnx Python module that PA installs on
# macOS older than 13.4 (see src-tauri/src/voice/sherpa_compat.rs).
#
# The PyPI wheel for sherpa-onnx 1.12.26 bundles onnxruntime 1.23.2, which needs
# macOS 13.4, and its _sherpa_onnx module imports std::to_chars(double) from the
# system libc++, which macOS 12 does not have. This script rebuilds the same
# sherpa-onnx release against onnxruntime 1.19.2 (macOS 11.0) with a macOS 11.0
# deployment target.
#
# Usage:  scripts/build-sherpa-compat.sh <x86_64|arm64> [out_dir]
# Env:    SHERPA_VERSION (1.12.26)  ORT_VERSION (1.19.2)  ORT_SHA256 (required
#         when ORT_VERSION is overridden)  PYTHON (/usr/bin/python3, must be 3.9)
#         WORK_DIR (build/sherpa-compat)  JOBS (cpu count)
#
# Output: <out_dir>/sherpa-onnx-compat-<sherpa>-ort<ort>-macos11-<arch>-cp39.tar.gz
#         and the same name + .sha256. The tarball holds exactly two files:
#         _sherpa_onnx.cpython-39-darwin.so and libonnxruntime.<ort>.dylib.
set -euo pipefail

ARCH="${1:-}"
OUT_DIR="${2:-dist}"
SHERPA_VERSION="${SHERPA_VERSION:-1.12.26}"
ORT_VERSION="${ORT_VERSION:-1.19.2}"
PYTHON="${PYTHON:-/usr/bin/python3}"
WORK_DIR="${WORK_DIR:-build/sherpa-compat}"
JOBS="${JOBS:-$(sysctl -n hw.ncpu)}"
DEPLOYMENT_TARGET="11.0"
CMAKE_PIP_VERSION="3.31.6"

die() { echo "error: $*" >&2; exit 1; }

case "$ARCH" in
  x86_64|arm64) ;;
  *) die "usage: $0 <x86_64|arm64> [out_dir]" ;;
esac
[[ "$(uname -s)" == "Darwin" ]] || die "must run on macOS"

# Upstream publishes no checksums for these archives; the values below were
# recorded from the v1.19.2 GitHub release when this script was written.
if [[ -z "${ORT_SHA256:-}" ]]; then
  [[ "$ORT_VERSION" == "1.19.2" ]] || die "set ORT_SHA256 when overriding ORT_VERSION"
  case "$ARCH" in
    x86_64) ORT_SHA256="6536e36d7ea92e32d53dad7ddd0fdf10be5b62d1dace85a13e1295ff81e9b5d4" ;;
    arm64)  ORT_SHA256="370c49770e2e1f243e17c7b227bb7f4b3da793b847d02f38016dc0e46c30fbe1" ;;
  esac
fi

PY_VER="$("$PYTHON" -c 'import sys; print("%d.%d" % sys.version_info[:2])')"
[[ "$PY_VER" == "3.9" ]] || die "$PYTHON is Python $PY_VER; PA's venv uses 3.9 (cp39)"
PY_INCLUDE="$("$PYTHON" -c 'import os, sys, sysconfig
p = sysconfig.get_config_var("INCLUDEPY")
print(p if os.path.isfile(os.path.join(p, "Python.h")) else os.path.join(sys.base_prefix, "Headers"))')"
[[ -f "$PY_INCLUDE/Python.h" ]] || die "Python.h not found (looked in $PY_INCLUDE)"

MODULE="_sherpa_onnx.cpython-39-darwin.so"
ORT_LIB="libonnxruntime.${ORT_VERSION}.dylib"
NAME="sherpa-onnx-compat-${SHERPA_VERSION}-ort${ORT_VERSION}-macos11-${ARCH}-cp39"

mkdir -p "$WORK_DIR" "$OUT_DIR"
WORK_DIR="$(cd "$WORK_DIR" && pwd)"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

# 1. Build tools in their own venv (CMake 4 drops policies sherpa's deps still use).
if [[ ! -x "$WORK_DIR/tools/bin/cmake" ]]; then
  "$PYTHON" -m venv "$WORK_DIR/tools"
  "$WORK_DIR/tools/bin/pip" install --disable-pip-version-check "cmake==${CMAKE_PIP_VERSION}"
fi
CMAKE="$WORK_DIR/tools/bin/cmake"

# 2. onnxruntime, checksum-verified before extraction.
ORT_TGZ="$WORK_DIR/onnxruntime-osx-${ARCH}-${ORT_VERSION}.tgz"
ORT_DIR="$WORK_DIR/onnxruntime-osx-${ARCH}-${ORT_VERSION}"
if [[ ! -f "$ORT_TGZ" ]]; then
  curl -fL --proto '=https' --tlsv1.2 -o "$ORT_TGZ.part" \
    "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-osx-${ARCH}-${ORT_VERSION}.tgz"
  mv "$ORT_TGZ.part" "$ORT_TGZ"
fi
echo "${ORT_SHA256}  ${ORT_TGZ}" | shasum -a 256 -c - || { rm -f "$ORT_TGZ"; die "onnxruntime checksum mismatch"; }
rm -rf "$ORT_DIR" && mkdir -p "$ORT_DIR"
tar -xzf "$ORT_TGZ" -C "$ORT_DIR" --strip-components 1

# 3. sherpa-onnx source at the release tag.
SRC="$WORK_DIR/sherpa-onnx-${SHERPA_VERSION}"
if [[ ! -d "$SRC/.git" ]]; then
  git -c advice.detachedHead=false clone --depth 1 --branch "v${SHERPA_VERSION}" \
    https://github.com/k2-fsa/sherpa-onnx.git "$SRC"
fi
echo "sherpa-onnx commit: $(git -C "$SRC" rev-parse HEAD)"

# 4. Configure and build only the Python extension.
BUILD="$WORK_DIR/build-${ARCH}"
rm -rf "$BUILD"
export MACOSX_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET"
SHERPA_ONNXRUNTIME_INCLUDE_DIR="$ORT_DIR/include" SHERPA_ONNXRUNTIME_LIB_DIR="$ORT_DIR/lib" \
"$CMAKE" -S "$SRC" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES="$ARCH" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET" \
  -DSHERPA_ONNX_USE_PRE_INSTALLED_ONNXRUNTIME_IF_AVAILABLE=ON \
  -DSHERPA_ONNX_ENABLE_PYTHON=ON \
  -DBUILD_SHARED_LIBS=ON \
  -DSHERPA_ONNX_ENABLE_C_API=OFF \
  -DSHERPA_ONNX_ENABLE_BINARY=OFF \
  -DSHERPA_ONNX_ENABLE_PORTAUDIO=OFF \
  -DSHERPA_ONNX_ENABLE_WEBSOCKET=OFF \
  -DSHERPA_ONNX_ENABLE_TESTS=OFF \
  -DSHERPA_ONNX_ENABLE_CHECK=OFF \
  -DBUILD_PIPER_PHONMIZE_EXE=OFF \
  -DBUILD_PIPER_PHONMIZE_TESTS=OFF \
  -DBUILD_ESPEAK_NG_EXE=OFF \
  -DBUILD_ESPEAK_NG_TESTS=OFF \
  -DPython_EXECUTABLE="$PYTHON" \
  -DPython_INCLUDE_DIR="$PY_INCLUDE"
"$CMAKE" --build "$BUILD" --target _sherpa_onnx -j "$JOBS"

# 5. Stage the two files; drop build-machine rpaths and re-sign.
STAGE="$WORK_DIR/stage-${ARCH}"
rm -rf "$STAGE" && mkdir -p "$STAGE"
BUILT="$(find "$BUILD" -name "$MODULE" -type f | head -n 1)"
[[ -n "$BUILT" ]] || die "$MODULE was not produced"
cp "$BUILT" "$STAGE/$MODULE"
cp "$ORT_DIR/lib/$ORT_LIB" "$STAGE/$ORT_LIB"
chmod 0644 "$STAGE/$MODULE" "$STAGE/$ORT_LIB"
otool -l "$STAGE/$MODULE" | awk '/LC_RPATH/ {getline; getline; print $2}' | while read -r rp; do
  install_name_tool -delete_rpath "$rp" "$STAGE/$MODULE"
done
install_name_tool -add_rpath @loader_path "$STAGE/$MODULE"
codesign --force --sign - "$STAGE/$MODULE" "$STAGE/$ORT_LIB"

# 6. Verify what PA's installer and macOS 11/12 rely on.
for f in "$MODULE" "$ORT_LIB"; do
  archs="$(lipo -archs "$STAGE/$f")"
  [[ "$archs" == "$ARCH" ]] || die "$f architectures are '$archs', expected $ARCH"
  minos="$(otool -l "$STAGE/$f" | awk '/LC_BUILD_VERSION/ {b=1} b && $1=="minos" {print $2; exit}')"
  [[ -n "$minos" ]] || die "$f has no LC_BUILD_VERSION"
  [[ "$(printf '%s\n%s\n' "$minos" "$DEPLOYMENT_TARGET" | sort -V | tail -n 1)" == "$DEPLOYMENT_TARGET" ]] \
    || die "$f minos $minos is newer than $DEPLOYMENT_TARGET"
  if nm -u "$STAGE/$f" | grep -q to_chars; then die "$f imports std::to_chars"; fi
done
otool -L "$STAGE/$MODULE" | tail -n +2 | awk '{print $1}' | while read -r dep; do
  case "$dep" in
    "@rpath/$ORT_LIB"|/usr/lib/*|/System/Library/*) ;;
    *) die "$MODULE depends on unexpected library $dep" ;;
  esac
done

# 7. Package.
TARBALL="$OUT_DIR/$NAME.tar.gz"
COPYFILE_DISABLE=1 tar --no-mac-metadata --uid 0 --gid 0 -czf "$TARBALL" -C "$STAGE" "$MODULE" "$ORT_LIB"
(cd "$OUT_DIR" && shasum -a 256 "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
cat "$TARBALL.sha256"
