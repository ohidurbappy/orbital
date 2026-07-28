import { chmodSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { checkForUpdate } from './checkForUpdate.js';
import { findAsset } from './assets.js';
import { isCompiled } from './refresh.js';

export interface ApplyOutcome {
  status: 'updated' | 'up-to-date' | 'unsupported' | 'no-asset' | 'error';
  message: string;
  from?: string;
  to?: string;
}

/** Phases an update goes through, reported via the optional progress callback. */
export interface UpdateProgress {
  phase: 'checking' | 'downloading' | 'installing';
  /** Total download size in bytes, known once the release asset is resolved. */
  totalBytes?: number;
}

export type ProgressFn = (progress: UpdateProgress) => void;

/**
 * Download the release asset for this platform and atomically replace the
 * running binary. Returns a structured outcome; never throws. `onProgress` is
 * called as the update moves through its phases so the UI can reflect them.
 */
export async function applyUpdate(
  fetchFn: typeof fetch = fetch,
  onProgress: ProgressFn = () => {},
): Promise<ApplyOutcome> {
  if (!isCompiled()) {
    return {
      status: 'unsupported',
      message: 'Self-update only applies to the compiled binary (not `bun run`).',
    };
  }

  onProgress({ phase: 'checking' });
  const result = await checkForUpdate(fetchFn);
  if (!result.hasUpdate || !result.latest) {
    return { status: 'up-to-date', message: `Already on the latest version (${result.current}).` };
  }

  const asset = findAsset(result.assets);
  if (!asset) {
    return {
      status: 'no-asset',
      message: `No release asset for ${process.platform}/${process.arch} in ${result.latest}.`,
    };
  }

  try {
    onProgress({ phase: 'downloading', totalBytes: asset.size });
    const res = await fetchFn(asset.browser_download_url, {
      headers: { 'User-Agent': `orb/${result.current}`, Accept: 'application/octet-stream' },
    });
    if (!res.ok) {
      return { status: 'error', message: `Download failed: HTTP ${res.status}` };
    }
    // Release assets are gzipped to cut download size (~62%); decompress back
    // to the raw executable before swapping it in.
    const bytes = gunzipSync(Buffer.from(await res.arrayBuffer()));

    onProgress({ phase: 'installing' });
    const target = process.execPath;
    const tmp = `${target}.new`;
    writeFileSync(tmp, bytes);
    if (process.platform !== 'win32') chmodSync(tmp, 0o755);

    if (process.platform === 'win32') {
      // Can't overwrite a running .exe; rename it aside, then move the new one in.
      const old = `${target}.old`;
      try {
        rmSync(old, { force: true });
      } catch {
        // ignore
      }
      renameSync(target, old);
      renameSync(tmp, target);
    } else {
      // Unix: replacing the file the process is executing is safe (inode swap).
      renameSync(tmp, target);
    }

    return {
      status: 'updated',
      message: `Updated to ${result.latest}. Restart orb to use the new version.`,
      from: result.current,
      to: result.latest,
    };
  } catch (err) {
    return { status: 'error', message: `Update failed: ${(err as Error).message}` };
  }
}
