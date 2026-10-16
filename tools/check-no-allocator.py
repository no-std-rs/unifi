#!/usr/bin/env python3
"""Fail if the CLI references a Rust or C allocator."""
import re
import subprocess
import sys

symbols = subprocess.run(["nm", "-u", sys.argv[1]], capture_output=True, text=True, check=True).stdout
for line in symbols.splitlines():
    symbol = line.split()[-1].split("@", 1)[0]
    if re.search(r"__(?:rust|rdl|rg)_alloc", symbol) or symbol in {"malloc", "calloc", "realloc", "aligned_alloc"}:
        raise SystemExit("Unexpected allocator reference: " + symbol)
print("No allocator references")
