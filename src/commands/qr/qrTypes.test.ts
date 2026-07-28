import { describe, expect, it } from 'bun:test';
import { QR_TYPES, type QrType } from './qrTypes.js';

const byId = (id: string): QrType => {
  const type = QR_TYPES.find((t) => t.id === id);
  if (!type) throw new Error(`unknown type ${id}`);
  return type;
};

describe('QR_TYPES build()', () => {
  it('text passes the value through unchanged', () => {
    expect(byId('text').build({ text: 'hello world' })).toBe('hello world');
  });

  it('url prepends https:// only when no scheme is present', () => {
    expect(byId('url').build({ url: 'example.com' })).toBe('https://example.com');
    expect(byId('url').build({ url: 'http://example.com' })).toBe('http://example.com');
  });

  it('telephone uses the tel: scheme', () => {
    expect(byId('tel').build({ number: '+15551234567' })).toBe('tel:+15551234567');
  });

  it('sms includes the message only when provided', () => {
    expect(byId('sms').build({ number: '+15551234567' })).toBe('SMSTO:+15551234567');
    expect(byId('sms').build({ number: '+15551234567', message: 'hi' })).toBe(
      'SMSTO:+15551234567:hi',
    );
  });

  it('email builds a mailto with optional subject/body', () => {
    expect(byId('email').build({ to: 'a@b.com' })).toBe('mailto:a@b.com');
    expect(byId('email').build({ to: 'a@b.com', subject: 'Hi there', body: 'Yo' })).toBe(
      'mailto:a@b.com?subject=Hi+there&body=Yo',
    );
  });

  it('wifi encodes encryption, escapes specials, and uses nopass without a password', () => {
    expect(byId('wifi').build({ ssid: 'home', password: 'pa;ss', encryption: 'wpa' })).toBe(
      'WIFI:T:WPA;S:home;P:pa\\;ss;;',
    );
    expect(byId('wifi').build({ ssid: 'guest' })).toBe('WIFI:T:nopass;S:guest;;');
  });

  it('geo joins latitude and longitude', () => {
    expect(byId('geo').build({ lat: '37.7749', lng: '-122.4194' })).toBe('geo:37.7749,-122.4194');
  });
});
