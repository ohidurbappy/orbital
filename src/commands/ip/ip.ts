import os from 'node:os';

export interface IpEntry {
  /** Network interface name, e.g. `en0`, `eth0`. */
  iface: string;
  /** The assigned address. */
  address: string;
  /** Address family. Node reports `'IPv4'`/`'IPv6'` (older Node used `4`/`6`). */
  family: 'IPv4' | 'IPv6';
}

type NetworkInterfaces = typeof os.networkInterfaces;

/**
 * Collect non-internal (loopback excluded) IP addresses for every interface.
 *
 * The `networkInterfaces` function is injected so tests can pass a fixture
 * instead of mocking the `os` module.
 */
export function getLocalIps(
  networkInterfaces: NetworkInterfaces = os.networkInterfaces,
): IpEntry[] {
  const interfaces = networkInterfaces();
  const entries: IpEntry[] = [];

  for (const [iface, addrs] of Object.entries(interfaces)) {
    if (!addrs) continue;
    for (const addr of addrs) {
      if (addr.internal) continue;
      const family = normalizeFamily(addr.family);
      entries.push({ iface, address: addr.address, family });
    }
  }

  return entries;
}

/**
 * The single most useful "this machine's IP" — the first non-internal IPv4,
 * or the first IPv6 if no IPv4 exists. Returns undefined when offline.
 */
export function getPrimaryIp(entries: IpEntry[]): IpEntry | undefined {
  return entries.find((e) => e.family === 'IPv4') ?? entries[0];
}

/** The first non-internal IPv4 — this machine's address on the LAN. */
export function getLocalIpv4(entries: IpEntry[]): IpEntry | undefined {
  return entries.find((e) => e.family === 'IPv4');
}

function normalizeFamily(family: string | number): 'IPv4' | 'IPv6' {
  if (family === 'IPv6' || family === 6) return 'IPv6';
  return 'IPv4';
}

export interface IpFlags {
  /** `--public`/`-p`: fetch this machine's public IP. */
  public: boolean;
  /** `--local`/`-l`: print just the LAN IPv4, plain (script-friendly). */
  local: boolean;
}

/** Parse the option flags `orb ip` accepts from the forwarded CLI tokens. */
export function parseIpFlags(args: string[] = []): IpFlags {
  const has = (...names: string[]) => args.some((a) => names.includes(a));
  return {
    public: has('--public', '-p'),
    local: has('--local', '-l'),
  };
}

/** Loose check that a string looks like an IPv4 or IPv6 address. */
function looksLikeIp(value: string): boolean {
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(value) || /^[0-9a-fA-F:]+$/.test(value);
}

/**
 * Fetch this machine's public IP from an external echo service. Returns null on
 * any network/parse error so callers can render a friendly message instead of
 * throwing. `fetchFn` is injected for tests (mirrors the DI pattern elsewhere).
 */
export async function getPublicIp(fetchFn: typeof fetch = fetch): Promise<string | null> {
  try {
    const res = await fetchFn('https://api.ipify.org', { signal: AbortSignal.timeout(5000) });
    if (!res.ok) return null;
    const text = (await res.text()).trim();
    return looksLikeIp(text) ? text : null;
  } catch {
    return null;
  }
}

/**
 * Plain-text CLI handler for `orb ip`'s scripting flags. Returns the address to
 * print (so `IP=$(orb ip --local)` is clean), or null with no flag so the
 * caller falls through to the interactive Ink view. Throws on a lookup failure
 * so the runner can exit non-zero. Dependencies are injected for tests.
 */
export async function runIp(
  args: string[] = [],
  deps: { networkInterfaces?: NetworkInterfaces; getPublic?: typeof getPublicIp } = {},
): Promise<string | null> {
  const flags = parseIpFlags(args);

  if (flags.public) {
    const ip = await (deps.getPublic ?? getPublicIp)();
    if (!ip) throw new Error('Could not determine public IP.');
    return ip;
  }

  if (flags.local) {
    const ipv4 = getLocalIpv4(getLocalIps(deps.networkInterfaces));
    if (!ipv4) throw new Error('No LAN IPv4 address found.');
    return ipv4.address;
  }

  return null;
}
