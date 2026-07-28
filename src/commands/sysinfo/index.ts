import type { Command } from '../types.js';
import { SysinfoCommand } from './SysinfoCommand.js';

export const sysinfoCommand: Command = {
  name: 'sysinfo',
  description: 'Show system information (neofetch-style)',
  aliases: ['sys', 'neofetch'],
  Component: SysinfoCommand,
};
