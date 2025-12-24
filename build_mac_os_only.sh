#!/bin/zsh

# build_macos_only.sh
# FAST build for macOS only - use during development
# Run from cyan-backend directory

set -e

echo "🦀 Building Cyan Backend (macOS only - FAST MODE)..."

# =============================================================================
# Setup environment (minimal)
# =============================================================================

brew unlink llvm 2>/dev/null || true
trap 'brew link llvm 2>/dev/null || true' EXIT

export CC=/usr/bin/clang
export CXX=/usr/bin/clang++
export AR=/usr/bin/ar
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
export MACOSX_DEPLOYMENT_TARGET=14.0
export SDKROOT=$(xcrun --sdk macosx --show-sdk-path)

# =============================================================================
# Build macOS only (NO clean, NO iOS)
# =============================================================================

echo "🔨 Building for macOS (Apple Silicon)..."
cargo build --release --target aarch64-apple-darwin

if [[ $? -eq 0 ]]; then
    echo "✅ Build succeeded"
else
    echo "❌ Build FAILED"
    exit 1
fi

# =============================================================================
# Update XCFramework (macOS portion only)
# =============================================================================

mkdir -p ./build

# Just copy the macOS library
cp target/aarch64-apple-darwin/release/libcyan_backend.a build/libcyan_backend_macos.a

# If existing xcframework exists, update just the macos part
if [[ -d "build/CyanBackend.xcframework/macos-arm64" ]]; then
    cp build/libcyan_backend_macos.a build/CyanBackend.xcframework/macos-arm64/libcyan_backend.a
    echo "✅ Updated existing XCFramework (macOS only)"
elif [[ -d "build/CyanBackend.xcframework" ]]; then
    # Find the macos folder (might have different naming)
    MACOS_DIR=$(find build/CyanBackend.xcframework -type d -name "*macos*" | head -1)
    if [[ -n "$MACOS_DIR" ]]; then
        cp build/libcyan_backend_macos.a "$MACOS_DIR/libcyan_backend.a"
        echo "✅ Updated existing XCFramework at $MACOS_DIR"
    else
        echo "⚠️  Could not find macos directory in XCFramework"
        echo "   Run full build_static_lib.sh once first"
    fi
else
    echo "⚠️  No existing XCFramework found"
    echo "   Run full build_static_lib.sh once first, then use this for fast iterations"
fi

# =============================================================================
# Copy to Xcode project (optional)
# =============================================================================

if [[ -n "$1" ]]; then
    if [[ -d "build/CyanBackend.xcframework" ]]; then
        echo "📋 Copying to: $1"
        rm -rf "$1/CyanBackend.xcframework"
        cp -R build/CyanBackend.xcframework "$1/"
        echo "✅ Copied to Xcode project"
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ FAST BUILD COMPLETE (macOS only)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ls -lh build/libcyan_backend_macos.a