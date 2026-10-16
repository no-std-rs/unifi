#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
# Adapted from no-std-rs/ubus-zero; this firmware has no compile-time credentials.
set -eu
repo_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=${1:-"$repo_dir/dist/firmware"}
mkdir -p "$output_dir"
output_dir=$(CDPATH='' cd -- "$output_dir" && pwd)
example_dir="$repo_dir/examples/esp32c3-unifi-display"
cd "$example_dir"
firmware_target_dir=${CARGO_TARGET_DIR:-"$example_dir/target"}
case "$firmware_target_dir" in /*) ;; *) firmware_target_dir="$example_dir/$firmware_target_dir" ;; esac
export CARGO_TARGET_DIR="$firmware_target_dir"
cargo build --locked --release --features firmware
espflash save-image --skip-update-check --chip esp32c3 --merge --skip-padding \
    --flash-mode dio --flash-freq 80mhz --flash-size 4mb \
    --partition-table partitions.csv \
    "$firmware_target_dir/riscv32imc-unknown-none-elf/release/unifi-display" "$output_dir/unifi-display.bin"
cd "$repo_dir"
python3 tools/package-dashboard.py "$output_dir"
