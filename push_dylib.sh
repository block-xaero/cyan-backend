#!/bin/zsh
# push_dylib.sh
# Quick dylib rebuild and push to other Mac for fast iteration
# Usage: ./push_dylib.sh [user@host]
#
# Examples:
#   ./push_dylib.sh                    # Just rebuild locally
#   ./push_dylib.sh mom@192.168.1.50   # Rebuild and push to other Mac
#   ./push_dylib.sh mom@othermac.local # Using .local hostname

set -e

REMOTE_HOST="$1"
CYAN_BACKEND=~/cyan-backend
LOCAL_APP=~/Desktop/CyanBuild/Cyan.app
REMOTE_APP="~/Applications/Cyan.app"
DYLIB_NAME="Cyan.debug.dylib"

# =============================================================================
# Use Apple's system clang (required for llama-cpp)
# =============================================================================
echo "🔧 Setting up Apple Clang environment..."

# Unlink Homebrew LLVM temporarily
brew unlink llvm 2>/dev/null || true

# Unset any Homebrew LLVM environment variables
unset CC CXX AR LIBCLANG_PATH CXXFLAGS LDFLAGS CMAKE_CXX_FLAGS

# Force Apple's system clang
export CC=/usr/bin/clang
export CXX=/usr/bin/clang++
export AR=/usr/bin/ar
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
export MACOSX_DEPLOYMENT_TARGET=14.0
export SDKROOT=$(xcrun --sdk macosx --show-sdk-path)

# Re-link LLVM on exit
trap 'brew link llvm 2>/dev/null || true' EXIT

echo "🔨 Building Rust (incremental)..."
cd "$CYAN_BACKEND"
cargo build --release --target aarch64-apple-darwin

# Find the built dylib
BUILT_DYLIB="$CYAN_BACKEND/target/aarch64-apple-darwin/release/libcyan_backend.dylib"

if [[ ! -f "$BUILT_DYLIB" ]]; then
    echo "❌ Dylib not found at: $BUILT_DYLIB"
    exit 1
fi

echo "✅ Built: $BUILT_DYLIB"

# Update local packaged app if it exists
if [[ -d "$LOCAL_APP" ]]; then
    echo "📦 Updating local app..."
    # Put dylib in MacOS folder (where @rpath resolves)
    cp "$BUILT_DYLIB" "$LOCAL_APP/Contents/MacOS/$DYLIB_NAME"
    # Remove from Frameworks if exists (avoid duplicates)
    rm -f "$LOCAL_APP/Contents/Frameworks/$DYLIB_NAME" 2>/dev/null || true
    codesign --force --sign - "$LOCAL_APP/Contents/MacOS/$DYLIB_NAME" 2>/dev/null || true
    codesign --force --deep --sign - "$LOCAL_APP" 2>/dev/null || true

    # Re-zip
    cd ~/Desktop/CyanBuild
    rm -f Cyan.app.zip
    zip -rq Cyan.app.zip Cyan.app
    echo "✅ Local app updated: $LOCAL_APP"
    echo "📦 Zip ready: ~/Desktop/CyanBuild/Cyan.app.zip"
else
    echo "⚠️  Local app not found at $LOCAL_APP"
    echo "   Run ./package_cyan.sh first to create it"
fi

# Push to remote if specified
if [[ -n "$REMOTE_HOST" ]]; then
    echo ""
    echo "🚀 Pushing to $REMOTE_HOST..."

    # Push dylib to MacOS folder (where @rpath resolves)
    scp "$BUILT_DYLIB" "$REMOTE_HOST:$REMOTE_APP/Contents/MacOS/$DYLIB_NAME"

    # Remote: remove duplicate from Frameworks, codesign, and restart
    ssh "$REMOTE_HOST" << 'EOF'
        echo "🧹 Removing duplicate dylib from Frameworks..."
        rm -f ~/Applications/Cyan.app/Contents/Frameworks/Cyan.debug.dylib 2>/dev/null || true

        echo "🔏 Codesigning..."
        codesign --force --sign - ~/Applications/Cyan.app/Contents/MacOS/Cyan.debug.dylib 2>/dev/null || true
        codesign --force --deep --sign - ~/Applications/Cyan.app 2>/dev/null || true

        echo "🔄 Killing existing Cyan..."
        pkill -x Cyan 2>/dev/null || true
        sleep 1

        echo "✅ Done! Dylib pushed and signed."
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "🚀 Starting Cyan with logging..."
        echo "   Press Ctrl+C to stop"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
EOF

    # Run the app via interactive SSH
    ssh -t "$REMOTE_HOST" "RUST_LOG=error ~/Applications/Cyan.app/Contents/MacOS/Cyan 2>&1"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ Session ended"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
else
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ Local build complete"
    echo ""
    echo "To push to other Mac:"
    echo "  ./push_dylib.sh user@hostname"
    echo ""
    echo "Or AirDrop: ~/Desktop/CyanBuild/Cyan.app.zip"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
fi