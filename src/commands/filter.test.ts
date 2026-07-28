import { describe, expect, it } from 'bun:test';
import { fuzzyScore, filterCommands } from './filter.js';
import type { Command } from './types.js';

const noop = () => null;
const cmd = (name: string, description: string, aliases?: string[]): Command => ({
  name,
  description,
  aliases,
  Component: noop as unknown as Command['Component'],
});

const commands = [
  cmd('ip', 'Print local IP address(es)', ['ipaddr']),
  cmd('sysinfo', 'Show system information (neofetch-style)', ['sys', 'neofetch']),
  cmd('update', 'Download and install the latest release', ['upgrade']),
];

describe('fuzzyScore', () => {
  it('returns 0 for an empty query', () => {
    expect(fuzzyScore('anything', '')).toBe(0);
  });

  it('returns null when the query is not a subsequence', () => {
    expect(fuzzyScore('ip', 'xyz')).toBeNull();
  });

  it('matches subsequences', () => {
    expect(fuzzyScore('sysinfo', 'sfo')).not.toBeNull();
  });

  it('scores a prefix match higher than a scattered one', () => {
    const prefix = fuzzyScore('sysinfo', 'sys')!;
    const scattered = fuzzyScore('sysinfo', 'sfo')!;
    expect(prefix).toBeGreaterThan(scattered);
  });
});

describe('filterCommands', () => {
  it('returns everything in order for an empty query', () => {
    expect(filterCommands(commands, '').map((c) => c.name)).toEqual(['ip', 'sysinfo', 'update']);
  });

  it('filters by name', () => {
    expect(filterCommands(commands, 'sys').map((c) => c.name)).toEqual(['sysinfo']);
  });

  it('matches aliases', () => {
    expect(filterCommands(commands, 'neofetch').map((c) => c.name)).toEqual(['sysinfo']);
    expect(filterCommands(commands, 'upgrade').map((c) => c.name)).toEqual(['update']);
  });

  it('matches description text when the name does not match', () => {
    expect(filterCommands(commands, 'release').map((c) => c.name)).toEqual(['update']);
  });

  it('ranks a name match above a description-only match', () => {
    // "in" appears in "sysinfo"/"install"; the name match should rank first.
    const names = filterCommands(commands, 'in').map((c) => c.name);
    expect(names[0]).toBe('sysinfo');
  });

  it('returns nothing when no command matches', () => {
    expect(filterCommands(commands, 'zzzzz')).toEqual([]);
  });
});
