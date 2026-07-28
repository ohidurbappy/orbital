import { describe, expect, it } from 'bun:test';
import { progressLabel } from './UpdateCommand.js';

describe('progressLabel', () => {
  it('labels the checking phase', () => {
    expect(progressLabel({ phase: 'checking' })).toBe('Checking for updates…');
  });

  it('labels the downloading phase with the size when known', () => {
    expect(progressLabel({ phase: 'downloading', totalBytes: 24 * 1024 * 1024 })).toBe(
      'Downloading update (24.0 MB)…',
    );
  });

  it('labels the downloading phase without a size when unknown', () => {
    expect(progressLabel({ phase: 'downloading' })).toBe('Downloading update…');
  });

  it('labels the installing phase', () => {
    expect(progressLabel({ phase: 'installing' })).toBe('Installing…');
  });
});
