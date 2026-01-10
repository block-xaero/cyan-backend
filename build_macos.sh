#!/bin/zsh

# build_macos.sh
# Builds macOS-only XCFramework (skips iOS)
# Uses ONLY Apple's system clang - avoids Homebrew LLVM ABI issues
# Run from cyan-backend directory

set -e
setopt +o nomatch

# =============================================================================
# Re-link LLVM on exit (success or failure)
# =============================================================================
trap 'echo "🔧 Re-linking Homebrew LLVM..."; brew link llvm 2>/dev/null || true' EXIT

echo "🦀 Building Cyan Backend XCFramework (macOS only)..."

# =============================================================================
# Unlink Homebrew LLVM to avoid ABI conflicts
# =============================================================================
echo "🔧 Unlinking Homebrew LLVM..."
brew unlink llvm 2>/dev/null || true

# =============================================================================
# CRITICAL: Use Apple's system clang, NOT Homebrew LLVM
# =============================================================================

# Unset any Homebrew LLVM environment variables
unset CC CXX AR LIBCLANG_PATH CXXFLAGS LDFLAGS CMAKE_CXX_FLAGS

# Force Apple's system clang
export CC=/usr/bin/clang
export CXX=/usr/bin/clang++
export AR=/usr/bin/ar

# Use Xcode's LIBCLANG for bindgen
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"

# Deployment target
export MACOSX_DEPLOYMENT_TARGET=14.0

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

# =============================================================================
# Build macOS
# =============================================================================

echo ""
echo "🔨 Building for macOS (Apple Silicon)..."

export SDKROOT=$(xcrun --sdk macosx --show-sdk-path)
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=${SDKROOT}"

cargo build --release --target aarch64-apple-darwin 2>&1 | tee -a build.log

if [[ ${pipestatus[1]} -eq 0 ]]; then
    echo "✅ macOS build succeeded"
else
    echo "❌ macOS build FAILED"
    echo ""
    echo "Check build.log for details"
    exit 1
fi

# =============================================================================
# Create XCFramework (macOS only)
# =============================================================================

echo ""
echo "📦 Creating XCFramework..."

cp target/aarch64-apple-darwin/release/libcyan_backend.a build/libcyan_backend_macos.a

# Also copy dylib for development
if [[ -f target/aarch64-apple-darwin/release/libcyan_backend.dylib ]]; then
    cp target/aarch64-apple-darwin/release/libcyan_backend.dylib build/Cyan.debug.dylib
    echo "✅ Dylib copied: build/Cyan.debug.dylib"
fi

rm -rf build/CyanBackend.xcframework

xcodebuild -create-xcframework \
    -library build/libcyan_backend_macos.a \
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
echo "✅ BUILD COMPLETE (macOS only)"
echo ""
echo "📁 XCFramework: build/CyanBackend.xcframework"
echo "📁 Dylib:       build/Cyan.debug.dylib"
echo ""
find build/CyanBackend.xcframework -name "*.a" -exec ls -lh {} \;
echo ""
echo "🔍 Symbols:"
nm build/libcyan_backend_macos.a 2>/dev/null | grep "_cyan_" | head -3
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Note: LLVM will be re-linked automatically by the EXIT trap