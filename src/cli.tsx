#!/usr/bin/env bun
import { render } from 'ink';
import { App } from './app.js';
import { COMMANDS, findCommand } from './commands/index.js';
import { VERSION } from './core/version.js';
import { runRefresh, spawnBackgroundRefresh } from './core/updater/refresh.js';

interface ParsedArgs {
  commandName?: string;
  /** Positional tokens after the command name, forwarded to the command. */
  commandArgs: string[];
  help: boolean;
  version: boolean;
}

export function parseArgs(argv: string[]): ParsedArgs {
  let commandName: string | undefined;
  const commandArgs: string[] = [];
  let help = false;
  let version = false;

  for (const arg of argv) {
    if (arg === '--help' || arg === '-h') help = true;
    else if (arg === '--version' || arg === '-v') version = true;
    else if (!commandName && !arg.startsWith('-')) commandName = arg;
    // Everything after the command name — positionals AND flags like `--public`
    // — is forwarded to the command so it can parse its own options.
    else commandArgs.push(arg);
  }

  return { commandName, commandArgs, help, version };
}

/** Read stdin to completion. Used for commands that accept piped input. */
async function readStdin(): Promise<string> {
  process.stdin.setEncoding('utf8');
  let data = '';
  for await (const chunk of process.stdin) data += chunk;
  return data;
}

function printHelp(): void {
  const lines = [
    'orb — a growable cross-platform CLI toolbox',
    '',
    'Usage:',
    '  orb              Open the interactive menu',
    '  orb <command>    Run a command directly',
    '',
    'Commands:',
    ...COMMANDS.map((c) => `  ${c.name.padEnd(12)} ${c.description}`),
    '',
    'Flags:',
    '  -h, --help       Show this help',
    '  -v, --version    Show version',
  ];
  console.log(lines.join('\n'));
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);

  // Hidden command used by the detached background update check.
  if (argv[0] === '__refresh-update') {
    await runRefresh();
    return;
  }

  const { commandName, commandArgs, help, version } = parseArgs(argv);

  if (version) {
    console.log(VERSION);
    return;
  }
  if (help) {
    printHelp();
    return;
  }

  if (commandName) {
    const command = findCommand(commandName);
    if (!command) {
      console.error(`Unknown command: ${commandName}\n`);
      printHelp();
      process.exitCode = 1;
      return;
    }
    // Pull piped input for commands that want it. Only when no positional args
    // were given (they take precedence, so there's nothing to wait for) and
    // stdin isn't a TTY (an interactive `orb qr` must not block on EOF).
    const wantsStdin =
      command.readsStdin && commandArgs.length === 0 && !process.stdin.isTTY;
    const input = wantsStdin ? await readStdin() : undefined;

    // Plain-output handler (e.g. `orb ip --local`): print to stdout and skip Ink
    // entirely — no menu chrome, no update banner — so the output is pipeable.
    if (command.run) {
      try {
        const out = await command.run(commandArgs, input);
        if (out !== null) {
          console.log(out);
          spawnBackgroundRefresh();
          return;
        }
      } catch (err) {
        console.error(err instanceof Error ? err.message : String(err));
        process.exitCode = 1;
        return;
      }
    }

    const app = render(<App command={command} args={commandArgs} input={input} />);
    await app.waitUntilExit();
    // Refresh the update cache for next time without blocking this run.
    spawnBackgroundRefresh();
    return;
  }

  // No command → interactive menu (its own startup + 10-min poll via the hook).
  const app = render(<App />);
  await app.waitUntilExit();
}

// Only run when invoked directly, so tests can import `parseArgs` without
// triggering the CLI.
if (import.meta.main) void main();
