import { describe, expect, it } from 'bun:test';
import { render } from 'ink-testing-library';
import { QrCommand } from './QrCommand.js';

// Non-interactive runs call exit() in an effect, which unmounts the Ink tree.
// Under `debug: true` (what ink-testing-library uses) ink's unmount path can
// overwrite the final buffer when it thinks it's in CI, so assert against the
// rendered history rather than the clobberable last frame.
const QR_GLYPHS = /[█▀▄]/;
const qrFrame = (frames: readonly string[]): string =>
  frames.find((f) => QR_GLYPHS.test(f)) ?? '';

const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));
const DOWN = '[B';

describe('<QrCommand>', () => {
  it('renders a QR code directly from a positional argument', () => {
    const { frames } = render(<QrCommand args={['https://example.com']} />);
    const frame = qrFrame(frames);
    expect(QR_GLYPHS.test(frame)).toBe(true);
    expect(frame.split('\n').length).toBeGreaterThan(5);
  });

  it('renders a QR code directly from piped input', () => {
    const { frames } = render(<QrCommand input={'hello\n'} />);
    expect(QR_GLYPHS.test(qrFrame(frames))).toBe(true);
  });

  it('offers a type picker when run interactively with no argument', () => {
    const frame = render(<QrCommand />).lastFrame() ?? '';
    expect(frame).toContain('What kind of QR code?');
    expect(frame).toContain('Text');
    expect(frame).toContain('Wi-Fi');
  });

  it('walks from picker through a field prompt to a rendered QR', async () => {
    const { stdin, lastFrame } = render(<QrCommand />);
    await tick(); // let useInput subscribe
    stdin.write('\r'); // select the first type (Text)
    await tick();
    expect(lastFrame() ?? '').toContain('Enter to confirm'); // now on the field prompt
    stdin.write('hello');
    await tick();
    stdin.write('\r'); // confirm the field → encode
    await tick();
    expect(/[█▀▄]/.test(lastFrame() ?? '')).toBe(true);
  });

  it('lets the user move the selection before choosing a type', async () => {
    const { stdin, lastFrame } = render(<QrCommand />);
    await tick();
    stdin.write(DOWN); // move to the second type (URL)
    await tick();
    stdin.write('\r');
    await tick();
    const frame = lastFrame() ?? '';
    expect(frame).toContain('URL');
    expect(frame).toContain('e.g. example.com'); // URL field placeholder
  });
});
