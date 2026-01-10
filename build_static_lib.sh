#!/bin/zsh

# build_static_lib.sh
# Uses ONLY Apple's system clang - avoids Homebrew LLVM ABI issues
# Run from cyan-backend directory
#
# Usage: ./build_static_lib.sh [OPTIONS]
#
# Options:
#   --clean           Full cargo clean before build
#   --test            Also build test binaries (snapshot_test, network_test)
#   --unit            Run unit tests after build (no network needed)
#   --host            After build, run as snapshot host (waits for joiner)
#   --join <NODE_ID>  After build, connect to host and download snapshot
#   --skip-lib        Skip library build (only build/run tests)
#   --deploy          SCP test binary to Aria's laptop
#   --remote-join <NODE_ID>  Deploy + run join on Aria's laptop

set -e
setopt +o nomatch
export IPHONEOS_DEPLOYMENT_TARGET=18.0

# CORRECT PATH - matches what Xcode expects
XCODE_PATH="$HOME/cyan-iOS/Cyan/Libraries"

# Remote machine (Aria's laptop)
REMOTE="mymomsaccount@Aria-Vyas-MacBook-Pro.local"
REMOTE_DIR="~/cyan-test"

# =============================================================================
# Parse arguments
# =============================================================================
DO_CLEAN=false
BUILD_TESTS=false
RUN_UNIT=false
RUN_HOST=false
RUN_JOIN=false
JOIN_NODE_ID=""
SKIP_LIB=false
DO_DEPLOY=false
REMOTE_JOIN=false
REMOTE_JOIN_NODE_ID=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --clean)
            DO_CLEAN=true
            shift
            ;;
        --test)
            BUILD_TESTS=true
            shift
            ;;
        --unit)
            BUILD_TESTS=true
            RUN_UNIT=true
            shift
            ;;
        --host)
            BUILD_TESTS=true
            RUN_HOST=true
            shift
            ;;
        --join)
            BUILD_TESTS=true
            RUN_JOIN=true
            JOIN_NODE_ID="$2"
            shift 2
            ;;
        --skip-lib)
            SKIP_LIB=true
            BUILD_TESTS=true
            shift
            ;;
        --deploy)
            BUILD_TESTS=true
            DO_DEPLOY=true
            shift
            ;;
        --remote-join)
            BUILD_TESTS=true
            DO_DEPLOY=true
            REMOTE_JOIN=true
            REMOTE_JOIN_NODE_ID="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo ""
            echo "Usage: ./build_static_lib.sh [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --clean                  Full cargo clean before build"
            echo "  --test                   Also build test binaries"
            echo "  --unit                   Build + run unit tests (no network)"
            echo "  --host                   Build + run as snapshot host"
            echo "  --join <NODE_ID>         Build + connect to host"
            echo "  --skip-lib               Skip library, only build tests"
            echo "  --deploy                 SCP test binary to Aria's laptop"
            echo "  --remote-join <NODE_ID>  Deploy + run join on Aria's laptop"
            echo ""
            echo "Examples:"
            echo "  ./build_static_lib.sh                           # Just build library"
            echo "  ./build_static_lib.sh --test                    # Build library + tests"
            echo "  ./build_static_lib.sh --unit                    # Build + run unit tests"
            echo "  ./build_static_lib.sh --skip-lib --host         # Run as host"
            echo "  ./build_static_lib.sh --skip-lib --deploy       # Deploy binary to Aria"
            echo ""
            echo "Two-machine test (easiest):"
            echo "  Terminal 1: ./build_static_lib.sh --skip-lib --host"
            echo "  Terminal 2: ./build_static_lib.sh --skip-lib --remote-join <NODE_ID>"
            exit 1
            ;;
    esac
done

# =============================================================================
# Re-link LLVM on exit (success or failure)
# =============================================================================
trap 'echo "🔧 Re-linking Homebrew LLVM..."; brew link llvm 2>/dev/null || true' EXIT

echo "🦀 Building Cyan Backend XCFramework (Apple Clang)..."
echo "📁 Target: $XCODE_PATH"

# =============================================================================
# Unlink Homebrew LLVM to avoid ABI conflicts
# =============================================================================
echo "🔧 Unlinking Homebrew LLVM..."
brew unlink llvm 2>/dev/null || true

# =============================================================================
# CRITICAL: Use Apple's system clang, NOT Homebrew LLVM
# =============================================================================

unset CC CXX AR LIBCLANG_PATH CXXFLAGS LDFLAGS CMAKE_CXX_FLAGS

