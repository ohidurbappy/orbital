import { describe, expect, it } from 'bun:test';
import { render } from 'ink-testing-library';
import { IpCommand } from './IpCommand.js';

describe('<IpCommand>', () => {
  it('renders without crashing and shows a heading or empty notice', () => {
    const { lastFrame } = render(<IpCommand />);
    const frame = lastFrame() ?? '';
    // Either we found interfaces (Local IP) or we reported none — both valid.
    expect(/Local IP:|No non-internal/.test(frame)).toBe(true);
  });
});
