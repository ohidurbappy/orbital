import { useEffect } from 'react';
import type { ComponentType } from 'react';
import { Box, useApp } from 'ink';
import type { Command, CommandProps } from './commands/types.js';
import { Menu } from './components/Menu.js';
import { UpdateBanner } from './components/UpdateBanner.js';
import { useUpdateCheck } from './core/updater/useUpdateCheck.js';

interface AppProps {
  /** When provided, run this single command. Otherwise show the menu. */
  command?: Command;
  /** Positional CLI args passed through to the command component. */
  args?: string[];
  /** Piped stdin passed through to commands that opt in via `readsStdin`. */
  input?: string;
}

export function App({ command, args, input }: AppProps) {
  const interactive = !command;
  const update = useUpdateCheck(interactive);
  const { exit } = useApp();

  // Print-and-exit commands have no exit logic of their own — release Ink once
  // their first frame has painted so the process can end.
  const autoExit = !!command && !command.managesExit;
  useEffect(() => {
    if (autoExit) exit();
  }, [autoExit, exit]);

  const Content: ComponentType<CommandProps> = command?.Component ?? Menu;

  return (
    <Box flexDirection="column">
      <UpdateBanner update={update} />
      <Content args={args} input={input} />
    </Box>
  );
}
