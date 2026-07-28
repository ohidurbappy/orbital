import { describe, expect, it } from 'bun:test';
import { render } from 'ink-testing-library';
import { UpdateBanner } from './UpdateBanner.js';
import type { UpdateResult } from '../core/updater/checkForUpdate.js';

const result = (hasUpdate: boolean): UpdateResult => ({
  hasUpdate,
  current: '1.0.0',
  latest: hasUpdate ? '1.1.0' : '1.0.0',
  url: 'http://x',
  assets: [],
});

describe('<UpdateBanner>', () => {
  it('renders nothing when there is no update', () => {
    expect(render(<UpdateBanner update={result(false)} />).lastFrame()).toBe('');
    expect(render(<UpdateBanner update={null} />).lastFrame()).toBe('');
  });

  it('shows the new version and the update command when an update exists', () => {
    const frame = render(<UpdateBanner update={result(true)} />).lastFrame() ?? '';
    expect(frame).toContain('1.1.0');
    expect(frame).toContain('orb update');
  });
});
