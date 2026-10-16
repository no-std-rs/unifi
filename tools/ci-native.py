#!/usr/bin/env python3
"""Run native checks for independently buildable entries in the PR stack."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def run(*args):
    print('+ ' + ' '.join(map(str, args)), flush=True)
    subprocess.run(args, cwd=ROOT, check=True)

def main():
    for schema in sorted((ROOT / 'schemas').glob('*.json')):
        document = json.loads(schema.read_text())
        if not isinstance(document, dict) or not isinstance(document.get('paths'), dict):
            raise SystemExit(f'{schema}: expected an OpenAPI object with paths')
    if (ROOT / 'Cargo.toml').exists():
        run('cargo', 'fmt', '--all', '--check')
        for generator in ['generate.py', 'generate-cli.py']:
            if (ROOT / 'tools' / generator).exists():
                run(sys.executable, 'tools/' + generator, '--check')
        run('cargo', 'test', '--locked')
        run('cargo', 'test', '--locked', '--all-features')
        for features in ['std', 'std_async', 'async_std', 'embassy']:
            run('cargo', 'test', '--locked', '--no-default-features', '--features', features, '--test', 'runtime_clients')
        if any((ROOT / 'examples').glob('*.rs')):
            run('cargo', 'test', '--locked', '--examples')
        run('cargo', 'clippy', '--locked', '-p', 'unifi', '--all-targets', '--all-features', '--', '-D', 'warnings')
        if (ROOT / 'cli/Cargo.toml').exists():
            run('cargo', 'clippy', '--locked', '-p', 'unifi-cli', '--bin', 'unifi', '--', '-D', 'warnings')
            run('cargo', 'build', '--locked', '-p', 'unifi-cli', '--timings')
            import os
            target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
            run(sys.executable, 'tools/check-no-allocator.py', target / 'debug/unifi')
    else:
        print('Bootstrap entry: no Rust workspace yet.', flush=True)
    requirements = ROOT / 'tools/requirements-test.txt'
    with tempfile.TemporaryDirectory(prefix='unifi-ci-') as directory:
        python = sys.executable
        if requirements.exists():
            run(sys.executable, '-m', 'venv', directory)
            python = str(Path(directory) / 'bin/python')
            run(python, '-m', 'pip', 'install', '--disable-pip-version-check', '-r', str(requirements))
        if (ROOT / 'tests').exists():
            run(python, '-m', 'unittest', 'discover', '-s', 'tests', '-p', 'test_*.py', '-v')

if __name__ == '__main__':
    main()
