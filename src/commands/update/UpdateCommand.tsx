import { useEffect, useState } from 'react';
import { Box, Text, useApp } from 'ink';
import Spinner from 'ink-spinner';
import {
  applyUpdate,
  type ApplyOutcome,
  type UpdateProgress,
} from '../../core/updater/applyUpdate.js';

const COLORS: Record<ApplyOutcome['status'], string> = {
  updated: 'green',
  'up-to-date': 'cyan',
  unsupported: 'yellow',
  'no-asset': 'yellow',
  error: 'red',
};

export function progressLabel(progress: UpdateProgress): string {
  switch (progress.phase) {
    case 'downloading': {
      const mb = progress.totalBytes ? ` (${(progress.totalBytes / 1024 / 1024).toFixed(1)} MB)` : '';
      return `Downloading update${mb}…`;
    }
    case 'installing':
      return 'Installing…';
    default:
      return 'Checking for updates…';
  }
}

export function UpdateCommand() {
  const { exit } = useApp();
  const [progress, setProgress] = useState<UpdateProgress>({ phase: 'checking' });
  const [outcome, setOutcome] = useState<ApplyOutcome | null>(null);

  useEffect(() => {
    let active = true;
    applyUpdate(fetch, (p) => {
      if (active) setProgress(p);
    }).then((o) => {
      if (!active) return;
      setOutcome(o);
      exit();
    });
    return () => {
      active = false;
    };
  }, [exit]);

  if (!outcome) {
    return (
      <Text>
        <Text color="cyan">
          <Spinner type="dots" />
        </Text>{' '}
        {progressLabel(progress)}
      </Text>
    );
  }

  return (
    <Box>
      <Text color={COLORS[outcome.status]}>{outcome.message}</Text>
    </Box>
  );
}
