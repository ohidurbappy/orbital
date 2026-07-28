import type { Command } from './types.js';
import { ipCommand } from './ip/index.js';
import { qrCommand } from './qr/index.js';
import { serveCommand } from './serve/index.js';
import { sysinfoCommand } from './sysinfo/index.js';
import { updateCommand } from './update/index.js';

/**
 * The single source of truth for available tools. Register new commands here
 * and they automatically appear in `--help`, the interactive menu, and CLI
 * dispatch.
 */
export const COMMANDS: Command[] = [
  ipCommand,
  qrCommand,
  serveCommand,
  sysinfoCommand,
  updateCommand,
];

/** Resolve a command by its name or one of its aliases. */
export function findCommand(name: string): Command | undefined {
  return COMMANDS.find((c) => c.name === name || c.aliases?.includes(name));
}

export type { Command } from './types.js';
