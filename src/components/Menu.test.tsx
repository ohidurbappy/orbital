import { describe, expect, it } from 'bun:test';
import { render } from 'ink-testing-library';
import { Menu } from './Menu.js';

const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

describe('<Menu>', () => {
  it('lists the available commands with a search prompt', () => {
    const frame = render(<Menu />).lastFrame() ?? '';
    expect(frame).toContain('ip');
    expect(frame).toContain('sysinfo');
    expect(frame).toContain('Type to search');
  });

  it('filters the list as the user types', async () => {
    const { stdin, lastFrame } = render(<Menu />);
    await tick(); // let useInput subscribe after mount
    stdin.write('sys');
    await tick();
    const frame = lastFrame() ?? '';
    expect(frame).toContain('sysinfo');
    expect(frame).not.toContain('update');
    expect(frame).toContain('❯ '); // typed query echoed in the prompt
  });

  it('shows a notice when nothing matches', async () => {
    const { stdin, lastFrame } = render(<Menu />);
    await tick();
    stdin.write('zzzzz');
    await tick();
    expect(lastFrame() ?? '').toContain('No matching tools');
  });

  it('opens the highlighted command on Enter and shows the back hint', async () => {
    const { stdin, lastFrame } = render(<Menu />);
    await tick();
    stdin.write('\r'); // Enter on the first item (ip)
    await tick();
    expect(lastFrame() ?? '').toContain('press Esc to go back');
  });
});
