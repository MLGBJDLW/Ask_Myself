import assert from 'node:assert/strict';
import { test } from 'node:test';
import imageSize from './index.cjs';

test('reads bounded reviewed formats', () => {
  const png = Buffer.alloc(24);
  Buffer.from('89504e470d0a1a0a', 'hex').copy(png);
  png.writeUInt32BE(640, 16);
  png.writeUInt32BE(480, 20);
  assert.deepEqual(imageSize(png), { width: 640, height: 480, type: 'png' });

  assert.deepEqual(
    imageSize(Buffer.from('<svg viewBox="0 0 16 9"></svg>')),
    { width: 16, height: 9, type: 'svg' },
  );
});

test('rejects vulnerable container families and zero-sized loops immediately', () => {
  const malformedIcns = Buffer.concat([
    Buffer.from('icns'),
    Buffer.from([0, 0, 0, 16]),
    Buffer.from('ic10'),
    Buffer.from([0, 0, 0, 0]),
  ]);
  const malformedBox = Buffer.from([0, 0, 0, 0, 0x6a, 0x78, 0x6c, 0x70]);
  assert.throws(() => imageSize(malformedIcns), /unsupported image type/);
  assert.throws(() => imageSize(malformedBox), /unsupported image type/);
});

test('enforces a hard inspection budget', () => {
  assert.throws(() => imageSize(Buffer.alloc(4 * 1024 * 1024 + 1)), /4 MiB/);
});
