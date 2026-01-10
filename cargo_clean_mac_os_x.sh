#!/bin/zsh
unset LLAMA_METAL_PATH_RESOURCES
unset GGML_METAL_PATH_RESOURCES
unset GGML_METAL_EMBED_LIBRARY
export LIBRARY_PATH=""
export RUSTFLAGS="-L /opt/homebrew/lib"
cargo "$@"
