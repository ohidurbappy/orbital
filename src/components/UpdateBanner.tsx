import { Box, Text } from 'ink';
import type { UpdateResult } from '../core/updater/checkForUpdate.js';

export function UpdateBanner({ update }: { update: UpdateResult | null }) {
  if (!update?.hasUpdate) return null;

  return (
    <Box borderStyle="round" borderColor="yellow" paddingX={1} marginBottom={1}>
      <Text>
        <Text color="yellow">⬆ Update available: </Text>
        <Text dimColor>{update.current}</Text>
        <Text> → </Text>
        <Text bold color="green">
          {update.latest}
        </Text>
        <Text> — run </Text>
        <Text bold color="cyan">
          orb update
        </Text>
      </Text>
    </Box>
  );
}
