import type { Command } from '../types.js';
import { UpdateCommand } from './UpdateCommand.js';

export const updateCommand: Command = {
  name: 'update',
  description: 'Download and install the latest release',
  aliases: ['upgrade', 'self-update'],
  Component: UpdateCommand,
  managesExit: true,
};
