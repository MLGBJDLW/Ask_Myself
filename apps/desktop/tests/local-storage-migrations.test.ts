import {
  CURRENT_LOCAL_STORAGE_MIGRATION_VERSION,
  runLocalStorageMigrations,
} from '../src/lib/localStorageMigrations';

class MemoryStorage implements Storage {
  private readonly values = new Map<string, string>();

  get length(): number { return this.values.size; }
  clear(): void { this.values.clear(); }
  getItem(key: string): string | null { return this.values.get(key) ?? null; }
  key(index: number): string | null { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string): void { this.values.delete(key); }
  setItem(key: string, value: string): void { this.values.set(key, value); }
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const storage = new MemoryStorage();
storage.setItem('ask-myself-theme', 'midnight');
storage.setItem('ask-myself-locale', 'zh-CN');
storage.setItem('chat-token-usage-v1', '{"stale":true}');

runLocalStorageMigrations(CURRENT_LOCAL_STORAGE_MIGRATION_VERSION, storage);

assert(storage.getItem('nexa-theme') === 'midnight', 'theme should migrate');
assert(storage.getItem('nexa-locale') === 'zh-CN', 'locale should migrate');
assert(storage.getItem('ask-myself-theme') === null, 'legacy theme should be removed');
assert(storage.getItem('ask-myself-locale') === null, 'legacy locale should be removed');
assert(storage.getItem('chat-token-usage-v1') === null, 'legacy usage cache should be removed');
assert(
  storage.getItem('nexa-local-migration-version') === String(CURRENT_LOCAL_STORAGE_MIGRATION_VERSION),
  'applied migration version should persist',
);

storage.setItem('nexa-theme', 'aurora');
runLocalStorageMigrations(CURRENT_LOCAL_STORAGE_MIGRATION_VERSION, storage);
assert(storage.getItem('nexa-theme') === 'aurora', 'completed migrations should be idempotent');

console.log('ok - local storage migrations are versioned and idempotent');