export CC=/usr/bin/clang
export CXX=/usr/bin/clang++
export AR=/usr/bin/ar
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
export MACOSX_DEPLOYMENT_TARGET=14.0
export IPHONEOS_DEPLOYMENT_TARGET=17.0

echo "🔧 Using Apple System Clang:"
echo "   CC:  $CC"
$CC --version | head -1

# =============================================================================
# Clean if requested
# =============================================================================

if [[ "$DO_CLEAN" == "true" ]]; then
    echo "🧹 Nuclear clean..."
    cargo clean
    rm -rf ./build
    echo "✓ Clean complete"
else
    echo "⭕ Skipping clean (use --clean for full rebuild)"
fi

mkdir -p ./build

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
# Build Library (unless --skip-lib)
# =============================================================================

export SDKROOT=$(xcrun --sdk macosx --show-sdk-path)
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=${SDKROOT}"

if [[ "$SKIP_LIB" == "false" ]]; then
    echo ""
    echo "🔨 Building for macOS (Apple Silicon)..."

    RUSTFLAGS="-Awarnings" cargo build --release --target "aarch64-apple-darwin" 2>&1 | tee -a build.log

    if [[ ${pipestatus[1]} -ne 0 ]]; then
        echo "❌ Build FAILED - check build.log"
        exit 1
    fi

    echo "✅ macOS build succeeded"

    # =============================================================================
    # Create XCFramework
    # =============================================================================

    echo ""
    echo "📦 Creating XCFramework..."

    cp target/aarch64-apple-darwin/release/libcyan_backend.a build/libcyan_backend_macos.a
    rm -rf build/CyanBackend.xcframework

    xcodebuild -create-xcframework \
        -library build/libcyan_backend_macos.a \
        -output build/CyanBackend.xcframework

    echo "✅ XCFramework created"

    # =============================================================================
    # Copy to Xcode project (CORRECT LOCATION)
    # =============================================================================

    echo "📋 Copying to: $XCODE_PATH"
    mkdir -p "$XCODE_PATH"
    rm -rf "$XCODE_PATH/CyanBackend.xcframework"
    cp -R build/CyanBackend.xcframework "$XCODE_PATH/"
    echo "✅ Copied to Xcode project"

    # =============================================================================
    # Verify
    # =============================================================================

    echo ""
    echo "🔍 Verifying symbols..."
    if strings "$XCODE_PATH/CyanBackend.xcframework/macos-arm64/libcyan_backend_macos.a" | grep -q "cyan_init"; then
        echo "✅ Build contains cyan_init symbol"
    else
        echo "❌ ERROR: cyan_init not found!"
        exit 1
    fi
else
    echo "⭕ Skipping library build (--skip-lib)"
fi

# =============================================================================
# Build Test Binaries (if --test, --unit, --host, or --join)
# =============================================================================

if [[ "$BUILD_TESTS" == "true" ]]; then
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🧪 Building Test Binaries..."
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    echo ""
    echo "🔨 Building snapshot_test..."
    RUSTFLAGS="-Awarnings" cargo build --release --bin snapshot_test 2>&1 | tee -a build.log
    echo "✅ snapshot_test built"

    echo ""
    echo "🔨 Building network_test..."
    RUSTFLAGS="-Awarnings" cargo build --release --bin network_test 2>&1 | tee -a build.log
    echo "✅ network_test built"

    # Copy to build dir for easy access
    cp target/release/snapshot_test build/ 2>/dev/null || cp target/aarch64-apple-darwin/release/snapshot_test build/
    cp target/release/network_test build/ 2>/dev/null || cp target/aarch64-apple-darwin/release/network_test build/

    echo ""
    echo "📁 Test binaries available at:"
    ls -lh build/snapshot_test build/network_test
fi

# =============================================================================
# Run Unit Tests (if --unit)
# =============================================================================

if [[ "$RUN_UNIT" == "true" ]]; then
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🧪 Running Unit Tests (no network required)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    echo ""
    echo "📋 snapshot_test unit:"
    ./build/snapshot_test unit

    echo ""
    echo "📋 network_test unit:"
    ./build/network_test unit
fi

# =============================================================================
# Run as Host (if --host)
# =============================================================================

if [[ "$RUN_HOST" == "true" ]]; then
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📡 Starting as SNAPSHOT HOST"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Copy the NODE ID printed below to the other machine."
    echo "On other machine run:"
    echo "  ./build_static_lib.sh --skip-lib --join <NODE_ID>"
    echo ""
    echo "Or if you SCP'd the binary:"
    echo "  ./snapshot_test join <NODE_ID>"
    echo ""

    RUST_LOG=info ./build/snapshot_test host
