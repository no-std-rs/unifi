#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Local UniFi display setup wizard. Python 3.10+, standard library only."""
import argparse
import hashlib
import hmac
import http.client
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, HTTPServer
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import socket
import ssl
import sys
from urllib.parse import quote, urlsplit
import webbrowser


class SetupError(Exception):
    pass


def local_ipv4(value):
    try:
        address = ipaddress.IPv4Address(value)
    except (ValueError, TypeError):
        return False
    return any(address in ipaddress.IPv4Network(network) for network in (
        '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '169.254.0.0/16', '100.64.0.0/10'))


def api_key(value):
    if not isinstance(value, str) or not re.fullmatch(r'[!-~]{1,512}', value):
        raise SetupError('Enter an API key without spaces (up to 512 ASCII characters).')
    return value


def read_json(response):
    raw = response.read(4 * 1024 * 1024 + 1)
    if len(raw) > 4 * 1024 * 1024:
        raise SetupError('UniFi returned an oversized response.')
    if not raw and response.status in (202, 204):
        return {}
    try:
        return json.loads(raw)
    except (ValueError, UnicodeError):
        raise SetupError('UniFi returned an unexpected response.') from None


def success(status, operation):
    if not 200 <= status < 300:
        raise SetupError(f'{operation} failed (HTTP {status}).')


class LocalHTTPS(http.client.HTTPSConnection):
    """Connect to the cloud-reported LAN IP, verifying the cloud-reported TLS name."""
    def __init__(self, address, hostname, pin=None):
        super().__init__(hostname, timeout=20, context=ssl.create_default_context())
        self.address, self.pin = address, pin
        self.fingerprint = None

    def connect(self):
        raw = socket.create_connection((self.address, 443), timeout=self.timeout)
        try:
            self.sock = self._context.wrap_socket(raw, server_hostname=self.host)
            self.fingerprint = hashlib.sha256(self.sock.getpeercert(binary_form=True)).hexdigest()
            if self.pin and not hmac.compare_digest(self.fingerprint, self.pin):
                raise SetupError('The console certificate changed during setup. Discover it again.')
        except Exception:
            raw.close()
            if self.sock:
                self.sock.close()
            raise


class Wizard:
    def __init__(self, saved_cloud_key=''):
        self.saved_cloud_key = saved_cloud_key
        self.cloud_key = ''
        self.hosts = {}
        self.target = None
        self.cookies = SimpleCookie()
        self.csrf = ''
        self.challenge = None
        self.authenticated = False
        self.local_key = None
        self.creation_attempted = False
        self.created_key = False
        self.selection = ''

    def cloud(self, path):
        connection = http.client.HTTPSConnection('api.ui.com', timeout=20)
        try:
            connection.request('GET', path, headers={'X-API-Key': self.cloud_key, 'Accept': 'application/json'})
            response = connection.getresponse()
            success(response.status, 'Cloud discovery')
            return read_json(response)
        finally:
            connection.close()

    def pages(self, get, path):
        rows, offset = [], 0
        for _ in range(1000):
            page = get(f'{path}?offset={offset}&limit=100')
            data, total = page.get('data'), page.get('totalCount')
            if not isinstance(data, list) or type(total) is not int or total < 0:
                raise SetupError('Unexpected UniFi pagination response.')
            rows.extend(data)
            offset += len(data)
            if offset >= total:
                return rows
            if not data:
                break
        raise SetupError('UniFi pagination stalled. Discover again.')

    def discover(self, payload):
        self.cloud_key = api_key(payload.get('cloud_key') or self.saved_cloud_key)
        hosts, next_token, seen = [], '', set()
        for _ in range(1000):
            page = self.cloud('/v1/hosts' + ('?nextToken=' + quote(next_token, safe='') if next_token else ''))
            if not isinstance(page.get('data'), list):
                raise SetupError('Unexpected Site Manager response.')
            hosts.extend(page['data'])
            next_token = page.get('nextToken')
            if not next_token:
                break
            if not isinstance(next_token, str) or next_token in seen:
                raise SetupError('Cloud pagination stalled.')
            seen.add(next_token)
        else:
            raise SetupError('Cloud pagination exceeded its limit.')
        self.hosts = {h['id']: h for h in hosts if h.get('type') in ('console', 'ucore')
                      and (h.get('reportedState') or {}).get('state') == 'connected'}
        return {'consoles': [{'id': h['id'], 'name': h['reportedState'].get('name') or h['id'],
                             'addresses': list(dict.fromkeys(a for a in h['reportedState'].get('ipAddrs', []) if local_ipv4(a)))}
                            for h in self.hosts.values()]}

    def inspect(self, payload):
        host = self.hosts.get(payload.get('console_id'))
        if not host:
            raise SetupError('Discover and choose an online console first.')
        state = host['reportedState']
        hostname = state.get('directConnectDomain', '')
        # Only use the console-specific TLS identity returned by authenticated cloud discovery.
        if not isinstance(hostname, str) or not re.fullmatch(r'[a-zA-Z0-9-]+(?:\.[a-zA-Z0-9-]+)*\.ui\.direct', hostname):
            raise SetupError('This console does not report a trusted direct-connect hostname. Use manual setup.')
        addresses = list(dict.fromkeys(a for a in state.get('ipAddrs', []) if local_ipv4(a)))
        address = payload.get('address') or (addresses[0] if len(addresses) == 1 else '')
        if address not in addresses:
            raise SetupError('Choose one of this console’s reported LAN addresses.')
        if not re.fullmatch(r'[a-zA-Z0-9:_-]{1,128}', host['id']):
            raise SetupError('Invalid console identifier.')
        connection = LocalHTTPS(address, hostname)
        try:
            connection.connect()
            pin = connection.fingerprint
        finally:
            connection.close()
        base = '/v1/connector/consoles/' + quote(host['id'], safe='') + '/proxy/network/integration'
        sites = self.pages(self.cloud, base + '/v1/sites')
        # Commit a new target only after certificate verification and cloud discovery succeed.
        self.target = {'host': host, 'hostname': hostname, 'address': address, 'pin': pin, 'base': base, 'sites': sites}
        self.selection = secrets.token_hex(16)
        self.cookies, self.csrf, self.challenge = SimpleCookie(), '', None
        self.authenticated, self.local_key, self.creation_attempted = False, None, False
        self.created_key = False
        return {'selection': self.selection, 'hostname': hostname, 'address': address,
                'certificate_sha256': pin, 'sites': [{'id': s['id'], 'name': s.get('name') or s['id']} for s in sites]}

    def selected(self, payload):
        if not self.target or not hmac.compare_digest(str(payload.get('selection', '')), self.selection):
            raise SetupError('The selected console changed. Start discovery again.')

    def site(self, value):
        if not self.target or not any(s['id'] == value for s in self.target['sites']):
            raise SetupError('Choose a site from this console.')
        if not isinstance(value, str) or not re.fullmatch(r'[0-9a-fA-F-]{36}', value):
            raise SetupError('Invalid site identifier.')
        return '/v1/sites/' + value

    def devices(self, payload):
        self.selected(payload)
        path = self.site(payload.get('site_id'))
        rows = self.pages(self.cloud, self.target['base'] + path + '/devices')
        normalize = lambda mac: re.sub(r'[:-]', '', str(mac or '')).lower()
        mac = normalize(self.target['host']['reportedState'].get('mac'))
        matches = [d['id'] for d in rows if mac and normalize(d.get('macAddress')) == mac]
        return {'devices': [{'id': d['id'], 'name': d.get('name') or d.get('model') or d['id']} for d in rows],
                'gateway': matches[0] if len(matches) == 1 else ''}

    def local(self, method, path, payload=None, use_key=False):
        target = self.target
        connection = LocalHTTPS(target['address'], target['hostname'], target['pin'])
        headers = {'Accept': 'application/json', 'Content-Type': 'application/json'}
        if use_key:
            headers['X-API-Key'] = api_key(self.local_key)
        else:
            if self.cookies:
                headers['Cookie'] = '; '.join(f'{name}={morsel.value}' for name, morsel in self.cookies.items())
            if self.csrf:
                headers['X-CSRF-Token'] = self.csrf
            headers['Origin'] = 'https://' + target['hostname']
        try:
            connection.request(method, path, body=json.dumps(payload) if payload is not None else None, headers=headers)
            response = connection.getresponse()
            for name, value in response.getheaders():
                if name.lower() == 'set-cookie':
                    self.cookies.load(value)
                if name.lower() in ('x-csrf-token', 'x-updated-csrf-token'):
                    self.csrf = value
            return response.status, read_json(response)
        finally:
            connection.close()

    def login(self, payload):
        self.selected(payload)
        self.authenticated = False
        if not self.created_key:
            self.local_key = None
        username, password, token = (payload.get(k, '') for k in ('username', 'password', 'token'))
        if not isinstance(username, str) or not username or not isinstance(password, str) or not password:
            raise SetupError('Enter your console username and password.')
        body = {'username': username, 'password': password, 'rememberMe': False}
        legacy_mfa = self.challenge and self.challenge.get('legacy')
        body['ubic_2fa_token' if legacy_mfa else 'token'] = token
        status, data = self.local('POST', '/api/auth/login', body)
        if status == 499:
            if isinstance(data.get('token'), str):
                # Older UniFi OS returns the challenge cookie in JSON rather than Set-Cookie.
                self.cookies.load(data['token'])
                self.challenge = {'legacy': True}
                methods = [{'id': 'totp', 'type': 'totp'}]
            else:
                self.challenge = data.get('data', {})
                methods = [{'id': a['id'], 'type': a['type']} for a in self.challenge.get('authenticators', [])]
            return {'state': 'mfa', 'methods': methods}
        success(status, 'Console sign-in')
        self.authenticated = True
        self.challenge = None
        return {'state': 'authenticated'}

    def mfa(self, payload):
        self.selected(payload)
        if not self.challenge:
            raise SetupError('Sign in before requesting two-factor approval.')
        method = next((a for a in self.challenge.get('authenticators', []) if a['id'] == payload.get('id')), None)
        if not method or method['type'] not in ('push', 'email', 'sms'):
            raise SetupError('Choose push, email, or SMS to send a code or approval request.')
        status, data = self.local('POST', '/api/sso/login/mfa/send', {
            'id': method['id'], 'method': method['type'], 'mfaCookie': self.challenge['mfaCookie']})
        success(status, 'Two-factor request')
        if data.get('mfaCookie'):
            self.challenge['mfaCookie'] = data['mfaCookie']
        return {'state': 'sent'}

    def poll(self, payload):
        self.selected(payload)
        if not self.challenge:
            raise SetupError('Sign in before checking two-factor approval.')
        status, _ = self.local('POST', '/api/auth/login/poll', {'mfaCookie': self.challenge['mfaCookie'], 'rememberMe': False})
        if status == 202:
            return {'state': 'waiting'}
        success(status, 'Two-factor approval')
        self.authenticated, self.challenge = True, None
        return {'state': 'authenticated'}

    def provision(self, payload):
        self.selected(payload)
        path = self.site(payload.get('site_id'))
        device = payload.get('device_id')
        if not isinstance(device, str) or not re.fullmatch(r'[0-9a-fA-F-]{36}', device):
            raise SetupError('Choose a device to display.')
        if payload.get('local_key'):
            self.local_key = api_key(payload['local_key'])
            self.created_key = False
        if not self.local_key:
            if not self.authenticated:
                raise SetupError('Sign in to your console before creating the local display key.')
            if self.creation_attempted:
                raise SetupError('Key creation was already attempted. Check Console → Integrations before restarting setup.')
            # Match the console UI's user-specific route and refresh the CSRF token.
            status, profile = self.local('GET', '/api/users/self')
            success(status, 'Console session check')
            user_id = profile.get('id') if isinstance(profile, dict) else None
            if not isinstance(user_id, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', user_id):
                raise SetupError('Console did not return a valid signed-in user ID. Retry local setup.')
            self.creation_attempted = True
            status, data = self.local('POST', f'/proxy/users/api/v2/user/{user_id}/keys', {'name': 'UniFi ESP32 Display'})
            success(status, 'Local key creation')
            if data.get('code', 0) != 0 or not data.get('data', {}).get('full_api_key'):
                raise SetupError('The console did not return a new key. Check Console → Integrations before retrying.')
            self.local_key = api_key(data['data']['full_api_key'])
            self.created_key = True
        # Match the display’s four sample reads. Keep a newly created key in memory on failure,
        # so Retry verification never creates another key.
        for suffix in [f'/devices/{device}', f'/devices/{device}/statistics/latest', '/devices?limit=1', '/clients?limit=1']:
            status, _ = self.local('GET', '/proxy/network/integration' + path + suffix, use_key=True)
            success(status, 'Local display verification')
        result = {'controller_url': 'https://' + self.target['hostname'] + '/proxy/network/integration',
                  'api_key': self.local_key, 'certificate_sha256': self.target['pin'],
                  'site_id': payload['site_id'], 'device_id': device}
        # End the temporary login session; the device only needs the integration key.
        if self.authenticated:
            try:
                self.local('POST', '/api/auth/logout', {})
            except (SetupError, OSError, http.client.HTTPException):
                pass
            self.authenticated = False
            self.cookies, self.csrf = SimpleCookie(), ''
        return {'state': 'verified', 'config': result}


HTML = r'''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="setup-token" content="TOKEN_PLACEHOLDER"><title>UniFi display · Guided setup</title>
<style>:root{color-scheme:dark;font:15px system-ui;background:#090e13;color:#eef3f5}*{box-sizing:border-box}body{max-width:620px;margin:auto;padding:32px 22px}h1{font-size:30px;letter-spacing:-1px}h2{font-size:19px}p{line-height:1.7;color:#a9bac6}section{padding:20px;border:1px solid #293845;border-radius:12px;margin:18px 0}label{display:block;margin:14px 0 6px}input,select,button{width:100%;padding:12px;font:inherit;border-radius:6px;border:1px solid #344553;background:#111922;color:inherit}button{background:#6be6bd;color:#08251b;margin-top:18px;font-weight:650;cursor:pointer}button:disabled{opacity:.5;cursor:wait}code{overflow-wrap:anywhere;font-size:12px}small{display:block;line-height:1.6;color:#a9bac6;margin-top:8px}.error{color:#ffb5a6}[hidden]{display:none!important}</style>
<h1>Let’s connect your display.</h1><p>Discover your gateway, sign in once, then return to USB installation. The display will connect directly over your LAN.</p>
<section id="discovery"><h2>1 · Find your console</h2><form id="discover-form"><label for="cloud-key">Site Manager cloud API key</label><input id="cloud-key" type="password" autocomplete="off" maxlength="512"><small>Used only for discovery at api.ui.com. A saved UNIFI_API_KEY can be used when this field is empty.</small><button>Discover my consoles</button></form></section>
<section id="choose" hidden><h2>2 · Verify your gateway</h2><form id="inspect-form"><label for="console">Console</label><select id="console"></select><label for="address">Gateway LAN address</label><select id="address"></select><button>Verify gateway certificate</button></form></section>
<section id="account" hidden><h2>3 · Create local access</h2><p id="identity"></p><small>Your username, password and two-factor codes go through this local helper to the verified gateway. They are never saved or sent to the installer.</small><label for="site">Site</label><select id="site"></select><label for="device">Device to display</label><select id="device"></select>
<form id="login-form"><label for="username">Console username or email</label><input id="username" autocomplete="username" required><label for="password">Console password</label><input id="password" type="password" autocomplete="current-password" required><label for="otp">Two-factor or backup code (if needed)</label><input id="otp" autocomplete="one-time-code"><div id="mfa" hidden><label for="method">Two-factor method</label><select id="method"></select><button id="send-mfa" type="button">Send code / approval</button></div><button id="login">Sign in and create display key</button><small>Creates one named “UniFi ESP32 Display” key with your account’s existing permissions. It remains valid until revoked in Console → Integrations.</small></form>
<details><summary>Already have a local key?</summary><form id="reuse-form"><label for="local-key">Existing local Network API key</label><input id="local-key" type="password" autocomplete="off" required maxlength="512"><button>Verify existing key</button></form></details><button id="retry" hidden>Retry local verification</button></section>
<section id="complete" hidden><h2>4 · Ready for USB installation</h2><p>The certificate and all four display API reads are verified over your LAN.</p><button id="return">Use these settings in the installer</button><button id="download">Download local settings</button><small>The download contains your local API key. Keep it private. Wi-Fi credentials stay in the installer.</small></section>
<p id="status" role="status" aria-live="polite">Ready. Keep this helper running until setup is complete.</p><script src="/wizard.js"></script></html>'''

JAVASCRIPT = r'''"use strict";
const $ = id => document.getElementById(id);
const nonce = document.querySelector('meta[name="setup-token"]').content;
const params = new URLSearchParams(location.hash.slice(1));
const parentOrigin = params.get("origin"), channel = params.get("channel");
const allowed = ["https://no-std-rs.github.io", "http://127.0.0.1:4173", "http://localhost:4173"];
let consoles = [], selection = "", result, busy = false, pollTimer;
function note(text, error = false) { $("status").textContent = text; $("status").classList.toggle("error", error); }
async function api(route, data = {}) {
  const response = await fetch("/api/" + route, { method: "POST", headers: { "Content-Type": "application/json", "X-Setup-Token": nonce }, body: JSON.stringify({ ...data, selection }) });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || "Setup request failed.");
  return body;
}
async function run(action) {
  if (busy) return;
  busy = true;
  const controls = [...document.querySelectorAll("button,input,select")].map(control => [control, control.disabled]);
  controls.forEach(([control]) => { control.disabled = true; });
  try { await action(); } catch (error) { note(error.message, true); }
  finally { busy = false; controls.forEach(([control, disabled]) => { control.disabled = disabled; }); }
}
function options(id, rows, preferred = "") {
  $(id).replaceChildren(new Option("Choose…", ""), ...rows.map(row => new Option(row.name || row.id, row.id)));
  $(id).required = true;
  if (preferred && rows.some(row => row.id === preferred)) $(id).value = preferred;
  else if (rows.length === 1) $(id).value = rows[0].id;
  else $(id).value = "";
}
function addresses() {
  const host = consoles.find(c => c.id === $("console").value);
  options("address", (host?.addresses || []).map(id => ({ id })));
}
async function discover() {
  note("Finding online consoles…");
  const data = await api("discover", { cloud_key: $("cloud-key").value });
  $("cloud-key").value = "";
  consoles = data.consoles;
  options("console", consoles); addresses();
  $("choose").hidden = !consoles.length;
  note(consoles.length ? "Choose your console and verify its certificate." : "No online consoles found for this key.");
}
async function devices() {
  options("device", []);
  if (!$("site").value) return;
  const data = await api("devices", { site_id: $("site").value });
  options("device", data.devices, data.gateway);
}
async function provision(localKey = "") {
  if (!$("site").value || !$("device").value) throw new Error("Choose a site and device first.");
  note("Creating local access and verifying the four display API reads…");
  $("retry").hidden = false;
  const data = await api("provision", { site_id: $("site").value, device_id: $("device").value, local_key: localKey });
  clearTimeout(pollTimer);
  result = data.config;
  $("password").value = ""; $("otp").value = ""; $("local-key").value = "";
  $("account").hidden = true; $("complete").hidden = false;
  note("Local connection verified. Return to the installer to add Wi-Fi and flash over USB.");
}
$("discover-form").onsubmit = event => { event.preventDefault(); run(discover); };
$("console").onchange = addresses;
$("inspect-form").onsubmit = event => { event.preventDefault(); run(async () => {
  note("Verifying the gateway’s certificate and identity…");
  const data = await api("inspect", { console_id: $("console").value, address: $("address").value });
  selection = data.selection;
  options("site", data.sites); await devices();
  $("identity").textContent = `Verified gateway: ${data.hostname} → ${data.address}`;
  $("discovery").hidden = true; $("choose").hidden = true; $("account").hidden = false;
  note("Certificate verified. Sign in once, or reuse an existing local key.");
}); };
$("site").onchange = () => run(devices);
$("login-form").onsubmit = event => { event.preventDefault(); run(async () => {
  if (!$("site").value || !$("device").value) throw new Error("Choose a site and device first.");
  clearTimeout(pollTimer);
  note("Signing in to the verified local gateway…");
  const data = await api("login", { username: $("username").value, password: $("password").value, token: $("otp").value });
  if (data.state === "mfa") {
    options("method", data.methods.map(m => ({ id: m.id, name: m.type })));
    $("mfa").hidden = false;
    note("Two-factor authentication required. Enter an authenticator / backup code and sign in again, or send an approval request.");
  } else await provision();
}); };
$("send-mfa").onclick = () => run(async () => {
  clearTimeout(pollTimer);
  await api("mfa", { id: $("method").value });
  const push = $("method").selectedOptions[0]?.textContent === "push";
  note(push ? "Approve the sign-in on your device…" : "Code sent. Enter it above and sign in again.");
  if (push) {
    const deadline = Date.now() + 600000;
    const poll = () => {
      if (Date.now() >= deadline) { note("Approval expired. Sign in again.", true); return; }
      if (busy) { pollTimer = setTimeout(poll, 1500); return; }
      run(async () => {
        const data = await api("poll");
        if (data.state === "authenticated") await provision();
        else pollTimer = setTimeout(poll, 1500);
      });
    };
    pollTimer = setTimeout(poll, 1500);
  }
});
$("reuse-form").onsubmit = event => { event.preventDefault(); run(() => provision($("local-key").value)); };
$("retry").onclick = () => run(() => provision());
$("return").onclick = () => {
  if (window.opener && allowed.includes(parentOrigin) && channel && result) {
    window.opener.postMessage({ type: "unifi-setup-result", channel, config: result }, parentOrigin);
    note("Settings sent. Continue in the installer tab.");
  } else note("Open Automatic setup from the installer, or download and import these settings there.");
};
$("download").onclick = () => {
  const url = URL.createObjectURL(new Blob([JSON.stringify(result, null, 2)], { type: "application/json" }));
  const a = document.createElement("a"); a.href = url; a.download = "unifi-local-settings.json"; a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
};
window.addEventListener("message", event => {
  if (event.source !== window.opener || event.origin !== parentOrigin || !allowed.includes(event.origin)
      || event.data?.type !== "unifi-setup-start" || event.data.channel !== channel || selection) return;
  $("cloud-key").value = typeof event.data.cloud_key === "string" ? event.data.cloud_key : "";
  if ($("cloud-key").value) run(discover);
});
if (window.opener && allowed.includes(parentOrigin) && /^[0-9a-f]{32}$/.test(channel || "")) {
  window.opener.postMessage({ type: "unifi-setup-ready", channel }, parentOrigin);
}
window.addEventListener("pagehide", () => { clearTimeout(pollTimer); $("password").value = ""; $("cloud-key").value = ""; });
'''


def handler(wizard, token, origin):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass  # Request bodies, credentials and tokens never enter logs.

        def reply(self, status, body, content_type='application/json'):
            self.send_response(status)
            self.send_header('Content-Type', content_type + '; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Cache-Control', 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            self.send_header('Referrer-Policy', 'no-referrer')
            self.send_header('Content-Security-Policy', "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")
            self.end_headers()
            self.wfile.write(body)

        def host_ok(self):
            return self.headers.get('Host') == urlsplit(origin).netloc

        def do_GET(self):
            if not self.host_ok():
                self.reply(403, b'{}')
            elif self.path == '/':
                self.reply(200, HTML.replace('TOKEN_PLACEHOLDER', token).encode(), 'text/html')
            elif self.path == '/wizard.js':
                self.reply(200, JAVASCRIPT.encode(), 'text/javascript')
            else:
                self.reply(404, b'{}')

        def do_POST(self):
            supplied = self.headers.get('X-Setup-Token', '')
            if not self.host_ok() or self.headers.get('Origin') != origin or not hmac.compare_digest(supplied, token):
                self.reply(403, b'{"error":"Open the local setup page to continue."}')
                return
            try:
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 < length <= 8192:
                    raise SetupError('Invalid setup request size.')
                payload = json.loads(self.rfile.read(length))
                if not isinstance(payload, dict):
                    raise SetupError('Invalid setup request.')
                routes = {name: getattr(wizard, name) for name in ('discover', 'inspect', 'devices', 'login', 'mfa', 'poll', 'provision')}
                if self.path not in ['/api/' + name for name in routes]:
                    self.reply(404, b'{}')
                    return
                result = routes[self.path.removeprefix('/api/')](payload)
                self.reply(200, json.dumps(result).encode())
            except SetupError as error:
                self.reply(400, json.dumps({'error': str(error)}).encode())
            except ssl.SSLCertVerificationError:
                self.reply(400, b'{"error":"The gateway certificate could not be verified. No console password was sent."}')
            except (OSError, http.client.HTTPException):
                self.reply(502, b'{"error":"Connection failed. Check that this computer is on the gateway LAN. If key creation was in progress, retry verification before restarting setup."}')
            except (ValueError, KeyError, TypeError):
                self.reply(400, b'{"error":"Unexpected setup or UniFi response. Discover again."}')
    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-browser', action='store_true')
    parser.add_argument('--env-file', type=Path, help='Optionally load UNIFI_API_KEY from this local file')
    parser.add_argument('--probe', action='store_true', help='Discover and verify the sole console certificate without signing in or creating a key')
    args = parser.parse_args()
    cloud_key = os.environ.get('UNIFI_API_KEY', '')
    if args.env_file:
        for line in args.env_file.read_text().splitlines():
            if line.startswith('UNIFI_API_KEY='):
                cloud_key = line.split('=', 1)[1].strip().strip('\"\'')
    wizard = Wizard(cloud_key)
    if args.probe:
        consoles = wizard.discover({})['consoles']
        if len(consoles) != 1:
            raise SetupError('The probe needs exactly one online console; use the wizard to choose.')
        print(json.dumps(wizard.inspect({'console_id': consoles[0]['id']}), indent=2))
        return
    origin = 'http://127.0.0.1:4177'
    server = HTTPServer(('127.0.0.1', 4177), handler(wizard, secrets.token_hex(32), origin))
    server.timeout = 1
    print(f'UniFi display setup: {origin}/\nIn the installer, choose Open guided setup.\nKeep this terminal open. Press Ctrl-C to stop.', flush=True)
    if not args.no_browser:
        webbrowser.open('https://no-std-rs.github.io/unifi/#install')
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == '__main__':
    try:
        main()
    except (SetupError, OSError, http.client.HTTPException) as error:
        print(f'Setup stopped: {error}', file=sys.stderr)
        sys.exit(1)
