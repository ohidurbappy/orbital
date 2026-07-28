import type { Command } from '../types.js';
import { ServeCommand } from './ServeCommand.js';

export const serveCommand: Command = {
  name: 'serve',
  description: 'Serve the current directory over HTTP (e.g. orb serve 8080)',
  aliases: ['http'],
  Component: ServeCommand,
  // Long-lived: the server runs until the user stops it (Esc / Ctrl-C).
  managesExit: true,
};
