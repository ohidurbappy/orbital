import { describe, expect, it } from 'bun:test';
import {
  encodeQrMatrix,
  renderQrLines,
  resolveQrInput,
  toQrLines,
  type QrEncoder,
} from './qr.js';

describe('renderQrLines', () => {
  it('maps each module pair to the right half-block (light = visible)', () => {
    // One text row from two module rows. Light modules render as blocks.
    const matrix = [
      [true, false], // dark, light
      [false, true], // light, dark
    ];
    // col0: top dark + bottom light → '▄'; col1: top light + bottom dark → '▀'.
    expect(renderQrLines(matrix, 0)).toEqual(['▄▀']);
  });

  it('renders an all-dark matrix as blank cells', () => {
    const matrix = [
      [true, true],
      [true, true],
    ];
    expect(renderQrLines(matrix, 0)).toEqual(['  ']);
  });

  it('surrounds the matrix with a light quiet zone of `margin` modules', () => {
    const lines = renderQrLines([[true]], 1);
    // 1x1 dark + margin 1 → 3x3 grid → 2 text rows; the dark cell sits at (1,1).
    expect(lines).toHaveLength(2);
    // Row 0 (light) over row 1 (light, dark, light) → center is light-on-top.
    expect(lines[0]).toBe('█▀█');
    // Row 2 (light) over the phantom light row → all blocks.
    expect(lines[1]).toBe('███');
  });

  it('treats the phantom row past an odd grid as light', () => {
    // size 1 + margin 0 → dim 1 (odd): single module, no row below it.
    expect(renderQrLines([[true]], 0)).toEqual(['▄']);
  });
});

describe('toQrLines', () => {
  it('encodes via the injected encoder and renders the result', () => {
    const fakeEncoder: QrEncoder = () => [
      [true, false],
      [false, true],
    ];
    expect(toQrLines('ignored', { encode: fakeEncoder, margin: 0 })).toEqual(['▄▀']);
  });
});

describe('encodeQrMatrix', () => {
  it('produces a square matrix with a dark finder pattern in the corner', () => {
    const matrix = encodeQrMatrix('hello', 'M');
    expect(matrix.length).toBeGreaterThan(0);
    expect(matrix.length).toBe(matrix[0]!.length); // square
    // The top-left finder pattern starts with a dark module.
    expect(matrix[0]![0]).toBe(true);
  });
});

describe('resolveQrInput', () => {
  it('prefers positional args joined with a space', () => {
    expect(resolveQrInput(['hello', 'world'], 'piped')).toBe('hello world');
  });

  it('falls back to stdin and trims trailing whitespace', () => {
    expect(resolveQrInput([], 'https://example.com\n')).toBe('https://example.com');
  });

  it('ignores whitespace-only args before using stdin', () => {
    expect(resolveQrInput(['   '], 'frompipe')).toBe('frompipe');
  });

  it('returns undefined when neither source has content', () => {
    expect(resolveQrInput([], '\n')).toBeUndefined();
    expect(resolveQrInput(undefined, undefined)).toBeUndefined();
  });
});
