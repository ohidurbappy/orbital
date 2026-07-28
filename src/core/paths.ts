import envPaths from 'env-paths';
import { join } from 'node:path';

const paths = envPaths('orb', { suffix: '' });

/** Directory for orb's persisted state (e.g. ~/.config/orb on Linux). */
export const configDir = paths.config;

/** Cached update-check result lives here. */
export const stateFile = join(configDir, 'state.json');
