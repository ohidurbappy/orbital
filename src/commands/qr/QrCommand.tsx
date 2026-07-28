import { useEffect, useState } from 'react';
import { Box, Text, useApp, useInput, useStdin } from 'ink';
import type { CommandProps } from '../types.js';
import { resolveQrInput, toQrLines } from './qr.js';
import { QR_TYPES } from './qrTypes.js';

type Stage = 'pick' | 'fill' | 'render' | 'usage';

export function QrCommand({ args, input }: CommandProps) {
  const { exit } = useApp();
  const { isRawModeSupported } = useStdin();
  const presetText = resolveQrInput(args, input);

  // Interactive only when there's no payload yet AND we can read keystrokes.
  const interactive = !presetText && isRawModeSupported;

  const [stage, setStage] = useState<Stage>(
    presetText ? 'render' : isRawModeSupported ? 'pick' : 'usage',
  );
  const [typeIndex, setTypeIndex] = useState(0);
  const [fieldIndex, setFieldIndex] = useState(0);
  const [values, setValues] = useState<Record<string, string>>({});
  const [buffer, setBuffer] = useState('');
  const [payload, setPayload] = useState<string | null>(null);

  // Non-interactive runs (payload from args/stdin, or no TTY to prompt on)
  // print a single frame and then release Ink so the process can exit.
  useEffect(() => {
    if (!interactive) exit();
  }, [interactive, exit]);

  const type = QR_TYPES[typeIndex]!;

  useInput(
    (char, key) => {
      // Final screen: any key dismisses.
      if (stage === 'render') {
        exit();
        return;
      }

      if (stage === 'pick') {
        if (key.escape) {
          exit();
        } else if (key.upArrow) {
          setTypeIndex((i) => Math.max(0, i - 1));
        } else if (key.downArrow) {
          setTypeIndex((i) => Math.min(QR_TYPES.length - 1, i + 1));
        } else if (key.return) {
          setValues({});
          setBuffer('');
          setFieldIndex(0);
          setStage('fill');
        }
        return;
      }

      // stage === 'fill'
      if (key.escape) {
        setStage('pick');
        setValues({});
        setBuffer('');
        setFieldIndex(0);
        return;
      }
      const field = type.fields[fieldIndex]!;
      if (key.return) {
        if (!buffer.trim() && !field.optional) return; // required field can't be empty
        const next = { ...values, [field.key]: buffer };
        if (fieldIndex + 1 < type.fields.length) {
          setValues(next);
          setFieldIndex(fieldIndex + 1);
          setBuffer('');
        } else {
          setValues(next);
          setPayload(type.build(next));
          setStage('render');
        }
        return;
      }
      if (key.backspace || key.delete) {
        setBuffer((b) => b.slice(0, -1));
        return;
      }
      if (char && !key.ctrl && !key.meta) {
        setBuffer((b) => b + char);
      }
    },
    { isActive: interactive },
  );

  const finalText = presetText ?? payload;
  if (stage === 'render' && finalText != null) {
    return <QrResult text={finalText} dismissable={interactive} />;
  }

  if (stage === 'usage') {
    return (
      <Box flexDirection="column">
        <Text color="yellow">Nothing to encode.</Text>
        <Text dimColor>Usage: orb qr {'<text>'}</Text>
        <Text dimColor>   or: echo {'"text"'} | orb qr</Text>
        <Text dimColor>Run in a terminal with no argument to pick a type interactively.</Text>
      </Box>
    );
  }

  if (stage === 'pick') {
    return (
      <Box flexDirection="column">
        <Text bold color="cyan">
          What kind of QR code?
        </Text>
        <Text dimColor>↑/↓ to move · Enter to select · Esc to quit</Text>
        <Box flexDirection="column" marginTop={1}>
          {QR_TYPES.map((t, i) => {
            const selected = i === typeIndex;
            return (
              <Box key={t.id}>
                <Text color={selected ? 'green' : undefined}>{selected ? '❯ ' : '  '}</Text>
                <Box width={14}>
                  <Text bold={selected} color={selected ? 'green' : 'cyan'}>
                    {t.label}
                  </Text>
                </Box>
                <Text dimColor={!selected}>{t.hint}</Text>
              </Box>
            );
          })}
        </Box>
      </Box>
    );
  }

  // stage === 'fill'
  const field = type.fields[fieldIndex]!;
  return (
    <Box flexDirection="column">
      <Box>
        <Text bold color="cyan">
          {type.label}
        </Text>
        <Text dimColor> — Enter to confirm · Esc to go back</Text>
      </Box>
      <Box flexDirection="column" marginTop={1}>
        {type.fields.slice(0, fieldIndex).map((f) => (
          <Box key={f.key}>
            <Box width={26}>
              <Text dimColor>{f.label}</Text>
            </Box>
            <Text>{values[f.key] || <Text dimColor>(skipped)</Text>}</Text>
          </Box>
        ))}
        <Box>
          <Box width={26}>
            <Text bold color="green">
              {field.label}
              {field.optional ? ' (optional)' : ''}
            </Text>
          </Box>
          <Text>{buffer}</Text>
          <Text color="green">▏</Text>
          {buffer === '' && field.placeholder && <Text dimColor> e.g. {field.placeholder}</Text>}
        </Box>
      </Box>
    </Box>
  );
}

function QrResult({ text, dismissable }: { text: string; dismissable: boolean }) {
  let lines: string[];
  try {
    lines = toQrLines(text);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return <Text color="red">Could not encode QR: {message}</Text>;
  }

  return (
    <Box flexDirection="column">
      {lines.map((line, i) => (
        <Text key={i}>{line}</Text>
      ))}
      <Box marginTop={1}>
        <Text dimColor>{text}</Text>
      </Box>
      {dismissable && <Text dimColor>Press any key to exit.</Text>}
    </Box>
  );
}
