#!/usr/bin/env bash
# Run each fuzz target for 60 seconds — long enough to catch trivial panics.
# Requires nightly Rust: rustup toolchain install nightly
set -euo pipefail
for target in block_roundtrip frame_roundtrip decompress_block_arbitrary decompress_frame_arbitrary; do
  echo "Fuzzing $target for 60s..."
  cargo +nightly fuzz run "$target" -- -max_total_time=60
done
echo "All fuzz targets completed without crashes."
