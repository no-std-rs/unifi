"""CLI behavior that needs neither schemas nor a controller server."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class CliSmokeTests(unittest.TestCase):
    def test_help_completion_and_write_guard_are_offline(self):
        with tempfile.TemporaryDirectory() as directory:
            env={key:value for key,value in os.environ.items() if not key.startswith('UNIFI_')}
            env.update(UNIFI_CONFIG=str(Path(directory)/'config.json'),UNIFI_API_KEY='fixture-only')
            binary=Path(os.environ.get('CARGO_TARGET_DIR',ROOT/'target'))/'debug/unifi'
            for args in [('--help',),('config','cloud-setup','--help'),('completions','bash')]:
                reply=subprocess.run([str(binary),*args],cwd=directory,env=env,capture_output=True,text=True,timeout=5)
                self.assertEqual(reply.returncode,0,reply.stderr);self.assertTrue(reply.stdout)
            reply=subprocess.run([str(binary),'wifi','create','--site','fixture','--controller','http://127.0.0.1:1','--data','{}'],cwd=directory,env=env,capture_output=True,text=True,timeout=5)
            self.assertEqual(reply.returncode,2,reply.stderr)
            self.assertIn('require --allow-write',reply.stderr)
