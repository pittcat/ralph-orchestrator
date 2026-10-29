import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  isPublicIp,
  readAllowlistEnv,
  validateWebhookUrl,
} from '../../src/notify/ssrf.js';
import { signBody } from '../../src/notify/channels/webhook.js';
import { createHmac } from 'node:crypto';

test('isPublicIp classifies the private/reserved matrix', () => {
  for (const ip of [
    '10.0.0.1',
    '10.255.255.255',
    '172.16.0.1',
    '172.31.255.1',
    '192.168.1.1',
    '127.0.0.1',
    '169.254.169.254',
    '0.0.0.0',
    '224.0.0.1',
    '::1',
    'fd00::1',
    'fe80::1',
    'ff02::1',
    '::ffff:127.0.0.1',
    '::ffff:7f00:1',
  ]) {
    assert.equal(isPublicIp(ip), false, ip);
  }
  for (const ip of ['8.8.8.8', '1.1.1.1', '172.15.0.1', '172.32.0.1', '2606:4700:4700::1111']) {
    assert.equal(isPublicIp(ip), true, ip);
  }
});

test('validateWebhookUrl rejects non-http(s), credentials, and literal private hosts', async () => {
  for (const [url, code] of [
    ['ftp://example.com/x', 'bad_protocol'],
    ['https://user:pw@example.com/x', 'credentials_in_url'],
    ['http://169.254.169.254/latest', 'http_not_allowed'],
    ['https://169.254.169.254/latest', 'private_address'],
    ['https://127.0.0.1/x', 'private_address'],
    ['https://10.0.0.1/x', 'private_address'],
  ] as const) {
    const v = await validateWebhookUrl(url, { allowlist: [] });
    assert.equal(v.ok, false, url);
    assert.equal((v as { code: string }).code, code, url);
  }
});

test('http targets need an allowlist hit; allowlisted CIDR passes', async () => {
  const miss = await validateWebhookUrl('http://internal.corp/hook', {
    allowlist: [],
  });
  assert.equal(miss.ok, false);
  // localhost resolves to 127.0.0.1 — allowlisting the host permits http.
  const hit = await validateWebhookUrl('http://localhost:9/hook', {
    allowlist: ['localhost'],
  });
  assert.equal(hit.ok, true);
  // Literal v4 loopback keeps the CIDR check deterministic — `localhost`
  // may resolve to ::1 + 127.0.0.1, and ::1 is outside 127.0.0.0/8.
  const cidr = await validateWebhookUrl('http://127.0.0.1:9/hook', {
    allowlist: ['127.0.0.0/8'],
  });
  assert.equal(cidr.ok, true);
});

test('test channel allows loopback only for loopback targets', async () => {
  const loop = await validateWebhookUrl('http://127.0.0.1:9/hook', {
    allowlist: [],
    allowTestLoopback: true,
  });
  assert.equal(loop.ok, true);
  // …but never a private non-loopback target.
  const priv = await validateWebhookUrl('http://192.168.1.5/hook', {
    allowlist: [],
    allowTestLoopback: true,
  });
  assert.equal(priv.ok, false);
});

test('unresolvable host fails closed', async () => {
  // A >253-char hostname is invalid at the DNS protocol level, so lookup
  // fails locally — deterministic even behind wildcard/captive resolvers.
  const v = await validateWebhookUrl(`https://${'x'.repeat(300)}/x`, {
    allowlist: [],
  });
  assert.equal(v.ok, false);
});

test('readAllowlistEnv parses the comma list', () => {
  assert.deepEqual(
    readAllowlistEnv({
      RALPH_MONITOR_WEBHOOK_ALLOWLIST: ' a.example ,10.0.0.0/8 ',
    } as NodeJS.ProcessEnv),
    ['a.example', '10.0.0.0/8'],
  );
});

test('signBody matches an independent HMAC computation', () => {
  const body = '{"a":1}';
  const expected = `sha256=${createHmac('sha256', 'sekret').update(body).digest('hex')}`;
  assert.equal(signBody('sekret', body), expected);
});
