import { describe, expect, it } from 'bun:test';
import semver from 'semver';
import { reconcile } from './useUpdateCheck.js';
import { VERSION } from '../version.js';
import type { UpdateResult } from './checkForUpdate.js';

const cached = (over: Partial<UpdateResult>): UpdateResult => ({
  hasUpdate: false,
  current: 'unknown',
  latest: null,
  url: null,
  assets: [],
  ...over,
});

describe('reconcile', () => {
  it('passes null through', () => {
    expect(reconcile(null)).toBeNull();
  });

  it('clears a stale hasUpdate when the cached latest is not newer than this binary', () => {
    // Cache written by an older binary: claims an update, but `latest` <= us.
    const result = reconcile(cached({ hasUpdate: true, current: '0.0.1', latest: VERSION }));
    expect(result?.current).toBe(VERSION);
    expect(result?.hasUpdate).toBe(false);
  });

  it('reports an update when the cached latest really is newer', () => {
    const newer = semver.inc(VERSION, 'major')!;
    const result = reconcile(cached({ hasUpdate: false, current: '0.0.1', latest: newer }));
    expect(result?.hasUpdate).toBe(true);
    expect(result?.latest).toBe(newer);
    expect(result?.current).toBe(VERSION);
  });

  it('treats a missing or invalid latest as no update', () => {
    expect(reconcile(cached({ hasUpdate: true, latest: null }))?.hasUpdate).toBe(false);
    expect(reconcile(cached({ hasUpdate: true, latest: 'nightly' }))?.hasUpdate).toBe(false);
  });
});
