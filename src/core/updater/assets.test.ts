import { describe, expect, it } from 'bun:test';
import { assetNameFor, findAsset } from './assets.js';
import type { ReleaseAsset } from './checkForUpdate.js';

describe('assetNameFor', () => {
  it('maps platforms and arches to gzipped asset names', () => {
    expect(assetNameFor('darwin', 'arm64')).toBe('orb-darwin-arm64.gz');
    expect(assetNameFor('linux', 'x64')).toBe('orb-linux-x64.gz');
    expect(assetNameFor('win32', 'x64')).toBe('orb-windows-x64.exe.gz');
  });

  it('returns null for unsupported combinations', () => {
    expect(assetNameFor('win32', 'arm64')).toBeNull();
    expect(assetNameFor('freebsd' as NodeJS.Platform, 'x64')).toBeNull();
    expect(assetNameFor('linux', 'ia32')).toBeNull();
  });
});

describe('findAsset', () => {
  const assets: ReleaseAsset[] = [
    { name: 'orb-linux-x64.gz', browser_download_url: 'u1', size: 1 },
    { name: 'orb-darwin-arm64.gz', browser_download_url: 'u2', size: 2 },
  ];

  it('finds the matching asset for a platform/arch', () => {
    expect(findAsset(assets, 'darwin', 'arm64')?.browser_download_url).toBe('u2');
  });

  it('returns null when no asset matches', () => {
    expect(findAsset(assets, 'win32', 'x64')).toBeNull();
  });
});
