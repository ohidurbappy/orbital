import { useEffect, useRef, useState } from 'react';
import { Box, Text, useApp, useInput, useStdin } from 'ink';
import type { CommandProps } from '../types.js';
import { toQrLines } from '../qr/qr.js';
import { resolveServePort, serveUrls, startServer, type ServeServer } from './serve.js';

export function ServeCommand({ args }: CommandProps) {
  const { exit } = useApp();
  // Coerce to a real boolean: Ink reports `undefined` for a non-TTY, and passing
  // `isActive: undefined` to useInput makes it default to true (and then throw
  // when it tries to enable raw mode on a stream that doesn't support it).
  const canReadKeys = Boolean(useStdin().isRawModeSupported);
  const port = resolveServePort(args);
  const root = process.cwd();

  const serverRef = useRef<ServeServer | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    try {
      serverRef.current = startServer(root, port);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(`Could not start server on port ${port}: ${message}`);
      exit();
    }
    return () => {
      void serverRef.current?.stop();
    };
  }, [root, port, exit]);

  // Esc / Ctrl-C stop the server (only when we can read keys).
  useInput(
    (input, key) => {
      if (key.escape || (key.ctrl && input === 'c')) {
        serverRef.current?.stop();
        exit();
      }
    },
    { isActive: canReadKeys },
  );

  if (error) return <Text color="red">{error}</Text>;

  const urls = serveUrls(port);
  const qrTarget = urls.network ?? urls.local;
  let qrLines: string[] = [];
  try {
    qrLines = toQrLines(qrTarget);
  } catch {
    qrLines = [];
  }

  return (
    <Box flexDirection="column">
      <Text>
        <Text bold color="green">
          Serving{' '}
        </Text>
        <Text>{root}</Text>
      </Text>

      <Box flexDirection="column" marginTop={1}>
        <Box>
          <Box width={10}>
            <Text dimColor>Local</Text>
          </Box>
          <Text color="cyan">{urls.local}</Text>
        </Box>
        {urls.network && (
          <Box>
            <Box width={10}>
              <Text dimColor>Network</Text>
            </Box>
            <Text color="cyan">{urls.network}</Text>
          </Box>
        )}
      </Box>

      {qrLines.length > 0 && (
        <Box flexDirection="column" marginTop={1}>
          <Text dimColor>Scan to open on your phone ({qrTarget}):</Text>
          <Box flexDirection="column" marginTop={1}>
            {qrLines.map((line, i) => (
              <Text key={i}>{line}</Text>
            ))}
          </Box>
        </Box>
      )}

      <Box marginTop={1}>
        <Text dimColor>{canReadKeys ? 'Press Esc to stop.' : 'Press Ctrl-C to stop.'}</Text>
      </Box>
    </Box>
  );
}
