import type { Command } from './types.js';

/**
 * Score how well `query` fuzzy-matches `text` (case-insensitive subsequence).
 * Returns `null` when the query is not a subsequence of the text. Higher is a
 * better match: contiguous runs, word-boundary hits, and an early first match
 * all score higher.
 */
export function fuzzyScore(text: string, query: string): number | null {
  if (query === '') return 0;
  const t = text.toLowerCase();
  const q = query.toLowerCase();

  let score = 0;
  let ti = 0;
  let prevMatch = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi]!;
    const found = t.indexOf(ch, ti);
    if (found === -1) return null;

    score += 1;
    if (found === prevMatch + 1) score += 5; // contiguous run
    if (found === 0) score += 8; // matches very start
    else if (!isWordChar(t[found - 1]!)) score += 3; // word boundary
    score -= found - ti; // penalize skipped chars

    prevMatch = found;
    ti = found + 1;
  }
  return score;
}

function isWordChar(ch: string): boolean {
  return /[a-z0-9]/.test(ch);
}

/**
 * Filter and rank commands by a query. Matches against the command name (or its
 * aliases) and description, weighting name matches highest. An empty query
 * returns every command in registry order.
 */
export function filterCommands(commands: Command[], query: string): Command[] {
  const trimmed = query.trim();
  if (trimmed === '') return commands;

  const scored = commands
    .map((command, index) => ({ command, index, score: scoreCommand(command, trimmed) }))
    .filter((entry): entry is typeof entry & { score: number } => entry.score !== null);

  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((entry) => entry.command);
}

function scoreCommand(command: Command, query: string): number | null {
  const nameScores = [command.name, ...(command.aliases ?? [])].map((n) => fuzzyScore(n, query));
  const bestName = max(nameScores);
  const descScore = fuzzyScore(command.description, query);

  if (bestName === null && descScore === null) return null;
  // Name matches dominate; a description-only match still surfaces, ranked lower.
  return Math.max(bestName === null ? -Infinity : bestName * 3, descScore ?? -Infinity);
}

function max(values: Array<number | null>): number | null {
  let best: number | null = null;
  for (const v of values) {
    if (v !== null && (best === null || v > best)) best = v;
  }
  return best;
}
