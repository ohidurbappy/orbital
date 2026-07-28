import { Box, Text } from 'ink';
import { getLocalIps, getPrimaryIp } from './ip.js';

export function IpCommand() {
  const entries = getLocalIps();
  const primary = getPrimaryIp(entries);

  if (entries.length === 0) {
    return <Text color="yellow">No non-internal network interfaces found.</Text>;
  }

  return (
    <Box flexDirection="column">
      {primary && (
        <Box marginBottom={1}>
          <Text>
            <Text bold color="green">
              Local IP:{' '}
            </Text>
            <Text bold>{primary.address}</Text>
            <Text dimColor> ({primary.iface})</Text>
          </Text>
        </Box>
      )}
      {entries.map((e) => (
        <Box key={`${e.iface}-${e.address}`}>
          <Box width={10}>
            <Text color="cyan">{e.iface}</Text>
          </Box>
          <Box width={6}>
            <Text dimColor>{e.family}</Text>
          </Box>
          <Text>{e.address}</Text>
        </Box>
      ))}
    </Box>
  );
}
