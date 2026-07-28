import semver from 'semver';
import { VERSION, REPO } from '../version.js';

export interface ReleaseAsset {
  name: string;
  browser_download_url: string;
  size: number;
}

export interface UpdateResult {
  hasUpdate: boolean;
  current: string;
  latest: string | null;
  /** Release page URL for the human. */
  url: string | null;
  assets: ReleaseAsset[];
}

const LATEST_RELEASE_URL = `https://api.github.com/repos/${REPO}/releases/latest`;

/**
 * Query GitHub for the latest release and compare it to the running version.
 *
 * `fetchFn` and `currentVersion` are injectable for tests. Network/parse errors
 * surface a "no update" result rather than throwing — update checks must never
 * break the actual command the user ran.
 */
export async function checkForUpdate(
  fetchFn: typeof fetch = fetch,
  currentVersion: string = VERSION,
): Promise<UpdateResult> {
  const noUpdate: UpdateResult = {
    hasUpdate: false,
    current: currentVersion,
    latest: null,
    url: null,
    assets: [],
  };

  try {
    const res = await fetchFn(LATEST_RELEASE_URL, {
      headers: {
        Accept: 'application/vnd.github+json',
        'User-Agent': `orb/${currentVersion}`,
      },
      signal: AbortSignal.timeout(5000),
    });
    if (!res.ok) return noUpdate;

    const data = (await res.json()) as {
      tag_name?: string;
      html_url?: string;
      assets?: ReleaseAsset[];
    };

    const latest = data.tag_name ? semver.clean(data.tag_name) : null;
    if (!latest || !semver.valid(latest)) return noUpdate;

    return {
      hasUpdate: semver.gt(latest, currentVersion),
      current: currentVersion,
      latest,
      url: data.html_url ?? null,
      assets: data.assets ?? [],
    };
  } catch {
    return noUpdate;
  }
}
