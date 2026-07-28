import { Box, Text } from 'ink';
import { collectSystemInfo } from './sysinfo.js';
import { KeyValue } from '../../components/KeyValue.js';

/** Small ASCII logos keyed by platform, neofetch-style. */
const LOGOS: Record<string, string[]> = {
  darwin: ['   .:\'    ', ' _ :\'_    ', " (_'\\/_)  ", ' /     \\  ', " \\     /  ", "  `---'   "],
  linux: ['   .--.   ', '  |o_o |  ', '  |:_/ |  ', ' //   \\ \\ ', '(|     | )', "/'\\_   _/`\\"],
  win32: [' .---.---.', ' |   |   |', ' |---+---|', ' |   |   |', " '---'---'", '          '],
};

function logoFor(platform: string): string[] {
  return LOGOS[platform] ?? ['  ___  ', ' / _ \\ ', '| | | |', '| |_| |', ' \\___/ ', '  orb  '];
}

export function SysinfoCommand() {
  const info = collectSystemInfo();
  const logo = logoFor(info.platform);

  const rows: Array<[string, string | null]> = [
    ['User', `${info.username}@${info.hostname}`],
    ['OS', info.osName],
    ['Kernel', info.kernel],
    ['Arch', info.arch],
    ['Uptime', info.uptime],
    ['Shell', info.shell],
    ['CPU', `${info.cpuModel} (${info.cpuCount})`],
    ['Memory', `${info.memUsed} / ${info.memTotal}`],
    ['Load', info.loadAverage],
  ];

  return (
    <Box flexDirection="row">
      <Box flexDirection="column" marginRight={2}>
        {logo.map((line, i) => (
          <Text key={i} color="green">
            {line}
          </Text>
        ))}
      </Box>
      <Box flexDirection="column">
        {rows
          .filter((r): r is [string, string] => r[1] != null)
          .map(([label, value]) => (
            <KeyValue key={label} label={label} value={value} />
          ))}
      </Box>
    </Box>
  );
}
