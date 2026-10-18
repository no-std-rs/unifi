"""Exercise the provisioning flow with a real TLS fixture and isolated credentials."""
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import importlib.util
import json
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('setup_display', Path(__file__).resolve().parents[1] / 'tools/setup-display.py')
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)
SITE = '11111111-1111-4111-8111-111111111111'
DEVICE = '22222222-2222-4222-8222-222222222222'
HOST = 'console.id.ui.direct'
CLOUD = 'fixture-cloud-key'
LOCAL = 'fixture-local-key'


class SetupTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix='unifi-setup-tls-')
        root = Path(cls.directory.name)
        cls.cert = root / 'certificate.pem'
        key = root / 'key.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                        '-subj', '/CN=' + HOST, '-addext', 'subjectAltName=DNS:' + HOST,
                        '-keyout', str(key), '-out', str(cls.cert)], check=True, capture_output=True)
        cls.pin = hashlib.sha256(ssl.PEM_cert_to_DER_cert(cls.cert.read_text())).hexdigest()
        cls.context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        cls.context.load_verify_locations(cafile=str(cls.cert))
        cls.server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        cls.server_context.load_cert_chain(cls.cert, key)

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def setUp(self):
        self.calls, self.creations, self.fail_stats, self.require_mfa = [], 0, False, False
        self.legacy_mfa = False
        self.creation_response = {'code': 1, 'codeS': 'SUCCESS', 'data': {'full_api_key': LOCAL}}
        self.accepted_keys = {LOCAL}
        owner = self
        class Gateway(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                self.dispatch()

            def handle(self):
                try:
                    super().handle()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Certificate-only probes deliberately send no HTTP request.

            def do_POST(self):
                self.dispatch()

            def dispatch(self):
                body = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))) or '{}')
                owner.calls.append((self.command, self.path, dict(self.headers), body))
                status, response = 200, {}
                if self.path == '/api/auth/login':
                    if owner.legacy_mfa and (not body.get('ubic_2fa_token') or 'UBIC_2FA=fixture-old-challenge' not in self.headers.get('Cookie', '')):
                        status, response = 499, {'token': 'UBIC_2FA=fixture-old-challenge; Path=/; HttpOnly; Secure'}
                    elif owner.require_mfa and not body.get('token'):
                        status, response = 499, {'data': {'mfaCookie': 'fixture-challenge', 'authenticators': [{'id': 'push', 'type': 'push'}]}}
                    else:
                        self.send_response(200)
                        self.send_header('Set-Cookie', 'TOKEN=fixture-session; Path=/; HttpOnly; Secure')
                        self.send_header('X-Updated-CSRF-Token', 'fixture-csrf')
                        self.end_headers()
                        self.wfile.write(b'{"id":"self"}')
                        return
                elif self.path == '/api/auth/login/poll':
                    self.send_response(200)
                    self.send_header('Set-Cookie', 'TOKEN=fixture-session; Path=/; HttpOnly; Secure')
                    self.send_header('X-Updated-CSRF-Token', 'fixture-csrf')
                    self.end_headers()
                    self.wfile.write(b'{}')
                    return
                elif self.path == '/api/users/self':
                    response = {'id': 'fixture-local-user'}
                elif self.path == '/proxy/users/api/v2/user/fixture-local-user/keys':
                    if 'TOKEN=fixture-session' not in self.headers.get('Cookie', '').split('; ') or self.headers.get('X-CSRF-Token') != 'fixture-csrf':
                        status = 403
                    else:
                        owner.creations += 1
                        owner.assertEqual(body, {'name': 'UniFi ESP32 Display'})
                        response = owner.creation_response
                elif self.path.startswith('/proxy/network/integration/'):
                    if self.headers.get('X-API-Key') not in owner.accepted_keys:
                        status = 401
                    elif owner.fail_stats and self.path.endswith('/statistics/latest'):
                        status = 503
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps(response).encode())
        self.gateway = HTTPServer(('127.0.0.1', 0), Gateway)
        self.gateway.socket = self.server_context.wrap_socket(self.gateway.socket, server_side=True)
        self.thread = threading.Thread(target=self.gateway.serve_forever, daemon=True)
        self.thread.start()
        create_connection = setup.socket.create_connection
        self.network = patch.object(setup.socket, 'create_connection', side_effect=lambda address, timeout: create_connection(
            ('127.0.0.1', self.gateway.server_port), timeout=timeout))
        self.trust = patch.object(setup.ssl, 'create_default_context', return_value=self.context)
        self.network.start()
        self.trust.start()
        self.wizard = setup.Wizard(CLOUD)
        self.wizard.cloud = self.cloud
        self.wizard.discover({})
        self.selection = self.wizard.inspect({'console_id': 'fixture-console'})['selection']

    def tearDown(self):
        self.network.stop()
        self.trust.stop()
        self.gateway.shutdown()
        self.gateway.server_close()
        self.thread.join()

    def cloud(self, path):
        if path == '/v1/hosts':
            return {'data': [{'id': 'null-state', 'type': 'ucore', 'reportedState': None},
                {'id': 'fixture-console', 'type': 'console', 'reportedState': {
                'state': 'connected', 'name': 'Gateway', 'directConnectDomain': HOST,
                'mac': 'aa:bb:cc:dd:ee:ff', 'ipAddrs': ['203.0.113.1', '192.168.6.1']}}]}
        if '/devices?' in path:
            return {'data': [{'id': DEVICE, 'macAddress': 'aa-bb-cc-dd-ee-ff', 'name': 'Root gateway'}], 'totalCount': 1}
        return {'data': [{'id': SITE, 'name': 'Default'}], 'totalCount': 1}

    def payload(self, **extra):
        return {'selection': self.selection, 'site_id': SITE, 'device_id': DEVICE, **extra}

    def login(self):
        return self.wizard.login(self.payload(username='fixture-user', password='fixture-password'))

    def test_certificate_root_gateway_session_key_and_four_local_reads(self):
        self.assertEqual(self.wizard.target['pin'], self.pin)
        self.assertEqual(self.wizard.devices(self.payload())['gateway'], DEVICE)
        self.assertEqual(self.login()['state'], 'authenticated')
        result = self.wizard.provision(self.payload())['config']
        self.assertEqual(result['api_key'], LOCAL)
        self.assertEqual(result['controller_url'], 'https://' + HOST + '/proxy/network/integration')
        self.assertEqual(set(result), {'controller_url', 'api_key', 'certificate_sha256', 'site_id', 'device_id'})
        reads = [call for call in self.calls if call[1].startswith('/proxy/network/integration/')]
        self.assertEqual(len(reads), 4)
        for _, _, headers, _ in reads:
            self.assertEqual(headers['X-API-Key'], LOCAL)
            self.assertNotIn('Cookie', headers)
        self.assertNotIn(CLOUD, json.dumps(self.calls))
        self.assertEqual(self.creations, 1)
        self.assertFalse(self.wizard.authenticated)

    def test_failed_verification_retains_created_key_without_creating_again(self):
        self.login()
        self.fail_stats = True
        with self.assertRaisesRegex(setup.SetupError, 'HTTP 503'):
            self.wizard.provision(self.payload())
        self.fail_stats = False
        self.assertEqual(self.wizard.provision(self.payload())['state'], 'verified')
        self.assertEqual(self.creations, 1)

    def test_legacy_zero_creation_response_still_verifies(self):
        self.creation_response = {'code': 0, 'data': {'full_api_key': LOCAL}}
        self.login()
        self.assertEqual(self.wizard.provision(self.payload())['state'], 'verified')

    def test_error_envelope_never_uses_a_key_or_repeats_creation(self):
        self.creation_response = {'code': 0, 'codeS': 'PERMISSION_DENIED', 'data': {'full_api_key': LOCAL}}
        self.login()
        with self.assertRaises(setup.SetupError):
            self.wizard.provision(self.payload())
        self.assertIsNone(self.wizard.local_key)
        self.assertFalse(any(path.startswith('/proxy/network/integration/') for _, path, _, _ in self.calls))
        with self.assertRaises(setup.SetupError):
            self.wizard.provision(self.payload())
        self.assertEqual(self.creations, 1)

    def test_generated_key_can_be_explicitly_replaced_after_verification_failure(self):
        self.login()
        self.fail_stats = True
        with self.assertRaisesRegex(setup.SetupError, 'HTTP 503'):
            self.wizard.provision(self.payload())
        self.fail_stats = False
        replacement = 'fixture-replacement-key'
        self.accepted_keys = {replacement}
        result = self.wizard.provision(self.payload(local_key=replacement))
        self.assertEqual(result['config']['api_key'], replacement)
        self.assertEqual(self.creations, 1)

    def test_discovery_ignores_null_reported_states(self):
        self.assertEqual(list(self.wizard.hosts), ['fixture-console'])

    def test_existing_key_can_be_corrected_without_console_login(self):
        with self.assertRaisesRegex(setup.SetupError, 'HTTP 401'):
            self.wizard.provision(self.payload(local_key='wrong-key'))
        self.assertEqual(self.wizard.provision(self.payload(local_key=LOCAL))['state'], 'verified')
        self.assertEqual(self.creations, 0)

    def test_rejected_existing_key_can_recover_by_signing_in(self):
        with self.assertRaisesRegex(setup.SetupError, 'HTTP 401'):
            self.wizard.provision(self.payload(local_key='wrong-key'))
        self.login()
        self.assertEqual(self.wizard.provision(self.payload())['state'], 'verified')
        self.assertEqual(self.creations, 1)

    def test_legacy_mfa_cookie_and_code_field_complete_login(self):
        self.legacy_mfa = True
        self.assertEqual(self.login()['state'], 'mfa')
        self.assertEqual(self.wizard.login(self.payload(username='fixture-user', password='fixture-password', token='123456'))['state'], 'authenticated')
        login = [call for call in self.calls if call[1] == '/api/auth/login'][-1]
        self.assertEqual(login[3]['ubic_2fa_token'], '123456')
        self.assertNotIn('token', login[3])
        self.assertIn('UBIC_2FA=fixture-old-challenge', login[2]['Cookie'])
        self.assertEqual(self.wizard.provision(self.payload())['state'], 'verified')

    def test_pin_change_blocks_credentials_before_http(self):
        self.wizard.target['pin'] = '00' * 32
        with self.assertRaisesRegex(setup.SetupError, 'certificate changed'):
            self.login()
        self.assertEqual(self.calls, [])

    def test_untrusted_or_wrong_hostname_blocks_credentials(self):
        self.wizard.target['hostname'] = 'other.id.ui.direct'
        with self.assertRaises(ssl.SSLCertVerificationError):
            self.login()
        self.assertEqual(self.calls, [])

    def test_stale_selection_cannot_sign_into_new_console(self):
        with self.assertRaisesRegex(setup.SetupError, 'selected console changed'):
            self.wizard.login({'selection': 'old', 'username': 'u', 'password': 'p'})
        self.assertEqual(self.calls, [])

    def test_mfa_push_uses_console_challenge_then_creates_key(self):
        self.require_mfa = True
        self.assertEqual(self.login()['state'], 'mfa')
        self.assertEqual(self.wizard.mfa(self.payload(id='push'))['state'], 'sent')
        self.assertEqual(self.wizard.poll(self.payload())['state'], 'authenticated')
        self.assertEqual(self.wizard.provision(self.payload())['state'], 'verified')
        self.assertEqual(self.creations, 1)


class HelperHTTPTests(unittest.TestCase):
    def test_origin_host_and_nonce_are_required(self):
        wizard = setup.Wizard()
        wizard.discover = lambda payload: {'consoles': []}
        server = HTTPServer(('127.0.0.1', 0), BaseHTTPRequestHandler)
        origin = f'http://127.0.0.1:{server.server_port}'
        server.RequestHandlerClass = setup.handler(wizard, 'fixture-token', origin)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            import http.client
            for origin_header, nonce, host, expected in [
                (origin, 'fixture-token', urlsplit_host(origin), 200),
                ('https://example.org', 'fixture-token', urlsplit_host(origin), 403),
                (origin, 'wrong', urlsplit_host(origin), 403),
                (origin, 'fixture-token', 'evil.example', 403),
            ]:
                connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
                connection.request('POST', '/api/discover', '{}', {'Origin': origin_header, 'X-Setup-Token': nonce, 'Host': host})
                response = connection.getresponse()
                self.assertEqual(response.status, expected)
                self.assertIsNone(response.getheader('Access-Control-Allow-Origin'))
                response.read()
                connection.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


def urlsplit_host(origin):
    return origin.removeprefix('http://')


if __name__ == '__main__':
    unittest.main()
