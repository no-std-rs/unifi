#!/usr/bin/env python3
"""Publish a CA/hostname-verified api.ui.com leaf pin; no API credentials needed."""
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import ssl
import subprocess
import sys


def metadata():
    # Match the firmware's TLS 1.3 cipher and supported signature algorithms.
    handshake = subprocess.run([
        'openssl', 's_client', '-showcerts', '-verify_return_error',
        '-verify_hostname', 'api.ui.com', '-servername', 'api.ui.com',
        '-connect', 'api.ui.com:443', '-tls1_3',
        '-ciphersuites', 'TLS_AES_128_GCM_SHA256', '-sigalgs',
        'ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pss_rsae_sha384:rsa_pss_rsae_sha512',
    ], input=b'', capture_output=True, timeout=30, check=True)
    pem = handshake.stdout.decode('ascii')
    start = pem.index('-----BEGIN CERTIFICATE-----')
    end = pem.index('-----END CERTIFICATE-----', start) + len('-----END CERTIFICATE-----')
    leaf = pem[start:end]
    expiry = subprocess.run(['openssl', 'x509', '-noout', '-enddate'],
                            input=leaf, text=True, capture_output=True, check=True).stdout.strip()
    not_after = datetime.strptime(expiry.removeprefix('notAfter='), '%b %d %H:%M:%S %Y %Z').replace(tzinfo=timezone.utc)
    now = datetime.now(timezone.utc)
    if not_after <= now:
        raise ValueError('expired cloud certificate')
    return {
        'origin': 'https://api.ui.com',
        'certificate_sha256': hashlib.sha256(ssl.PEM_cert_to_DER_cert(leaf)).hexdigest(),
        'not_after': not_after.isoformat(),
        'verified_at': now.isoformat(),
    }


if __name__ == '__main__':
    Path(sys.argv[1]).write_text(json.dumps(metadata(), indent=2) + '\n')
    print('Published a CA/hostname-verified api.ui.com TLS 1.3 certificate fingerprint')
