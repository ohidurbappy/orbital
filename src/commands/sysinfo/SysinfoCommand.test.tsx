import { describe, expect, it } from 'bun:test';
import { render } from 'ink-testing-library';
import { SysinfoCommand } from './SysinfoCommand.js';

describe('<SysinfoCommand>', () => {
  it('renders the standard labels', () => {
    const { lastFrame } = render(<SysinfoCommand />);
    const frame = lastFrame() ?? '';
    for (const label of ['OS', 'Kernel', 'Arch', 'CPU', 'Memory']) {
      expect(frame).toContain(label);
    }
  });
});
