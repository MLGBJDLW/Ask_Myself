/** Stable, non-secret cache partition for account-scoped provider metadata. */
export function credentialFingerprint(apiKey: string): string {
  const credential = apiKey.trim();
  if (!credential) return 'anonymous';
  let hash = 0x811c9dc5;
  let secondary = 0x9e3779b9;
  for (let index = 0; index < credential.length; index += 1) {
    const code = credential.charCodeAt(index);
    hash ^= code;
    hash = Math.imul(hash, 0x01000193);
    secondary = Math.imul(secondary ^ code, 0x85ebca6b);
    secondary ^= secondary >>> 13;
  }
  return `${credential.length}-${(hash >>> 0).toString(16).padStart(8, '0')}${(secondary >>> 0).toString(16).padStart(8, '0')}`;
}
