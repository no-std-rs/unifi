#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Adapted from no-std-rs/ubus-zero; see the gadget LICENSE and README attribution.
"""Build a static site from the verified firmware directory."""
import json
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[1]
firmware = Path(sys.argv[1])
site = Path(sys.argv[2])
site.mkdir(parents=True, exist_ok=True)
for name in ['index.html', 'style.css', 'app.js', 'config.js', 'cloud.js', 'cloud-setup.js', 'saved-settings.js', 'network-profiles.js', 'serial-client.js', 'device-setup.js', 'LICENSE']:
    shutil.copyfile(ROOT / 'web' / name, site / name)
shutil.copyfile(ROOT / 'web/dist/installer.js', site / 'installer.js')
# Remove files from older site assemblies; the public installer has no localhost helper.
for obsolete in ['setup-display.py', 'setup-wizard.js']:
    (site / obsolete).unlink(missing_ok=True)
for name in ['unifi-display.bin', 'latest.json', 'manifest.json', 'SHA256SUMS']:
    shutil.copyfile(firmware / name, site / name)
(site / '.nojekyll').touch()
metadata = json.loads((site / 'latest.json').read_text())
print(f'Assembled flasher for {metadata["revision"]} at {site}')
