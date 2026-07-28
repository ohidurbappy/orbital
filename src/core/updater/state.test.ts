import { describe, expect, it } from 'bun:test';
import { isStale, CHECK_INTERVAL_MS, type UpdateState } from './state.js';

const state = (lastCheck: number): UpdateState => ({ lastCheck, result: null });

describe('isStale', () => {
  it('is stale when there is no cached state', () => {
    expect(isStale(null, 1000)).toBe(true);
  });

  it('is fresh within the interval', () => {
    const now = 1_000_000;
    expect(isStale(state(now - 1000), now)).toBe(false);
  });

  it('is stale once the interval has elapsed', () => {
    const now = 1_000_000;
    expect(isStale(state(now - CHECK_INTERVAL_MS), now)).toBe(true);
  });
});
