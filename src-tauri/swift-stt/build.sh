#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/../resources"
mkdir -p "$OUTPUT_DIR"

swiftc \
    -target x86_64-apple-macosx10.15 \
    -framework Speech \
    -framework AVFoundation \
    -framework Foundation \
    -O \
    -o "$OUTPUT_DIR/stt-helper" \
    "$SCRIPT_DIR/main.swift"

echo "Built: $OUTPUT_DIR/stt-helper"
