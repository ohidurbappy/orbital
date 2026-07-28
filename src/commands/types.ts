import type { ComponentType } from 'react';

/**
 * Inputs the one-shot runner resolves from argv/stdin and hands to a command's
 * component. Both are optional — most commands ignore them and simply render.
 */
export interface CommandProps {
  /** Positional CLI tokens after the command name, e.g. `orb qr hi there` → `['hi', 'there']`. */
  args?: string[];
  /** Text piped via stdin, populated only when the command sets `readsStdin`. */
  input?: string;
}

/**
 * A single tool in the orb toolbox.
 *
 * To add a new command: create a folder under `src/commands/<name>/`, export a
 * `Command` descriptor, and register it in `src/commands/index.ts`. The registry
 * is the single source of truth used by both the CLI dispatcher and the menu.
 */
export interface Command {
  /** Canonical name used on the CLI, e.g. `orb ip`. */
  name: string;
  /** One-line description shown in `--help` and the interactive menu. */
  description: string;
  /** Optional alternate names that also resolve to this command. */
  aliases?: string[];
  /** Ink component rendered when the command runs. */
  Component: ComponentType<CommandProps>;
  /**
   * Optional plain-text handler for one-shot, script-friendly invocations (e.g.
   * `orb ip --local`). Return a string to print to stdout and skip Ink entirely
   * — no menu chrome, no update banner — or null to fall through to the Ink
   * `Component`. Throw to print the message to stderr and exit non-zero. Only
   * consulted for one-shot CLI runs, never from the interactive menu.
   */
  run?(args: string[], input?: string): string | null | Promise<string | null>;
  /**
   * When true the component drives its own exit (async work, interactivity) and
   * the one-shot runner will not auto-exit after first paint. Defaults to false
   * for simple print-and-exit commands like `ip` and `sysinfo`.
   */
  managesExit?: boolean;
  /**
   * When true and stdin is piped (not a TTY), the one-shot runner reads it fully
   * and passes the contents as the `input` prop. Used by commands like `qr` that
   * accept their payload from a pipe.
   */
  readsStdin?: boolean;
}
