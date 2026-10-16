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
                path = self.path.split('?')[0]
                if path.endswith('/snapshot'):
                    self.reply_bytes(b'\xff\xd8fixture\x00\xff\xd9', 'image/jpeg')
                    return
                if path.endswith('/api/self/sites'):
                    value = {'meta': {'rc': 'ok'}, 'data': [
                        {'_id': SITE, 'name': 'branch42', 'desc': 'Branch Office'}]}
                elif path.endswith('/stat/health'):
                    value = {'meta': {'rc': 'ok'}, 'data': [{'subsystem': 'www', 'status': 'ok'}]}
                elif path.endswith('/v1/sites'):
                    value = {'data': [{'id': SITE, 'name': 'Branch Office',
                                       'internalReference': 'branch42'}], 'count': 1, 'totalCount': 1}
                elif '/firewall/zones' in self.path:
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
                self.reply_bytes(json.dumps(value).encode(), 'application/json')

            def reply_bytes(self, data, content_type):
                self.send_response(200)
                self.send_header('Content-Type', content_type)
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

    def run_cli(self, *args, site=SITE):
        binary = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')) / 'debug/unifi'
        return subprocess.run([str(binary), *args, '--site', site, '--controller',
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

    def test_json_saves_redact_secrets_unless_requested(self):
        for with_secrets in [False, True]:
            with self.subTest(with_secrets=with_secrets):
                destination = Path(self.directory.name) / 'wifi.json'
                flags = ['--with-secrets'] if with_secrets else []
                reply = self.run_cli('wifi', 'get', RESOURCE, '--save', str(destination), *flags)
                self.assertEqual(reply.returncode, 0, reply.stderr)
                saved = destination.read_text()
                self.assertEqual('fixture-wifi-secret' in saved, with_secrets)
                self.assertEqual('fixture-private-secret' in saved, with_secrets)

    def test_binary_saves_preserve_bytes_and_reject_demo(self):
        destination = Path(self.directory.name) / 'snapshot.jpg'
        reply = self.run_cli('protect', 'cameras', 'snapshot', RESOURCE, '--save', str(destination))
        self.assertEqual(reply.returncode, 0, reply.stderr)
        self.assertEqual(destination.read_bytes(), b'\xff\xd8fixture\x00\xff\xd9')
        destination.unlink()
        reply = self.run_cli('protect', 'cameras', 'snapshot', RESOURCE, '--save', str(destination), '--demo')
        self.assertEqual(reply.returncode, 2, reply.stderr)
        self.assertFalse(destination.exists())

    def test_direct_session_site_selectors_use_internal_names(self):
        for standalone in [False, True]:
            for selector in ['Branch Office', SITE, 'branch42']:
                with self.subTest(standalone=standalone, selector=selector):
                    self.requests.clear()
                    flags = ['--standalone'] if standalone else []
                    reply = self.run_cli('system', 'health', *flags, site=selector)
                    self.assertEqual(reply.returncode, 0, reply.stderr)
                    self.assertTrue(self.requests[-1][1].endswith('/api/s/branch42/stat/health'))
        self.requests.clear()
        reply = self.run_cli('system', 'health', site='default')
        self.assertEqual(reply.returncode, 0, reply.stderr)
        self.assertEqual(len(self.requests), 1)

    def test_cloud_switch_saves_console_and_site_together(self):
        path = Path(self.env['UNIFI_CONFIG'])
        path.write_text(json.dumps({'activeProfile': 'cloud', 'profiles': {'cloud': {
            'authMode': 'cloud', 'console': 'console-a', 'site': 'old-site', 'unrelated': 'keep'}}}))
        reply = self.run_cli('cloud', 'switch', 'Branch Office', '--console', 'console-b')
        self.assertEqual(reply.returncode, 0, reply.stderr)
        saved = json.loads(path.read_text())['profiles']['cloud']
        self.assertEqual(saved['console'], 'console-b')
        self.assertEqual(saved['site'], 'branch42')
        self.assertEqual(saved['unrelated'], 'keep')
