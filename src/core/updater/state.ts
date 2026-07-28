import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { configDir, stateFile } from '../paths.js';
import type { UpdateResult } from './checkForUpdate.js';

export interface UpdateState {
  /** Epoch ms of the last completed check. */
  lastCheck: number;
  result: UpdateResult | null;
}

/** Re-check at most this often (10 minutes). */
export const CHECK_INTERVAL_MS = 10 * 60 * 1000;

export function readState(): UpdateState | null {
  try {
    return JSON.parse(readFileSync(stateFile, 'utf8')) as UpdateState;
  } catch {
    return null;
  }
}

export function writeState(state: UpdateState): void {
  try {
    mkdirSync(dirname(stateFile) || configDir, { recursive: true });
    writeFileSync(stateFile, JSON.stringify(state, null, 2));
  } catch {
    // Persisting the cache is best-effort; never break the CLI over it.
  }
}

/** True when the cache is missing or older than the interval. */
export function isStale(state: UpdateState | null, now: number): boolean {
  if (!state) return true;
  return now - state.lastCheck >= CHECK_INTERVAL_MS;
}
