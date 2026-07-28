import type { Command } from '../types.js';
import { QrCommand } from './QrCommand.js';

export const qrCommand: Command = {
  name: 'qr',
  description: 'Encode text (argument or piped stdin) into a QR code',
  aliases: ['qrcode'],
  Component: QrCommand,
  readsStdin: true,
  managesExit: true,
};
