#!/usr/bin/env bash
set -euo pipefail

SECRET="${1:-myNameIsSlimShady}"
EXPIRY_HOURS="${2:-1}"

python3 -c "
import json, base64, hmac, hashlib, time

def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b'=').decode()

header = b64url(json.dumps({'alg': 'HS256', 'typ': 'JWT'}).encode())
payload = b64url(json.dumps({
    'sub': 'test-user',
    'exp': int(time.time()) + ($EXPIRY_HOURS * 3600),
    'iat': int(time.time()),
}).encode())

sig = b64url(hmac.new(
    b'$SECRET',
    f'{header}.{payload}'.encode(),
    hashlib.sha256
).digest())

token = f'{header}.{payload}.{sig}'
print(token)
print()
print('Usage:')
print(f'  curl -k -H \"Authorization: Bearer {token}\" https://localhost:3000/')
"
