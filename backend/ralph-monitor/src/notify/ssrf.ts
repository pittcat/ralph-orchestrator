/**
 * SSRF guard for the webhook channel (Plan4-U2, D5/KTD4).
 *
 * Policy: https by default; http only for allowlisted targets. Every
 * resolved address must be a public IP — loopback, private,
 * link-local, multicast, and unspecified ranges are rejected. DNS is
 * resolved at config/test time via `dns.lookup(..., all:true)`; a
 * lookup failure fails closed. `redirect:'manual'` at the fetch layer
 * means the target can never bounce us somewhere unchecked.
 *
 * Known residue (plan §20): TOCTOU — DNS may shift between validate
 * and connect; pinning the resolved IP is a later version.
 */

import { isIPv4, isIPv6 } from 'node:net';
import { lookup } from 'node:dns/promises';

export type SsrfVerdict =
  | { ok: true; addresses: string[] }
  | { ok: false; code: string; message: string };

const IPV4_BLOCKS: [number, number][] = [
  // [network, mask bits]
  [0x00000000, 8], // 0.0.0.0/8 "this host"
  [0x0a000000, 8], // 10/8 private
  [0x7f000000, 8], // 127/8 loopback
  [0xa9fe0000, 16], // 169.254/16 link-local
  [0xac100000, 12], // 172.16/12 private
  [0xc0a80000, 16], // 192.168/16 private
  [0xe0000000, 4], // 224/4 multicast
  [0xf0000000, 4], // 240/4 reserved
];

function ipToInt(ip: string): number {
  const parts = ip.split('.').map(Number);
  return (
    ((parts[0] << 24) | (parts[1] << 16) | (parts[2] << 8) | parts[3]) >>> 0
  );
}

function isPrivateV4(ip: string): boolean {
  const n = ipToInt(ip);
  return IPV4_BLOCKS.some(
    ([net, bits]) => (n >>> (32 - bits)) === (net >>> (32 - bits)),
  );
}

function isPrivateV6(ip: string): boolean {
  const v = ip.toLowerCase();
  if (v === '::1' || v === '::' || v.startsWith('0:0:0:0:0:0:0:1')) return true;
  if (v.startsWith('fc') || v.startsWith('fd')) return true; // fc00::/7
  if (/^fe[89ab]/.test(v)) return true; // fe80::/10
  if (v.startsWith('ff')) return true; // multicast ff00::/8
  // IPv4-mapped forms (::ffff:127.0.0.1 / ::ffff:7f00:1).
  const mapped = v.match(/::ffff:(\d+\.\d+\.\d+\.\d+)$/);
  if (mapped) return isPrivateV4(mapped[1]);
  const hexMapped = v.match(/::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/);
  if (hexMapped) {
    const hi = parseInt(hexMapped[1], 16);
    const lo = parseInt(hexMapped[2], 16);
    const ipv4 = `${(hi >> 8) & 0xff}.${hi & 0xff}.${(lo >> 8) & 0xff}.${lo & 0xff}`;
    return isPrivateV4(ipv4);
  }
  return false;
}

export function isPublicIp(ip: string): boolean {
  if (isIPv4(ip)) return !isPrivateV4(ip);
  if (isIPv6(ip)) return !isPrivateV6(ip);
  return false;
}

export interface AllowlistEntry {
  host?: string;
  cidr?: string;
}

/** `RALPH_MONITOR_WEBHOOK_ALLOWLIST` — comma-separated host or CIDR strings. */
export function readAllowlistEnv(
  env: NodeJS.ProcessEnv = process.env,
): string[] {
  return (env.RALPH_MONITOR_WEBHOOK_ALLOWLIST ?? '')
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
}

function hostAllowlisted(host: string, allowlist: string[]): boolean {
  return allowlist.some((e) => !e.includes('/') && e === host);
}

function ipAllowlisted(ip: string, allowlist: string[]): boolean {
  return allowlist.some((e) => {
    if (!e.includes('/') || !isIPv4(ip)) return false;
    const [net, bits] = e.split('/');
    const b = Number(bits);
    if (!isIPv4(net) || !Number.isFinite(b) || b < 0 || b > 32) return false;
    return ipToInt(ip) >>> (32 - b) === ipToInt(net) >>> (32 - b);
  });
}

/**
 * Validate `url` for webhook delivery. `allowTestLoopback` lets the
 * test-delivery channel reach loopback targets only (D10) — it never
 * opens private ranges generally.
 */
export async function validateWebhookUrl(
  url: string,
  opts: { allowlist?: string[]; allowTestLoopback?: boolean } = {},
): Promise<SsrfVerdict> {
  const allowlist = opts.allowlist ?? readAllowlistEnv();
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return { ok: false, code: 'invalid_url', message: 'unparseable URL' };
  }
  if (u.username || u.password) {
    return {
      ok: false,
      code: 'credentials_in_url',
      message: 'URLs with credentials are rejected',
    };
  }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') {
    return {
      ok: false,
      code: 'bad_protocol',
      message: 'only http(s) webhook targets are allowed',
    };
  }
  const host = u.hostname.toLowerCase();
  const isLoopback =
    host === 'localhost' ||
    host === '127.0.0.1' ||
    host === '::1' ||
    host === '[::1]' ||
    host.endsWith('.localhost');

  let addresses: { address: string }[];
  try {
    addresses = await lookup(host, { all: true });
  } catch (err) {
    return {
      ok: false,
      code: 'dns_failed',
      message: `DNS resolution failed: ${err instanceof Error ? err.message : err}`,
    };
  }
  if (addresses.length === 0) {
    return { ok: false, code: 'dns_empty', message: 'DNS resolved to nothing' };
  }
  const ips = addresses.map((a) => a.address);
  const allowlisted =
    hostAllowlisted(host, allowlist) ||
    ips.every((ip) => ipAllowlisted(ip, allowlist));
  const testOk =
    opts.allowTestLoopback === true &&
    isLoopback &&
    ips.every((ip) => ip === '127.0.0.1' || ip === '::1');

  if (u.protocol === 'http:' && !allowlisted && !testOk) {
    return {
      ok: false,
      code: 'http_not_allowed',
      message: 'http targets require an allowlist entry (https by default)',
    };
  }
  for (const ip of ips) {
    if (allowlisted) break; // an allowlisted target is trusted wholesale
    if (ipAllowlisted(ip, allowlist)) continue;
    if (testOk && (ip === '127.0.0.1' || ip === '::1')) continue;
    if (!isPublicIp(ip)) {
      return {
        ok: false,
        code: 'private_address',
        message: `resolves to non-public address ${ip}`,
      };
    }
  }
  return { ok: true, addresses: ips };
}
