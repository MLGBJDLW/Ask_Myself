function messageFromError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export function isTauriBridgeUnavailable(error: unknown): boolean {
  const message = messageFromError(error).toLowerCase();
  return (
    message.includes("reading 'invoke'") ||
    message.includes('reading "invoke"') ||
    message.includes('__tauri__') ||
    (message.includes('tauri') && message.includes('invoke'))
  );
}

export function formatUserError(action: string, error: unknown): string {
  if (isTauriBridgeUnavailable(error)) {
    return `${action}. Desktop features are unavailable in this browser preview.`;
  }

  const cleaned = messageFromError(error)
    .replace(/^Error:\s*/i, '')
    .replace(/^TypeError:\s*/i, '')
    .trim();

  return cleaned ? `${action}: ${cleaned}` : action;
}
