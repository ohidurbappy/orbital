import os from 'node:os';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';

export interface SystemInfo {
  osName: string;
  platform: NodeJS.Platform;
  arch: string;
  kernel: string;
  hostname: string;
  username: string;
  uptime: string;
  cpuModel: string;
  cpuCount: number;
  memTotal: string;
  memUsed: string;
  loadAverage: string | null;
  shell: string | null;
}

/**
 * Side-effecting dependencies, injected so `collectSystemInfo` is pure & testable.
 */
export interface SysinfoDeps {
  platform: () => NodeJS.Platform;
  arch: () => string;
  release: () => string;
  hostname: () => string;
  uptime: () => number;
  totalmem: () => number;
  freemem: () => number;
  cpus: () => Array<{ model: string }>;
  loadavg: () => number[];
  userInfo: () => { username: string; shell: string | null };
  env: NodeJS.ProcessEnv;
  /** Reads a text file, returning null on any error. */
  readText: (path: string) => string | null;
  /** Runs a command for the pretty OS name, returning null on any error. */
  runText: (cmd: string, args: string[]) => string | null;
}

export const realDeps: SysinfoDeps = {
  platform: os.platform,
  arch: os.arch,
  release: os.release,
  hostname: os.hostname,
  uptime: os.uptime,
  totalmem: os.totalmem,
  freemem: os.freemem,
  cpus: () => os.cpus(),
  loadavg: os.loadavg,
  userInfo: () => {
    const info = os.userInfo();
    return { username: info.username, shell: info.shell ?? null };
  },
  env: process.env,
  readText: (path) => {
    try {
      return readFileSync(path, 'utf8');
    } catch {
      return null;
    }
  },
  runText: (cmd, args) => {
    try {
      return execFileSync(cmd, args, { encoding: 'utf8', timeout: 2000 }).trim();
    } catch {
      return null;
    }
  },
};

export function collectSystemInfo(deps: SysinfoDeps = realDeps): SystemInfo {
  const platform = deps.platform();
  const cpus = deps.cpus();
  const total = deps.totalmem();
  const free = deps.freemem();
  const user = deps.userInfo();

  return {
    osName: prettyOsName(deps, platform),
    platform,
    arch: deps.arch(),
    kernel: deps.release(),
    hostname: deps.hostname(),
    username: user.username,
    uptime: formatUptime(deps.uptime()),
    cpuModel: cpus[0]?.model.trim() ?? 'Unknown',
    cpuCount: cpus.length,
    memTotal: formatBytes(total),
    memUsed: formatBytes(total - free),
    loadAverage: platform === 'win32' ? null : formatLoad(deps.loadavg()),
    shell: user.shell ?? deps.env.SHELL ?? null,
  };
}

/** Best-effort human OS name with graceful per-platform fallback. */
function prettyOsName(deps: SysinfoDeps, platform: NodeJS.Platform): string {
  if (platform === 'linux') {
    const osRelease = deps.readText('/etc/os-release');
    const name = osRelease && parseOsRelease(osRelease);
    if (name) return name;
  }
  if (platform === 'darwin') {
    const product = deps.runText('sw_vers', ['-productName']);
    const version = deps.runText('sw_vers', ['-productVersion']);
    if (product) return version ? `${product} ${version}` : product;
    return 'macOS';
  }
  if (platform === 'win32') {
    return `Windows ${deps.release()}`;
  }
  return `${platform} ${deps.release()}`;
}

function parseOsRelease(content: string): string | null {
  const map = new Map<string, string>();
  for (const line of content.split('\n')) {
    const eq = line.indexOf('=');
    if (eq === -1) continue;
    const key = line.slice(0, eq).trim();
    const value = line.slice(eq + 1).trim().replace(/^"|"$/g, '');
    map.set(key, value);
  }
  return map.get('PRETTY_NAME') ?? map.get('NAME') ?? null;
}

function formatUptime(seconds: number): string {
  const s = Math.floor(seconds);
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const mins = Math.floor((s % 3600) / 60);
  const parts: string[] = [];
  if (days) parts.push(`${days}d`);
  if (hours) parts.push(`${hours}h`);
  parts.push(`${mins}m`);
  return parts.join(' ');
}

function formatBytes(bytes: number): string {
  const gib = bytes / 1024 ** 3;
  if (gib >= 1) return `${gib.toFixed(2)} GiB`;
  const mib = bytes / 1024 ** 2;
  return `${mib.toFixed(0)} MiB`;
}

function formatLoad(load: number[]): string {
  return load.map((n) => n.toFixed(2)).join(', ');
}
