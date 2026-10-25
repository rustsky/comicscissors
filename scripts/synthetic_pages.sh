#!/bin/sh
# Write the synthetic regression pages (src/synth.rs) as PNGs, each with a copy showing the
# panels detected on it, to look at what a test draws: scripts/synthetic_pages.sh [out-dir]
set -e
cd "$(dirname "$0")/.."
SYNTH_OUT="$(cd "$(dirname "${1:-target/synthetic}")" 2>/dev/null && pwd)/$(basename "${1:-target/synthetic}")" \
  cargo test --release write_synthetic_pages -- --ignored --nocapture
