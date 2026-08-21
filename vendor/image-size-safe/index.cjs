'use strict';

// This intentionally implements only the image formats accepted by Nexa's
// PptxGenJS adapter.  Complex container formats (ICNS/JXL/HEIF and friends)
// remain unsupported instead of entering attacker-controlled scanning loops.

const fs = require('node:fs');

const MAX_INPUT_BYTES = 4 * 1024 * 1024;
let filesystemEnabled = true;
let disabledTypes = new Set();

function fail(message) {
  throw new TypeError(message);
}

function boundedBytes(input) {
  if (input instanceof Uint8Array || Buffer.isBuffer(input)) {
    if (input.byteLength === 0 || input.byteLength > MAX_INPUT_BYTES) {
      fail('image input is empty or exceeds the 4 MiB inspection budget');
    }
    return Buffer.from(input.buffer, input.byteOffset, input.byteLength);
  }
  if (typeof input !== 'string' || !filesystemEnabled) {
    fail('input must be a Uint8Array or an enabled filesystem path');
  }
  const stat = fs.statSync(input);
  if (!stat.isFile() || stat.size === 0 || stat.size > MAX_INPUT_BYTES) {
    fail('image file is empty, not a regular file, or exceeds the 4 MiB inspection budget');
  }
  return fs.readFileSync(input);
}

function assertEnabled(type) {
  if (disabledTypes.has(type)) fail(`disabled file type: ${type}`);
}

function pngSize(bytes) {
  if (bytes.length < 24 || bytes.toString('hex', 0, 8) !== '89504e470d0a1a0a') return null;
  assertEnabled('png');
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  return width > 0 && height > 0 ? { width, height, type: 'png' } : fail('invalid PNG dimensions');
}

function gifSize(bytes) {
  if (bytes.length < 10 || !/^GIF8[79]a$/.test(bytes.toString('ascii', 0, 6))) return null;
  assertEnabled('gif');
  const width = bytes.readUInt16LE(6);
  const height = bytes.readUInt16LE(8);
  return width > 0 && height > 0 ? { width, height, type: 'gif' } : fail('invalid GIF dimensions');
}

function jpegSize(bytes) {
  if (bytes.length < 4 || bytes[0] !== 0xff || bytes[1] !== 0xd8) return null;
  assertEnabled('jpg');
  let offset = 2;
  let iterations = 0;
  while (offset + 4 <= bytes.length && iterations++ < 65536) {
    while (offset < bytes.length && bytes[offset] === 0xff) offset += 1;
    if (offset >= bytes.length) break;
    const marker = bytes[offset++];
    if (marker === 0xd8 || marker === 0xd9 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue;
    if (offset + 2 > bytes.length) break;
    const segmentLength = bytes.readUInt16BE(offset);
    if (segmentLength < 2 || offset + segmentLength > bytes.length) break;
    const isStartOfFrame = (marker >= 0xc0 && marker <= 0xc3)
      || (marker >= 0xc5 && marker <= 0xc7)
      || (marker >= 0xc9 && marker <= 0xcb)
      || (marker >= 0xcd && marker <= 0xcf);
    if (isStartOfFrame) {
      if (segmentLength < 7) break;
      const height = bytes.readUInt16BE(offset + 3);
      const width = bytes.readUInt16BE(offset + 5);
      return width > 0 && height > 0 ? { width, height, type: 'jpg' } : fail('invalid JPEG dimensions');
    }
    offset += segmentLength;
  }
  fail('invalid or unsupported JPEG');
}

function svgSize(bytes) {
  const prefix = bytes.subarray(0, Math.min(bytes.length, 1024 * 1024)).toString('utf8');
  if (!/<svg(?:\s|>)/i.test(prefix)) return null;
  assertEnabled('svg');
  const openTag = prefix.match(/<svg\b[^>]*>/i)?.[0];
  if (!openTag) fail('invalid SVG');
  const widthMatch = openTag.match(/\bwidth\s*=\s*["']\s*([0-9]+(?:\.[0-9]+)?)/i);
  const heightMatch = openTag.match(/\bheight\s*=\s*["']\s*([0-9]+(?:\.[0-9]+)?)/i);
  let width = widthMatch ? Number(widthMatch[1]) : 0;
  let height = heightMatch ? Number(heightMatch[1]) : 0;
  if (!(width > 0 && height > 0)) {
    const viewBox = openTag.match(/\bviewBox\s*=\s*["']\s*[-+0-9.eE]+[ ,]+[-+0-9.eE]+[ ,]+([-+0-9.eE]+)[ ,]+([-+0-9.eE]+)/i);
    width = viewBox ? Number(viewBox[1]) : 0;
    height = viewBox ? Number(viewBox[2]) : 0;
  }
  return width > 0 && height > 0 && Number.isFinite(width) && Number.isFinite(height)
    ? { width, height, type: 'svg' }
    : fail('SVG requires finite positive width/height or viewBox dimensions');
}

function imageSize(input, callback) {
  if (typeof callback === 'function') {
    queueMicrotask(() => {
      try { callback(null, imageSize(input)); } catch (error) { callback(error); }
    });
    return undefined;
  }
  const bytes = boundedBytes(input);
  return pngSize(bytes) || gifSize(bytes) || jpegSize(bytes) || svgSize(bytes)
    || fail('unsupported image type; Nexa permits only PNG, JPEG, GIF, and SVG');
}

imageSize.imageSize = imageSize;
imageSize.default = imageSize;
imageSize.disableFS = (disabled) => { filesystemEnabled = !disabled; };
imageSize.disableTypes = (types) => { disabledTypes = new Set(types); };
imageSize.setConcurrency = () => {};
imageSize.types = ['png', 'jpg', 'gif', 'svg'];

module.exports = imageSize;
