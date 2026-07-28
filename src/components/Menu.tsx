import { useMemo, useState } from 'react';
import { Box, Text, useApp, useInput } from 'ink';
import { COMMANDS } from '../commands/index.js';
import { filterCommands } from '../commands/filter.js';
import type { Command } from '../commands/types.js';

export function Menu() {
  const { exit } = useApp();
  const [active, setActive] = useState<Command | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);

  const results = useMemo(() => filterCommands(COMMANDS, query), [query]);
  // Keep the highlighted row inside the (possibly shrunken) result list.
  const index = results.length === 0 ? 0 : Math.min(selected, results.length - 1);

  useInput((input, key) => {
    if (active) {
      if (key.escape) setActive(null);
      return;
    }

    if (key.escape) {
      if (query) {
        setQuery('');
        setSelected(0);
      } else {
        exit();
      }
      return;
    }
    if (key.return) {
      const choice = results[index];
      if (choice) setActive(choice);
      return;
    }
    if (key.upArrow || (key.ctrl && input === 'p')) {
      setSelected((i) => Math.max(0, Math.min(i, results.length - 1) - 1));
      return;
    }
    if (key.downArrow || (key.ctrl && input === 'n')) {
      setSelected((i) => Math.min(results.length - 1, i + 1));
      return;
    }
    if (key.backspace || key.delete) {
      setQuery((q) => q.slice(0, -1));
      setSelected(0);
      return;
    }
    // Printable character → extend the search query.
    if (input && !key.ctrl && !key.meta) {
      setQuery((q) => q + input);
      setSelected(0);
    }
  });

  if (active) {
    const Active = active.Component;
    return (
      <Box flexDirection="column">
        <Box marginBottom={1}>
          <Text bold color="green">
            {active.name}
          </Text>
          <Text dimColor> — press Esc to go back</Text>
        </Box>
        <Active />
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      <Box>
        <Text bold color="cyan">
          ❯{' '}
        </Text>
        <Text>{query}</Text>
        {query === '' && <Text dimColor>Type to search… (↑/↓, Enter; Esc to quit)</Text>}
      </Box>

      <Box flexDirection="column" marginTop={1}>
        {results.length === 0 ? (
          <Text dimColor>No matching tools.</Text>
        ) : (
          results.map((command, i) => {
            const isSelected = i === index;
            return (
              <Box key={command.name}>
                <Text color={isSelected ? 'green' : undefined}>{isSelected ? '❯ ' : '  '}</Text>
                <Box width={12}>
                  <Text bold={isSelected} color={isSelected ? 'green' : 'cyan'}>
                    {command.name}
                  </Text>
                </Box>
                <Text dimColor={!isSelected}>{command.description}</Text>
              </Box>
            );
          })
        )}
      </Box>
    </Box>
  );
}
