#!/bin/sh
set -eu

source_root="$1"
output="$2"
target_dir="${CARGO_TARGET_DIR:-$source_root/target}"

cargo build --manifest-path "$source_root/Cargo.toml" --release --locked
cp "$target_dir/release/flashcards" "$output"
