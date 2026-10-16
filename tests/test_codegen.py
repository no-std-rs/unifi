"""Generated bindings must be reproducible across Python hash seeds."""
import os
import json
import jsonschema
from pathlib import Path
import subprocess
import unittest
from tools.mock_server import normalized

ROOT = Path(__file__).resolve().parents[1]

class CodegenTests(unittest.TestCase):
    def test_network_prose_nullability_keeps_non_null_bounds(self):
        spec=normalized(json.loads((ROOT/'schemas/network.json').read_text()))
        check=jsonschema.Draft202012Validator({**spec,'$ref':'#/components/schemas/IntegrationWifiHandoffSuggestionsConfigurationDto'})
        self.assertTrue(check.is_valid({'band5GHzRssiThreshold':None,'band6GHzRssiThreshold':-80}))
        self.assertFalse(check.is_valid({'band5GHzRssiThreshold':False}))
        self.assertFalse(check.is_valid({'band5GHzRssiThreshold':-59}))

    def test_official_request_schema_corpus_is_current(self):
        result = subprocess.run(['python3', 'tools/generate-request-fixtures.py', '--check'], cwd=ROOT,
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_hash_independent_generation(self):
        for seed in ['0', '42']:
            result = subprocess.run(['python3', 'tools/generate.py', '--check'], cwd=ROOT,
                env={**os.environ, 'PYTHONHASHSEED': seed}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
