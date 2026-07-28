import { spawn } from 'node:child_process';
import { checkForUpdate } from './checkForUpdate.js';
import { readState, writeState, isStale } from './state.js';

/** True when running as a compiled binary rather than via `bun run`. */
export function isCompiled(): boolean {
  const exec = process.execPath.toLowerCase();
  return !exec.endsWith('bun') && !exec.endsWith('bun.exe') && !exec.endsWith('node');
}

/** Build the argv needed to re-invoke this same program with extra args. */
function selfInvocation(extraArgs: string[]): { command: string; args: string[] } {
  if (isCompiled()) {
    return { command: process.execPath, args: extraArgs };
  }
  // Dev: `bun run <script> <extraArgs>`
  const script = process.argv[1] ?? 'src/cli.tsx';
  return { command: process.execPath, args: [script, ...extraArgs] };
}

/** Perform a check and persist the result. Used by the hidden refresh command. */
export async function runRefresh(): Promise<void> {
  const result = await checkForUpdate();
  writeState({ lastCheck: Date.now(), result });
}

/**
 * Fire a detached child process to refresh the update cache, then return
 * immediately so a one-shot command never blocks on the network. No-op when
 * the cache is still fresh.
 */
export function spawnBackgroundRefresh(now: number = Date.now()): void {
  if (!isStale(readState(), now)) return;
  try {
    const { command, args } = selfInvocation(['__refresh-update']);
    const child = spawn(command, args, {
      detached: true,
      stdio: 'ignore',
      env: { ...process.env, ORB_REFRESH: '1' },
    });
    child.unref();
  } catch {
    // Best-effort; never break the foreground command.
  }
}
