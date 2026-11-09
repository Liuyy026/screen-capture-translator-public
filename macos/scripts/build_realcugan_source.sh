#!/bin/bash
# Build both PNG encoder variants from the pinned Real-CUGAN release source.
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1

if [[ $# != 3 ]]; then
  echo "Usage: $0 /absolute/path/libMoltenVK.a /absolute/path/Vulkan/include /absolute/path/OpenMP" >&2
  exit 2
fi
if [[ "$1" != /* || "$2" != /* || "$3" != /* || ! -f "$1" || \
      ! -f "$2/vulkan/vulkan.h" || ! -f "$3/include/omp.h" || ! -f "$3/lib/libomp.a" ]]; then
  echo "Provide absolute paths to MoltenVK, Vulkan headers and a static OpenMP installation." >&2
  exit 2
fi
if [[ "$(uname -m)" != arm64 ]]; then
  echo "This candidate build is for Apple Silicon." >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$ROOT/.runtime/super-resolution/realcugan/source-build"
SOURCE="$WORK/upstream"
PATCH="$ROOT/macos/patches/realcugan-fast-png.patch"
REVISION=395302c5c70f1bff604c974e92e0a87e45c9f9ee
MOLTENVK="$1"
HEADERS="$2"
OPENMP="$3"

if [[ -x "$ROOT/.runtime/build-tools/bin/cmake" ]]; then
  CMAKE="$ROOT/.runtime/build-tools/bin/cmake"
  NINJA="$ROOT/.runtime/build-tools/bin/ninja"
else
  CMAKE="$(command -v cmake)"
  NINJA="$(command -v ninja)"
fi

mkdir -p "$WORK"
if [[ ! -d "$SOURCE/.git" ]]; then
  git clone --depth 1 --branch 20220728 https://github.com/nihui/realcugan-ncnn-vulkan.git "$SOURCE"
fi
[[ "$(git -C "$SOURCE" rev-parse HEAD)" == "$REVISION" ]] || {
  echo "Unexpected upstream revision in $SOURCE" >&2
  exit 1
}
git -C "$SOURCE" submodule update --init --depth 1 -- src/libwebp src/ncnn
git -C "$SOURCE/src/ncnn" submodule update --init --depth 1 -- glslang
if git -C "$SOURCE" diff --quiet -- src/CMakeLists.txt src/main.cpp; then
  git -C "$SOURCE" apply --unidiff-zero "$PATCH"
fi
git -C "$SOURCE" diff --unified=0 -- src/CMakeLists.txt src/main.cpp | cmp - "$PATCH" || {
  echo "Unexpected changes in the source files used by the PNG patch." >&2
  exit 1
}

LIB_SHA="$(shasum -a 256 "$MOLTENVK" | cut -d ' ' -f 1)"
OMP_SHA="$(shasum -a 256 "$OPENMP/lib/libomp.a" | cut -d ' ' -f 1)"
DELIVERY="$WORK/builds/${LIB_SHA:0:12}-omp${OMP_SHA:0:8}"
BUILD="$DELIVERY/build"
mkdir -p "$DELIVERY/baseline" "$DELIVERY/fast"
if [[ -f "$DELIVERY/moltenvk.sha256" && "$(cat "$DELIVERY/moltenvk.sha256")" != "$LIB_SHA" ]]; then
  echo "Different MoltenVK library already has this build prefix." >&2
  exit 1
fi
printf '%s\n' "$LIB_SHA" > "$DELIVERY/moltenvk.sha256"
printf '%s\n' "$OMP_SHA" > "$DELIVERY/openmp.sha256"
for variant in baseline fast; do
  mode=OFF
  if [[ "$variant" == fast ]]; then mode=ON; fi
  "$CMAKE" -S "$SOURCE/src" -B "$BUILD" -G Ninja \
    -DCMAKE_MAKE_PROGRAM="$NINJA" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_OSX_DEPLOYMENT_TARGET=11.0 \
    -DUSE_STATIC_MOLTENVK=ON -DVulkan_INCLUDE_DIR="$HEADERS" -DVulkan_LIBRARY="$MOLTENVK" \
    -DNCNN_OPENMP=ON \
    -DOpenMP_C_FLAGS="-Xclang -fopenmp -I\"$OPENMP/include\"" \
    -DOpenMP_CXX_FLAGS="-Xclang -fopenmp -I\"$OPENMP/include\"" \
    -DOpenMP_C_LIB_NAMES=libomp -DOpenMP_CXX_LIB_NAMES=libomp \
    -DOpenMP_libomp_LIBRARY="$OPENMP/lib/libomp.a" \
    -DREALCUGAN_FAST_PNG="$mode"
  "$CMAKE" --build "$BUILD" --parallel 8 --target realcugan-ncnn-vulkan
  cp "$BUILD/realcugan-ncnn-vulkan" "$DELIVERY/$variant/realcugan-ncnn-vulkan"
  model_dir="$WORK/../realcugan-ncnn-vulkan-20220728-macos/models-se"
  if [[ -d "$model_dir" && ! -e "$DELIVERY/$variant/models-se" ]]; then
    ln -s "$model_dir" "$DELIVERY/$variant/models-se"
  fi
done
shasum -a 256 "$DELIVERY"/{baseline,fast}/realcugan-ncnn-vulkan
printf 'Build outputs: %s\n' "$DELIVERY"
