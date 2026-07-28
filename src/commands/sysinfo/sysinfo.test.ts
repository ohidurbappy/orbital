import { describe, expect, it } from 'bun:test';
import { collectSystemInfo, type SysinfoDeps } from './sysinfo.js';

function makeDeps(overrides: Partial<SysinfoDeps> = {}): SysinfoDeps {
  return {
    platform: () => 'linux',
    arch: () => 'x64',
    release: () => '6.1.0',
    hostname: () => 'box',
    uptime: () => 90061, // 1d 1h 1m
    totalmem: () => 16 * 1024 ** 3,
    freemem: () => 8 * 1024 ** 3,
    cpus: () => [{ model: 'Test CPU @ 3.0GHz' }, { model: 'Test CPU @ 3.0GHz' }],
    loadavg: () => [0.5, 0.75, 1.0],
    userInfo: () => ({ username: 'tester', shell: '/bin/bash' }),
    env: {},
    readText: () => null,
    runText: () => null,
    ...overrides,
  };
}

describe('collectSystemInfo', () => {
  it('formats memory, uptime, cpu and load', () => {
    const info = collectSystemInfo(makeDeps());
    expect(info.memTotal).toBe('16.00 GiB');
    expect(info.memUsed).toBe('8.00 GiB');
    expect(info.uptime).toBe('1d 1h 1m');
    expect(info.cpuModel).toBe('Test CPU @ 3.0GHz');
    expect(info.cpuCount).toBe(2);
    expect(info.loadAverage).toBe('0.50, 0.75, 1.00');
  });

  it('reads PRETTY_NAME from /etc/os-release on linux', () => {
    const deps = makeDeps({
      readText: () => 'NAME="Ubuntu"\nPRETTY_NAME="Ubuntu 24.04 LTS"\n',
    });
    expect(collectSystemInfo(deps).osName).toBe('Ubuntu 24.04 LTS');
  });

  it('falls back to platform+release when os-release is missing', () => {
    const info = collectSystemInfo(makeDeps({ readText: () => null }));
    expect(info.osName).toBe('linux 6.1.0');
  });

  it('uses sw_vers on darwin', () => {
    const deps = makeDeps({
      platform: () => 'darwin',
      runText: (cmd, args) => {
        if (cmd === 'sw_vers' && args[0] === '-productName') return 'macOS';
        if (cmd === 'sw_vers' && args[0] === '-productVersion') return '15.0';
        return null;
      },
    });
    expect(collectSystemInfo(deps).osName).toBe('macOS 15.0');
  });

  it('omits load average on windows', () => {
    const info = collectSystemInfo(makeDeps({ platform: () => 'win32' }));
    expect(info.loadAverage).toBeNull();
    expect(info.osName).toBe('Windows 6.1.0');
  });
});
