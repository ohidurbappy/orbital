/**
 * The kinds of QR code the interactive builder offers, with the fields each
 * needs and a pure `build` that turns the entered values into the string that
 * actually gets encoded. Keeping this as data (not UI) makes every payload
 * format independently testable.
 */
export interface QrField {
  /** Key under which the entered value is stored. */
  key: string;
  /** Prompt label shown to the user. */
  label: string;
  /** Example value shown as a hint when the field is empty. */
  placeholder?: string;
  /** When true the field may be left blank. */
  optional?: boolean;
}

export interface QrType {
  /** Stable identifier. */
  id: string;
  /** Name shown in the type picker. */
  label: string;
  /** One-line description shown next to the label. */
  hint: string;
  /** Ordered fields the user fills in. */
  fields: QrField[];
  /** Turn entered values into the string to encode. */
  build: (values: Record<string, string>) => string;
}

const get = (values: Record<string, string>, key: string): string => (values[key] ?? '').trim();

/** Escape the characters that are significant inside a `WIFI:` payload. */
function escapeWifi(value: string): string {
  return value.replace(/([\\;,:"])/g, '\\$1');
}

/** Prefix a bare host with `https://` so URL codes open in a browser. */
function ensureScheme(url: string): string {
  return /^[a-zA-Z][a-zA-Z\d+.-]*:\/\//.test(url) ? url : `https://${url}`;
}

export const QR_TYPES: QrType[] = [
  {
    id: 'text',
    label: 'Text',
    hint: 'Any plain text',
    fields: [{ key: 'text', label: 'Text' }],
    build: (v) => v.text ?? '',
  },
  {
    id: 'url',
    label: 'URL',
    hint: 'Open a website',
    fields: [{ key: 'url', label: 'URL', placeholder: 'example.com' }],
    build: (v) => ensureScheme(get(v, 'url')),
  },
  {
    id: 'tel',
    label: 'Telephone',
    hint: 'Dial a phone number',
    fields: [{ key: 'number', label: 'Phone number', placeholder: '+15551234567' }],
    build: (v) => `tel:${get(v, 'number')}`,
  },
  {
    id: 'sms',
    label: 'SMS',
    hint: 'Pre-filled text message',
    fields: [
      { key: 'number', label: 'Phone number', placeholder: '+15551234567' },
      { key: 'message', label: 'Message', optional: true },
    ],
    build: (v) => {
      const number = get(v, 'number');
      const message = get(v, 'message');
      return message ? `SMSTO:${number}:${message}` : `SMSTO:${number}`;
    },
  },
  {
    id: 'email',
    label: 'Email',
    hint: 'Pre-filled email',
    fields: [
      { key: 'to', label: 'To', placeholder: 'name@example.com' },
      { key: 'subject', label: 'Subject', optional: true },
      { key: 'body', label: 'Body', optional: true },
    ],
    build: (v) => {
      const to = get(v, 'to');
      const params = new URLSearchParams();
      if (get(v, 'subject')) params.set('subject', get(v, 'subject'));
      if (get(v, 'body')) params.set('body', get(v, 'body'));
      const query = params.toString();
      return query ? `mailto:${to}?${query}` : `mailto:${to}`;
    },
  },
  {
    id: 'wifi',
    label: 'Wi-Fi',
    hint: 'Join a wireless network',
    fields: [
      { key: 'ssid', label: 'Network name (SSID)' },
      { key: 'password', label: 'Password', optional: true },
      { key: 'encryption', label: 'Encryption (WPA/WEP/nopass)', placeholder: 'WPA', optional: true },
    ],
    build: (v) => {
      const ssid = escapeWifi(get(v, 'ssid'));
      const password = escapeWifi(get(v, 'password'));
      const enc = password ? (get(v, 'encryption').toUpperCase() || 'WPA') : 'nopass';
      const passwordPart = enc === 'nopass' ? '' : `P:${password};`;
      return `WIFI:T:${enc};S:${ssid};${passwordPart};`;
    },
  },
  {
    id: 'geo',
    label: 'Location',
    hint: 'Geographic coordinates',
    fields: [
      { key: 'lat', label: 'Latitude', placeholder: '37.7749' },
      { key: 'lng', label: 'Longitude', placeholder: '-122.4194' },
    ],
    build: (v) => `geo:${get(v, 'lat')},${get(v, 'lng')}`,
  },
];
