import { useEffect, useState } from 'react';
import semver from 'semver';
import type { UpdateResult } from './checkForUpdate.js';
import { checkForUpdate } from './checkForUpdate.js';
import { VERSION } from '../version.js';
import { readState, writeState, isStale, CHECK_INTERVAL_MS } from './state.js';

/**
 * Re-derive the update verdict against the version actually running. The cache
 * may have been written by a different (older) binary — e.g. before `orb
 * update`, or by another orb on the same machine — so its frozen `hasUpdate`
 * and `current` can't be trusted. Only `latest`/`url`/`assets` are reused.
 */
export function reconcile(result: UpdateResult | null): UpdateResult | null {
  if (!result) return null;
  const hasUpdate =
    !!result.latest && semver.valid(result.latest) !== null
      ? semver.gt(result.latest, VERSION)
      : false;
  return { ...result, current: VERSION, hasUpdate };
}

/**
 * Returns the latest known update result. Reads the cache immediately for an
 * instant banner, then (in interactive mode) checks on startup and every 10
 * minutes while the app stays open.
 */
export function useUpdateCheck(poll: boolean): UpdateResult | null {
  const [result, setResult] = useState<UpdateResult | null>(() =>
    reconcile(readState()?.result ?? null),
  );

  useEffect(() => {
    let active = true;

    const refresh = async () => {
      const fresh = await checkForUpdate();
      writeState({ lastCheck: Date.now(), result: fresh });
      if (active) setResult(reconcile(fresh));
    };

    if (isStale(readState(), Date.now())) void refresh();

    if (!poll) return;
    const timer = setInterval(() => void refresh(), CHECK_INTERVAL_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [poll]);

  return result;
}
