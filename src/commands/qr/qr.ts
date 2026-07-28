import QRCode from 'qrcode';

/** QR error-correction levels, lowest (most capacity) to highest (most redundancy). */
export type QrErrorLevel = 'L' | 'M' | 'Q' | 'H';

/**
 * Produce the QR module matrix for `text`. `true` is a dark module.
 *
 * The encoder is injected so the renderer can be tested against fixed matrices
 * without pulling in the real encoder (mirrors the DI pattern in `ip.ts`).
 */
export type QrEncoder = (text: string, level: QrErrorLevel) => boolean[][];

export const encodeQrMatrix: QrEncoder = (text, level) => {
  const qr = QRCode.create(text, { errorCorrectionLevel: level });
  const { size, data } = qr.modules;
  const matrix: boolean[][] = [];
  for (let r = 0; r < size; r++) {
    const row: boolean[] = [];
    for (let c = 0; c < size; c++) row.push(data[r * size + c] === 1);
    matrix.push(row);
  }
  return matrix;
};

/**
 * Render a module matrix to terminal lines using vertical half-blocks so each
 * text row carries two module rows. Light modules are drawn as the visible
 * block (and dark as empty), which keeps the code scannable on the dark
 * terminal backgrounds that are the common default.
 *
 * A quiet zone of `margin` light modules is added around the matrix so the
 * finder patterns aren't flush against surrounding text.
 */
export function renderQrLines(matrix: boolean[][], margin = 1): string[] {
  const size = matrix.length;
  const dim = size + margin * 2;

  // Dark inside the data area, light everywhere in the quiet zone.
  const isDark = (r: number, c: number): boolean => {
    const mr = r - margin;
    const mc = c - margin;
    if (mr < 0 || mc < 0 || mr >= size || mc >= size) return false;
    return matrix[mr]![mc]!;
  };

  const lines: string[] = [];
  for (let r = 0; r < dim; r += 2) {
    let line = '';
    for (let c = 0; c < dim; c++) {
      // Rows past the matrix (when `dim` is odd) read as the light quiet zone.
      const topDark = isDark(r, c);
      const bottomDark = r + 1 < dim ? isDark(r + 1, c) : false;
      line += glyph(topDark, bottomDark);
    }
    lines.push(line);
  }
  return lines;
}

function glyph(topDark: boolean, bottomDark: boolean): string {
  if (!topDark && !bottomDark) return '█'; // █ both light
  if (!topDark && bottomDark) return '▀'; // ▀ light on top only
  if (topDark && !bottomDark) return '▄'; // ▄ light on bottom only
  return ' '; // both dark
}

export interface QrOptions {
  level?: QrErrorLevel;
  margin?: number;
  encode?: QrEncoder;
}

/** Encode `text` and render it to terminal lines in one step. */
export function toQrLines(text: string, options: QrOptions = {}): string[] {
  const { level = 'M', margin = 1, encode = encodeQrMatrix } = options;
  return renderQrLines(encode(text, level), margin);
}

/**
 * Decide what to encode: an explicit positional argument wins; otherwise fall
 * back to piped stdin (with its trailing newline trimmed). Returns `undefined`
 * when neither yields any content.
 */
export function resolveQrInput(args?: string[], stdin?: string): string | undefined {
  const fromArgs = args?.join(' ').trim();
  if (fromArgs) return fromArgs;
  const fromStdin = stdin?.replace(/\s+$/, '');
  if (fromStdin) return fromStdin;
  return undefined;
}
