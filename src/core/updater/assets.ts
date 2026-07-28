import type { ReleaseAsset } from './checkForUpdate.js';

/**
 * The release asset name expected for a given platform/arch. Assets are shipped
 * gzipped (see `scripts/build.ts`) and gunzipped by `applyUpdate`. Must match
 * the names produced by the build script and uploaded by the release workflow.
 */
export function assetNameFor(platform: NodeJS.Platform, arch: string): string | null {
  const os = { darwin: 'darwin', linux: 'linux', win32: 'windows' }[platform as string];
  const cpu = { arm64: 'arm64', x64: 'x64' }[arch];
  if (!os || !cpu) return null;
  if (os === 'windows' && cpu !== 'x64') return null; // only build windows-x64
  return os === 'windows' ? `orb-${os}-${cpu}.exe.gz` : `orb-${os}-${cpu}.gz`;
}

export function findAsset(
  assets: ReleaseAsset[],
  platform: NodeJS.Platform = process.platform,
  arch: string = process.arch,
): ReleaseAsset | null {
  const name = assetNameFor(platform, arch);
  if (!name) return null;
  return assets.find((a) => a.name === name) ?? null;
}
