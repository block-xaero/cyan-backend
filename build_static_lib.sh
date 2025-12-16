#!/bin/zsh

# build_static_lib.sh
# Uses ONLY Apple's system clang - avoids Homebrew LLVM ABI issues
# Run from cyan-backend directory

set -e
setopt +o nomatch

# =============================================================================
# Re-link LLVM on exit (success or failure)
# =============================================================================
trap 'echo "🔧 Re-linking Homebrew LLVM..."; brew link llvm 2>/dev/null || true' EXIT

echo "🦀 Building Cyan Backend XCFramework (Apple Clang)..."

# =============================================================================
# Unlink Homebrew LLVM to avoid ABI conflicts
# =============================================================================
echo "🔧 Unlinking Homebrew LLVM..."
brew unlink llvm 2>/dev/null || true

# =============================================================================
# CRITICAL: Use Apple's system clang, NOT Homebrew LLVM
# =============================================================================

# Unset any Homebrew LLVM environment variables
unset CC
unset CXX
unset AR
unset LIBCLANG_PATH
unset CXXFLAGS
unset LDFLAGS
unset CMAKE_CXX_FLAGS

# Force Apple's system clang
export CC=/usr/bin/clang
export CXX=/usr/bin/clang++
export AR=/usr/bin/ar

# Use Xcode's LIBCLANG for bindgen
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"

# Deployment targets
export MACOSX_DEPLOYMENT_TARGET=14.0
export IPHONEOS_DEPLOYMENT_TARGET=17.0

echo "🔧 Using Apple System Clang:"
echo "   CC:  $CC"
echo "   CXX: $CXX"
echo "   LIBCLANG_PATH: $LIBCLANG_PATH"
$CC --version | head -1

# =============================================================================
# Clean everything - CRITICAL for ABI fix
# =============================================================================

echo "🧹 Nuclear clean of all build artifacts..."

cargo clean

# Remove llama-cpp from cargo cache
rm -rf ~/.cargo/registry/cache/*/llama-cpp-* 2>/dev/null || true
rm -rf ~/.cargo/git/checkouts/llama-cpp-* 2>/dev/null || true

# Clean build directory
rm -rf ./build
mkdir -p ./build

echo "✓ Clean complete"

# =============================================================================
# Check Rust targets
# =============================================================================

check_target() {
    if ! rustup target list --installed | grep -q "$1"; then
        echo "Installing target: $1"
        rustup target add "$1"
    fi
}

echo "📦 Checking Rust targets..."
check_target "aarch64-apple-darwin"
check_target "aarch64-apple-ios"
check_target "aarch64-apple-ios-sim"

# =============================================================================
# Build each target
# =============================================================================

build_target() {
    local TARGET=$1
    local SDK=$2
    local DESC=$3

    echo ""
    echo "🔨 Building for ${DESC}..."

    export SDKROOT=$(xcrun --sdk $SDK --show-sdk-path)

    # Set bindgen to use the SDK's headers
    if [[ "$SDK" == "iphoneos" ]]; then
        export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=${SDKROOT} -target arm64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}"
    elif [[ "$SDK" == "iphonesimulator" ]]; then
        export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=${SDKROOT} -target arm64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}-simulator"
    else
        export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=${SDKROOT}"
    fi

    cargo build --release --target "$TARGET" 2>&1 | tee -a build.log

    if [[ ${pipestatus[1]} -eq 0 ]]; then
        echo "✅ ${DESC} build succeeded"
    else
        echo "❌ ${DESC} build FAILED"
        echo ""
        echo "Check build.log for details"
        exit 1
    fi
}

# Build all targets
build_target "aarch64-apple-darwin" "macosx" "macOS (Apple Silicon)"
build_target "aarch64-apple-ios" "iphoneos" "iOS Device"
build_target "aarch64-apple-ios-sim" "iphonesimulator" "iOS Simulator"

# =============================================================================
# Create XCFramework
# =============================================================================

echo ""
echo "📦 Creating XCFramework..."

cp target/aarch64-apple-darwin/release/libcyan_backend.a build/libcyan_backend_macos.a
cp target/aarch64-apple-ios/release/libcyan_backend.a build/libcyan_backend_ios.a
cp target/aarch64-apple-ios-sim/release/libcyan_backend.a build/libcyan_backend_simulator.a

rm -rf build/CyanBackend.xcframework

xcodebuild -create-xcframework \
    -library build/libcyan_backend_macos.a \
    -library build/libcyan_backend_ios.a \
    -library build/libcyan_backend_simulator.a \
    -output build/CyanBackend.xcframework

echo "✅ XCFramework created"

# =============================================================================
# Copy to Xcode project (optional)
# =============================================================================

if [[ -n "$1" ]]; then
    echo "📋 Copying to: $1"
    rm -rf "$1/CyanBackend.xcframework"
    cp -R build/CyanBackend.xcframework "$1/"
    echo "✅ Copied to Xcode project"
fi

# =============================================================================
# Summary
# =============================================================================

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ BUILD COMPLETE"
echo ""
echo "📁 XCFramework: build/CyanBackend.xcframework"
echo ""
find build/CyanBackend.xcframework -name "*.a" -exec ls -lh {} \;
echo ""
echo "🔍 Symbols:"
nm build/libcyan_backend_macos.a 2>/dev/null | grep "_cyan_" | head -3
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Note: LLVM will be re-linked automatically by the EXIT trap