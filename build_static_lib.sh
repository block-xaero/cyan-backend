#!/bin/bash

# build-universal.sh
# Builds Cyan backend for all Apple platforms and creates XCFramework

set -e  # Exit on error

echo "🦀 Building Cyan Backend for all Apple platforms..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if required targets are installed
check_target() {
    if ! rustup target list --installed | grep -q "$1"; then
        echo -e "${YELLOW}Installing target: $1${NC}"
        rustup target add "$1"
    fi
}

# Install all required targets
echo "📦 Checking Rust targets..."
check_target "aarch64-apple-darwin"
check_target "x86_64-apple-darwin"
check_target "aarch64-apple-ios"
check_target "aarch64-apple-ios-sim"
check_target "x86_64-apple-ios"

# Clean previous builds
echo "🧹 Cleaning previous builds..."
rm -rf ./build
mkdir -p ./build

# Build for all targets
echo "🔨 Building for macOS (Apple Silicon)..."
cargo build --release --target aarch64-apple-darwin

echo "🔨 Building for macOS (Intel)..."
if cargo build --release --target x86_64-apple-darwin 2>/dev/null; then
    INTEL_MAC_BUILT=true
else
    echo -e "${YELLOW}Skipping Intel Mac build (cross-compilation not set up)${NC}"
    INTEL_MAC_BUILT=false
fi

echo "🔨 Building for iOS devices..."
cargo build --release --target aarch64-apple-ios

echo "🔨 Building for iOS Simulator (Apple Silicon)..."
cargo build --release --target aarch64-apple-ios-sim

echo "🔨 Building for iOS Simulator (Intel)..."
if cargo build --release --target x86_64-apple-ios 2>/dev/null; then
    INTEL_SIM_BUILT=true
else
    echo -e "${YELLOW}Skipping Intel Simulator build${NC}"
    INTEL_SIM_BUILT=false
fi

# Create universal libraries
echo "🔗 Creating universal libraries..."

# macOS universal binary
if [ "$INTEL_MAC_BUILT" = true ]; then
    lipo -create \
        target/aarch64-apple-darwin/release/libcyan_backend.a \
        target/x86_64-apple-darwin/release/libcyan_backend.a \
        -output build/libcyan_backend_macos.a
else
    cp target/aarch64-apple-darwin/release/libcyan_backend.a build/libcyan_backend_macos.a
fi

# iOS Simulator universal binary
if [ "$INTEL_SIM_BUILT" = true ]; then
    lipo -create \
        target/aarch64-apple-ios-sim/release/libcyan_backend.a \
        target/x86_64-apple-ios/release/libcyan_backend.a \
        -output build/libcyan_backend_simulator.a
else
    cp target/aarch64-apple-ios-sim/release/libcyan_backend.a build/libcyan_backend_simulator.a
fi

# iOS device library
cp target/aarch64-apple-ios/release/libcyan_backend.a build/libcyan_backend_ios.a

# Create XCFramework without headers
echo "📦 Creating XCFramework..."
xcodebuild -create-xcframework \
    -library build/libcyan_backend_macos.a \
    -library build/libcyan_backend_ios.a \
    -library build/libcyan_backend_simulator.a \
    -output build/CyanBackend.xcframework

# Copy to Xcode project if path is provided
if [ -n "$1" ]; then
    XCODE_PROJECT_PATH="$1"
    echo "📋 Copying to Xcode project at: $XCODE_PROJECT_PATH"
    rm -rf "$XCODE_PROJECT_PATH/CyanBackend.xcframework"
    cp -R build/CyanBackend.xcframework "$XCODE_PROJECT_PATH/"
    echo -e "${GREEN}✅ XCFramework copied to Xcode project${NC}"
fi

echo -e "${GREEN}✅ Build complete! XCFramework created at: build/CyanBackend.xcframework${NC}"

# Show framework info
echo ""
echo "📊 Framework contents:"
find build/CyanBackend.xcframework -name "*.a" -exec ls -lh {} \;

# Verify symbols are present
echo ""
echo "🔍 Verifying cyan symbols in macOS library:"
nm build/libcyan_backend_macos.a | grep "_cyan_" | head -5
