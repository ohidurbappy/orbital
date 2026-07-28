import { Box, Text } from 'ink';

interface KeyValueProps {
  label: string;
  value: string;
  /** Width reserved for the label column so values line up. */
  labelWidth?: number;
  color?: string;
}

/** A single aligned `label  value` row, reused across command output. */
export function KeyValue({ label, value, labelWidth = 12, color = 'cyan' }: KeyValueProps) {
  return (
    <Box>
      <Box width={labelWidth}>
        <Text color={color} bold>
          {label}
        </Text>
      </Box>
      <Text>{value}</Text>
    </Box>
  );
}
