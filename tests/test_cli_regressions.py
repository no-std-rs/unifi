"""CLI wire and output regressions against an isolated local HTTP fixture."""
import json
import os
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]
SITE = '11111111-1111-4111-8111-111111111111'
RESOURCE = '22222222-2222-4222-8222-222222222222'
NETWORK = '33333333-3333-4333-8333-333333333333'
ZONE = '44444444-4444-4444-8444-444444444444'
WIFI = {'id': RESOURCE, 'name': 'Guest', 'type': 'STANDARD', 'enabled': True,
        'network': {'type': 'NATIVE'},
        'securityConfiguration': {'type': 'WPA2_PERSONAL', 'passphrase': 'fixture-wifi-secret'},
        'privatePresharedKeys': [{'passphrase': 'fixture-private-secret'}]}


class CliRegressionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('UNIFI_')}
        self.env.update(UNIFI_CONFIG=str(Path(self.directory.name) / 'config.json'),
                        UNIFI_API_KEY='fixture-only')
        self.requests = []
        requests = self.requests

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                requests.append(('GET', self.path, None))
                if '/firewall/zones' in self.path:
                    value = {'data': [{'id': ZONE, 'name': 'Trusted'}], 'count': 1, 'totalCount': 1}
                elif self.path.split('?')[0].endswith('/networks'):
                    value = {'data': [{'id': NETWORK, 'name': 'Lab'}], 'count': 1, 'totalCount': 1}
                elif '/networks/' in self.path:
                    value = {'id': RESOURCE, 'name': 'Guest', 'enabled': True,
                             'management': 'UNMANAGED', 'vlanId': 1}
                else:
                    value = WIFI
                self.reply(value)

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))))
                requests.append((self.command, self.path, body))
                self.reply({'id': RESOURCE, **body})

            do_PUT = do_POST

            def reply(self, value):
                data = json.dumps(value).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.directory.cleanup()

    def run_cli(self, *args):
        binary = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')) / 'debug/unifi'
        return subprocess.run([str(binary), *args, '--site', SITE, '--controller',
                               f'http://127.0.0.1:{self.server.server_port}'],
                              cwd=self.directory.name, env=self.env,
                              capture_output=True, text=True, timeout=10)

    def test_wifi_network_names_resolve_for_create_and_network_only_update(self):
        for action in ['create', 'update']:
            with self.subTest(action=action):
                args = ['wifi', action]
                args += ['--name', 'Guest', '--security', 'open'] if action == 'create' else [RESOURCE]
                reply = self.run_cli(*args, '--network', 'Lab', '--allow-write', '--output', 'json')
                self.assertEqual(reply.returncode, 0, reply.stderr)
                body = [r[2] for r in self.requests if r[0] in ('POST', 'PUT')][-1]
                self.assertEqual(body['network']['networkId'], NETWORK)

    def test_network_zone_names_resolve_before_serializing(self):
        for action in ['create', 'update']:
            with self.subTest(action=action):
                args = ['networks', action]
                args += ['--name', 'Guest', '--management', 'unmanaged'] if action == 'create' else [RESOURCE]
                reply = self.run_cli(*args, '--zone', 'Trusted', '--allow-write', '--output', 'json')
                self.assertEqual(reply.returncode, 0, reply.stderr)
                body = [r[2] for r in self.requests if r[0] in ('POST', 'PUT')][-1]
                self.assertEqual(body['zoneId'], ZONE)

    def test_uuid_selectors_skip_resource_lookups(self):
        for command in [
            ['wifi', 'create', '--name', 'Guest', '--security', 'open', '--network', NETWORK],
            ['networks', 'create', '--name', 'Guest', '--management', 'unmanaged', '--zone', ZONE],
        ]:
            with self.subTest(command=command):
                self.requests.clear()
                reply = self.run_cli(*command, '--allow-write', '--output', 'json')
                self.assertEqual(reply.returncode, 0, reply.stderr)
                self.assertEqual([r[0] for r in self.requests], ['POST'])

    def test_wifi_secrets_require_explicit_output_opt_in(self):
        for output in ['json', 'table']:
            with self.subTest(output=output):
                reply = self.run_cli('wifi', 'get', RESOURCE, '--output', output)
                self.assertEqual(reply.returncode, 0, reply.stderr)
                self.assertNotIn('fixture-wifi-secret', reply.stdout)
                self.assertNotIn('fixture-private-secret', reply.stdout)
        reply = self.run_cli('wifi', 'get', RESOURCE, '--output', 'json', '--with-secrets')
        self.assertEqual(reply.returncode, 0, reply.stderr)
        self.assertIn('fixture-wifi-secret', reply.stdout)
        self.assertIn('fixture-private-secret', reply.stdout)

    def test_oversized_frequency_list_returns_a_capacity_error(self):
        reply = self.run_cli('wifi', 'create', '--name', 'Guest', '--security', 'open',
                             '--frequencies', ','.join(['2.4'] * 130), '--allow-write')
        self.assertEqual(reply.returncode, 2, reply.stderr)
        self.assertIn('capacity', reply.stderr)
        self.assertEqual(self.requests, [])
