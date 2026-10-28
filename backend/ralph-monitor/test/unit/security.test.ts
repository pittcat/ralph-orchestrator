import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  csrfSatisfied,
  isAllowedHost,
  isAllowedOrigin,
  parseCookies,
} from '../../src/plugins/security.js';

test('host allowlist accepts loopback hostnames at any port', () => {
  for (const h of [
    'localhost:3200',
    'localhost',
    '127.0.0.1:3200',
    '127.0.0.1',
    '[::1]:3200',
    '[::1]',
  ]) {
    assert.equal(isAllowedHost(h), true, h);
  }
  for (const h of [
    'evil.example.com',
    'localhost.evil.com',
    '10.0.0.1',
    'localhost:3200.evil.com',
    '',
    undefined,
  ]) {
    assert.equal(isAllowedHost(h), false, `${h}`);
  }
});

test('origin allowlist: absent ok, loopback ok, cross-site and null rejected', () => {
  assert.equal(isAllowedOrigin(undefined), true);
  assert.equal(isAllowedOrigin('http://localhost:5173'), true);
  assert.equal(isAllowedOrigin('http://127.0.0.1:3200'), true);
  assert.equal(isAllowedOrigin('null'), false);
  assert.equal(isAllowedOrigin('http://evil.example.com'), false);
  assert.equal(isAllowedOrigin('file:///etc/passwd'), false);
  assert.equal(isAllowedOrigin('not a url'), false);
});

test('cookie parser handles values, quoting-free tokens, missing header', () => {
  assert.deepEqual(parseCookies('a=1; b=two'), { a: '1', b: 'two' });
  assert.deepEqual(parseCookies(undefined), {});
  assert.deepEqual(parseCookies(''), {});
});

test('csrfSatisfied requires sid==csrf cookie==header', () => {
  const req = (headers: Record<string, string | undefined>) =>
    ({ headers }) as never;
  const good = req({
    cookie: 'monitor_sid=T; monitor_csrf=T',
    'x-monitor-csrf': 'T',
  });
  assert.equal(csrfSatisfied(good), true);

  for (const headers of [
    { cookie: 'monitor_sid=T; monitor_csrf=T' }, // no header
    { cookie: 'monitor_sid=A; monitor_csrf=B', 'x-monitor-csrf': 'B' },
    { cookie: 'monitor_csrf=T', 'x-monitor-csrf': 'T' }, // no sid
    { 'x-monitor-csrf': 'T' }, // no cookies
    {},
  ]) {
    assert.equal(csrfSatisfied(req(headers)), false, JSON.stringify(headers));
  }
});
