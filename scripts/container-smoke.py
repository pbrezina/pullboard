#!/usr/bin/env python3
"""Exercise the real empty image; never use live credentials or other containers."""
import argparse
from html.parser import HTMLParser
import json
import subprocess
import time
import urllib.error
import urllib.request
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--engine', default='podman')
parser.add_argument('--image', default='localhost/pullboard:latest')
args = parser.parse_args()
name = 'pullboard-smoke-' + uuid.uuid4().hex[:12]
created = False


class AssetParser(HTMLParser):
    """Catch a CSP-blocked critical-CSS loader without needing a browser install."""
    def __init__(self):
        super().__init__()
        self.assets = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'script':
            assert attrs.get('src'), 'Inline script is blocked by the production CSP'
            self.assets.append(attrs['src'])
        if tag == 'link' and attrs.get('rel') == 'stylesheet':
            assert attrs.get('media') != 'print', 'Stylesheet depends on a CSP-blocked loader'
            self.assets.append(attrs['href'])


def command(*parts):
    return subprocess.check_output([args.engine, *parts], text=True).strip()


def request(path, method='GET', headers=None, payload=None):
    req = urllib.request.Request(base + path, data=payload, method=method, headers=headers or {})
    try:
        response = urllib.request.urlopen(req, timeout=3)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return response.status, response.headers, response.read()


try:
    # Podman rejects raw tmpfs uid/gid options; U maps only this tmpfs to USER.
    ownership = 'U' if 'podman' in command('--version').lower() else 'uid=10001,gid=10001'
    command('run', '-d', '--name', name, '--read-only', '--cap-drop=ALL',
            '--security-opt=no-new-privileges',
            '--tmpfs', '/data:rw,nosuid,nodev,noexec,size=64m,mode=0700,' + ownership,
            '-p', '127.0.0.1::8080', args.image)
    created = True
    mapped = command('port', name, '8080/tcp')
    assert mapped.startswith('127.0.0.1:'), mapped
    base = 'http://' + mapped
    for attempt in range(60):
        try:
            status, headers, body = request('/api/health')
            if status == 200:
                break
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.5)
    else:
        raise RuntimeError('Container failed to become healthy')
    assert command('exec', name, 'id', '-u') == '10001'
    assert command('exec', name, 'cat', '/proc/1/comm') == 'pullboard'
    assert command('exec', name, 'stat', '-c', '%u:%g %a', '/data') == '10001:10001 700'
    assert json.loads(body)['read_only_test_mode'] is True
    assert headers.get('Cache-Control') == 'no-store'
    status, headers, body = request('/')
    assert status == 200 and b'<app-root' in body
    assert "script-src 'self'" in headers.get('Content-Security-Policy', '')
    assets = AssetParser()
    assets.feed(body.decode())
    assert assets.assets
    for asset in assets.assets:
        assert not asset.startswith(('http:', 'https:', '//')), 'External production asset'
        status, _, asset_body = request('/' + asset.lstrip('/'))
        assert status == 200 and asset_body and b'<app-root' not in asset_body
    for path in ['/api/not-an-endpoint', '/api/']:
        status, headers, body = request(path)
        assert status == 404 and 'application/json' in headers.get('Content-Type', '')
        assert 'error' in json.loads(body)
    status, _, _ = request('/api/sync', 'POST', {'Content-Type': 'application/json'}, b'{}')
    assert status == 403
    status, _, _ = request('/api/sync', 'POST', {
        'Content-Type': 'application/json', 'X-PullBoard-Request': '1',
        'Origin': 'https://not-local.invalid'}, b'{}')
    assert status == 403
    status, _, _ = request('/api/sync', 'POST', {
        'Content-Type': 'application/json', 'X-PullBoard-Request': '1'}, b'{}')
    assert status == 202
    status, _, body = request('/api/board')
    board = json.loads(body)
    assert status == 200 and board['issues'] == [] and board['pull_requests'] == []
    print('PASS: health, static SPA/assets/CSP compatibility, JSON API 404, CSRF/origin rejection, refresh, empty board')
    print('Tested loopback URL: ' + base)
except Exception:
    if created:
        subprocess.run([args.engine, 'logs', name], check=False)
    raise
finally:
    if created:
        subprocess.run([args.engine, 'stop', '--time', '5', name], check=False,
                       stdout=subprocess.DEVNULL)
        subprocess.run([args.engine, 'rm', name], check=False, stdout=subprocess.DEVNULL)
