import type { Command } from '../types.js';
import { IpCommand } from './IpCommand.js';
import { runIp } from './ip.js';

export const ipCommand: Command = {
  name: 'ip',
  description: 'Show IPs — default lists local; --local (LAN IPv4), --public',
  aliases: ['ipaddr'],
  Component: IpCommand,
  // --local / --public print a plain address for scripting (no Ink chrome).
  run: (args) => runIp(args),
};
