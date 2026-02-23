#!/usr/bin/env bash
# bench_compare.sh — Run the Criterion benchmark suite and optionally compare
# against the reference lzbench tool.
#
# Usage:
#   cargo build --release            # ensure the binary is current
#   ./scripts/bench_compare.sh
#
# With Silesia corpus and lzbench installed:
#   SILESIA_CORPUS_DIR=~/silesia ./scripts/bench_compare.sh
set -euo pipefail

echo "=== Running cargo bench ==="
# Try bencher output format first (machine-readable); fall back to default.
if cargo bench -- --output-format bencher 2>/dev/null | grep "^test "; then
    :
else
    cargo bench
fi

if command -v lzbench >/dev/null 2>&1 && [ -n "${SILESIA_CORPUS_DIR:-}" ]; then
    echo ""
    echo "=== Running lzbench (lz4 + lz4hc) against $SILESIA_CORPUS_DIR ==="
    lzbench -elz4,lz4hc -r "$SILESIA_CORPUS_DIR"
else
    echo ""
    echo "(lzbench not found or SILESIA_CORPUS_DIR not set — skipping C reference comparison)"
    echo "To enable: install lzbench and set SILESIA_CORPUS_DIR=<path to silesia corpus>"
fi
