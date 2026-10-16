#!/usr/bin/env python3
"""Install this repo's fetch maintenance without replacing an existing hook set."""
from pathlib import Path
import subprocess
root=Path(__file__).resolve().parents[1]
def git(*args):return subprocess.run(['git',*args],cwd=root,text=True,capture_output=True)
existing=git('config','--get','core.hooksPath').stdout.strip()
common=Path(git('rev-parse','--path-format=absolute','--git-common-dir').stdout.strip())
hooks=root/'.githooks'
if existing and Path(existing).resolve()!=hooks:
 raise SystemExit('Existing core.hooksPath is set; integrate .githooks/reference-transaction manually.')
if (common/'hooks'/'reference-transaction').exists() and not existing:
 raise SystemExit('Existing reference-transaction hook; integrate it before installing.')
for key,value in [('fetch.prune','true'),('core.hooksPath',str(hooks))]:
 result=git('config','--local',key,value)
 if result.returncode:raise SystemExit(result.stderr)
print('Enabled fetch pruning and safe main fast-forward hook for this repository.')
