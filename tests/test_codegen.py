"""Generated bindings must be reproducible across Python hash seeds."""
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]

class CodegenTests(unittest.TestCase):
    def test_official_request_schema_corpus_is_current(self):
        result = subprocess.run(['python3', 'tools/generate-request-fixtures.py', '--check'], cwd=ROOT,
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_hash_independent_generation(self):
        for seed in ['0', '42']:
            result = subprocess.run(['python3', 'tools/generate.py', '--check'], cwd=ROOT,
                env={**os.environ, 'PYTHONHASHSEED': seed}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
