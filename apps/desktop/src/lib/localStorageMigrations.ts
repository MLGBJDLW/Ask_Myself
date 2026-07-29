const MIGRATION_VERSION_KEY = 'nexa-local-migration-version';
export const CURRENT_LOCAL_STORAGE_MIGRATION_VERSION = 1;

interface LocalStorageMigration {
  version: number;
  run(storage: Storage): void;
}

const migrations: LocalStorageMigration[] = [
  {
    version: 1,
    run(storage) {
      migrateKey(storage, 'ask-myself-theme', 'nexa-theme');
      migrateKey(storage, 'ask-myself-locale', 'nexa-locale');
      storage.removeItem('chat-token-usage-v1');
    },
  },
];

export function runLocalStorageMigrations(
  currentVersion = CURRENT_LOCAL_STORAGE_MIGRATION_VERSION,
  storage: Storage | undefined = typeof window === 'undefined' ? undefined : window.localStorage,
): void {
  if (!storage) return;
  const storedVersion = parseMigrationVersion(storage.getItem(MIGRATION_VERSION_KEY));
  for (const migration of migrations) {
    if (migration.version > storedVersion && migration.version <= currentVersion) {
      migration.run(storage);
      storage.setItem(MIGRATION_VERSION_KEY, String(migration.version));
    }
  }
}

function migrateKey(storage: Storage, legacyKey: string, currentKey: string): void {
  const legacyValue = storage.getItem(legacyKey);
  if (legacyValue !== null && storage.getItem(currentKey) === null) {
    storage.setItem(currentKey, legacyValue);
  }
  storage.removeItem(legacyKey);
}

function parseMigrationVersion(value: string | null): number {
  if (value === null) return 0;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
}
