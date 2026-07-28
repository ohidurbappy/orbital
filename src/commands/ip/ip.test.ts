import { describe, expect, it } from 'bun:test';
import {
  getLocalIps,
  getLocalIpv4,
  getPrimaryIp,
  getPublicIp,
  parseIpFlags,
  runIp,
} from './ip.js';

const fixture = () =>
  ({
    lo0: [{ address: '127.0.0.1', family: 'IPv4', internal: true } as any],
    en0: [
      { address: 'fe80::1', family: 'IPv6', internal: false } as any,
      { address: '192.168.1.5', family: 'IPv4', internal: false } as any,
    ],
  }) as ReturnType<typeof import('node:os').networkInterfaces>;

describe('getLocalIps', () => {
  it('excludes internal addresses', () => {
    const ips = getLocalIps(fixture);
    expect(ips.some((e) => e.address === '127.0.0.1')).toBe(false);
  });

  it('returns both IPv4 and IPv6 non-internal addresses', () => {
    const ips = getLocalIps(fixture);
    expect(ips).toEqual([
      { iface: 'en0', address: 'fe80::1', family: 'IPv6' },
      { iface: 'en0', address: '192.168.1.5', family: 'IPv4' },
    ]);
  });

  it('handles no interfaces', () => {
    expect(getLocalIps(() => ({}))).toEqual([]);
  });
});

describe('getPrimaryIp', () => {
  it('prefers the first IPv4', () => {
    const ips = getLocalIps(fixture);
    expect(getPrimaryIp(ips)?.address).toBe('192.168.1.5');
  });

  it('falls back to the first entry when no IPv4 exists', () => {
    const ipv6Only = getLocalIps(
      () => ({ en0: [{ address: 'fe80::1', family: 'IPv6', internal: false } as any] }) as any,
    );
    expect(getPrimaryIp(ipv6Only)?.family).toBe('IPv6');
  });

  it('returns undefined for an empty list', () => {
    expect(getPrimaryIp([])).toBeUndefined();
  });
});

describe('getLocalIpv4', () => {
  it('returns the first IPv4 entry, ignoring IPv6', () => {
    const ips = getLocalIps(fixture);
    expect(getLocalIpv4(ips)?.address).toBe('192.168.1.5');
  });

  it('returns undefined when only IPv6 addresses exist', () => {
    const ipv6Only = getLocalIps(
      () => ({ en0: [{ address: 'fe80::1', family: 'IPv6', internal: false } as any] }) as any,
    );
    expect(getLocalIpv4(ipv6Only)).toBeUndefined();
  });
});

describe('parseIpFlags', () => {
  it('defaults to no flags', () => {
    expect(parseIpFlags()).toEqual({ public: false, local: false });
    expect(parseIpFlags([])).toEqual({ public: false, local: false });
  });

  it('recognizes long and short forms', () => {
    expect(parseIpFlags(['--public'])).toEqual({ public: true, local: false });
    expect(parseIpFlags(['-p'])).toEqual({ public: true, local: false });
    expect(parseIpFlags(['--local'])).toEqual({ public: false, local: true });
    expect(parseIpFlags(['-l'])).toEqual({ public: false, local: true });
  });
});

describe('getPublicIp', () => {
  const fetchReturning = (body: string, ok = true) =>
    (async () => new Response(body, { status: ok ? 200 : 500 })) as unknown as typeof fetch;

  it('returns the trimmed address on success', async () => {
    expect(await getPublicIp(fetchReturning('203.0.113.7\n'))).toBe('203.0.113.7');
  });

  it('returns null on a non-OK response', async () => {
    expect(await getPublicIp(fetchReturning('nope', false))).toBeNull();
  });

  it('returns null when the body is not an IP', async () => {
    expect(await getPublicIp(fetchReturning('<html>error</html>'))).toBeNull();
  });

  it('returns null when the request throws', async () => {
    const throwing = (async () => {
      throw new Error('offline');
    }) as unknown as typeof fetch;
    expect(await getPublicIp(throwing)).toBeNull();
  });
});

describe('runIp', () => {
  const ipv4Fixture = () =>
    ({
      en0: [{ address: '192.168.1.5', family: 'IPv4', internal: false } as any],
    }) as ReturnType<typeof import('node:os').networkInterfaces>;

  it('returns null with no scripting flag (falls through to the UI)', async () => {
    expect(await runIp([])).toBeNull();
  });

  it('--local returns the plain LAN IPv4 string', async () => {
    expect(await runIp(['--local'], { networkInterfaces: ipv4Fixture })).toBe('192.168.1.5');
  });

  it('--local throws when there is no IPv4', () => {
    expect(runIp(['-l'], { networkInterfaces: () => ({}) as any })).rejects.toThrow(
      'No LAN IPv4',
    );
  });

  it('--public returns the fetched address', async () => {
    expect(await runIp(['--public'], { getPublic: async () => '203.0.113.7' })).toBe('203.0.113.7');
  });

  it('--public throws when the lookup fails', () => {
    expect(runIp(['-p'], { getPublic: async () => null })).rejects.toThrow('public IP');
  });
});