fi

# =============================================================================
# Run as Joiner (if --join)
# =============================================================================

if [[ "$RUN_JOIN" == "true" ]]; then
    if [[ -z "$JOIN_NODE_ID" ]]; then
        echo "❌ ERROR: --join requires a NODE_ID"
        echo "Usage: ./build_static_lib.sh --join <NODE_ID>"
        exit 1
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📥 Connecting to HOST as JOINER"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Connecting to: ${JOIN_NODE_ID:0:16}..."
    echo ""

    RUST_LOG=info ./build/snapshot_test join "$JOIN_NODE_ID"
fi

# =============================================================================
# Deploy to Aria's laptop (if --deploy or --remote-join)
# =============================================================================

if [[ "$DO_DEPLOY" == "true" ]]; then
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📤 Deploying to Aria's laptop"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "🎯 Target: $REMOTE"
    echo ""

    # Create remote directory and copy binaries
    echo "📁 Creating remote directory..."
    ssh "$REMOTE" "mkdir -p $REMOTE_DIR"

    echo "📤 Copying snapshot_test..."
    scp build/snapshot_test "$REMOTE:$REMOTE_DIR/"

    echo "📤 Copying network_test..."
    scp build/network_test "$REMOTE:$REMOTE_DIR/"

    echo "✅ Deployed to $REMOTE:$REMOTE_DIR/"

    # List what's there
    ssh "$REMOTE" "ls -lh $REMOTE_DIR/"
fi

# =============================================================================
# Run join on Aria's laptop (if --remote-join)
# =============================================================================

if [[ "$REMOTE_JOIN" == "true" ]]; then
    if [[ -z "$REMOTE_JOIN_NODE_ID" ]]; then
        echo "❌ ERROR: --remote-join requires a NODE_ID"
        echo "Usage: ./build_static_lib.sh --remote-join <NODE_ID>"
        exit 1
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🚀 Running snapshot_test join on Aria's laptop"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Connecting to: ${REMOTE_JOIN_NODE_ID:0:16}..."
    echo ""

    ssh -t "$REMOTE" "cd $REMOTE_DIR && RUST_LOG=info ./snapshot_test join $REMOTE_JOIN_NODE_ID"
fi

# =============================================================================
# Summary
# =============================================================================

if [[ "$RUN_HOST" == "false" && "$RUN_JOIN" == "false" && "$REMOTE_JOIN" == "false" ]]; then
    echo ""
    echo "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓"
    echo "┃ ✅ BUILD COMPLETE                                            ┃"
    echo "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛"

    if [[ "$SKIP_LIB" == "false" ]]; then
        echo ""
        echo "📁 XCFramework: $XCODE_PATH/CyanBackend.xcframework"
        find "$XCODE_PATH/CyanBackend.xcframework" -name "*.a" -exec ls -lh {} \;
    fi

    if [[ "$BUILD_TESTS" == "true" ]]; then
        echo ""
        echo "🧪 Test binaries:"
        ls -lh build/snapshot_test build/network_test 2>/dev/null || true
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "🧪 TEST COMMANDS:"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "  Unit tests (local, no network):"
        echo "    ./build_static_lib.sh --skip-lib --unit"
        echo ""
        echo "  ┌─────────────────────────────────────────────────────────┐"
        echo "  │ TWO-MACHINE TEST (easiest way):                        │"
        echo "  ├─────────────────────────────────────────────────────────┤"
        echo "  │ Terminal 1 (this machine - HOST):                      │"
        echo "  │   ./build_static_lib.sh --skip-lib --host              │"
        echo "  │                                                        │"
        echo "  │ Terminal 2 (same machine - runs on Aria's laptop):     │"
        echo "  │   ./build_static_lib.sh --skip-lib --remote-join <ID>  │"
        echo "  └─────────────────────────────────────────────────────────┘"
        echo ""
        echo "  Deploy only (no auto-run):"
        echo "    ./build_static_lib.sh --skip-lib --deploy"
        echo "    # Then SSH to Aria and run manually:"
        echo "    ssh $REMOTE"
        echo "    cd $REMOTE_DIR && ./snapshot_test join <NODE_ID>"
        echo ""
    fi

    if [[ "$SKIP_LIB" == "false" ]]; then
        echo ""
        echo "📌 XCODE NEXT STEPS:"
        echo "   1. In Xcode: Cmd+Shift+K (Clean Build Folder)"
        echo "   2. Cmd+B (Build) with Release scheme"
        echo "   3. ./package_cyan.sh"
    fi

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
fi