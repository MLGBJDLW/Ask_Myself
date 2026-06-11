import {
  collectPastedImageFiles,
  getAllowedAttachmentMediaType,
  getPastedImageDataUrl,
} from '../src/lib/chatAttachments';

type TestFn = () => void | Promise<void>;

const tests: Array<{ name: string; fn: TestFn }> = [];

function test(name: string, fn: TestFn): void {
  tests.push({ name, fn });
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function file(name: string, type: string): File {
  return { name, type } as File;
}

function clipboard(overrides: Partial<Pick<DataTransfer, 'files' | 'items' | 'getData'>>): Pick<DataTransfer, 'files' | 'items' | 'getData'> {
  return {
    files: [] as unknown as FileList,
    items: [] as unknown as DataTransferItemList,
    getData: () => '',
    ...overrides,
  };
}

test('allows image attachments by extension when clipboard file has no MIME type', () => {
  const candidates = collectPastedImageFiles(
    clipboard({ files: [file('screenshot.png', '')] as unknown as FileList }),
    () => 123,
  );

  assertEqual(candidates.length, 1, 'candidate count');
  assertEqual(candidates[0].name, 'screenshot.png', 'candidate name');
});

test('allows image clipboard items when item MIME is missing but file extension is supported', () => {
  const candidates = collectPastedImageFiles(
    clipboard({
      items: [{
        kind: 'file',
        type: '',
        getAsFile: () => file('capture.webp', ''),
      }] as unknown as DataTransferItemList,
    }),
    () => 123,
  );

  assertEqual(candidates.length, 1, 'candidate count');
  assertEqual(candidates[0].name, 'capture.webp', 'candidate name');
});

test('extracts pasted plain data URL images', () => {
  const pasted = getPastedImageDataUrl(
    clipboard({
      getData: (format) => format === 'text/plain' ? 'data:image/png;base64,abc123' : '',
    }),
    () => 456,
  );

  assert(pasted, 'data URL image should be detected');
  assertEqual(pasted.name, 'pasted-image-456.png', 'generated name');
  assertEqual(pasted.dataUrl, 'data:image/png;base64,abc123', 'data URL');
});

test('keeps non-image files out of pasted image candidates', () => {
  const candidates = collectPastedImageFiles(
    clipboard({ files: [file('notes.txt', '')] as unknown as FileList }),
    () => 123,
  );

  assertEqual(candidates.length, 0, 'candidate count');
  assertEqual(getAllowedAttachmentMediaType('', 'notes.txt'), 'text/plain', 'text attachment type');
});

async function run(): Promise<void> {
  for (const { name, fn } of tests) {
    try {
      await fn();
      console.log(`ok - ${name}`);
    } catch (error) {
      console.error(`not ok - ${name}`);
      throw error;
    }
  }
}

void run();
