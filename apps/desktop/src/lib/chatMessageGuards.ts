import type { ConversationMessage } from '../types/conversation';

function artifactKind(message: ConversationMessage): string | null {
  const artifacts = message.artifacts;
  if (!artifacts || Array.isArray(artifacts) || typeof artifacts !== 'object') {
    return null;
  }
  const kind = (artifacts as Record<string, unknown>).kind;
  return typeof kind === 'string' ? kind : null;
}

export function isSteeringMessage(message: ConversationMessage): boolean {
  return message.role === 'user'
    && (message.id.startsWith('temp-steer-') || artifactKind(message) === 'steering');
}

export function isOptimisticSteeringMessage(message: ConversationMessage): boolean {
  return message.id.startsWith('temp-steer-') && isSteeringMessage(message);
}

export function isCompactionSummaryMessage(message: ConversationMessage): boolean {
  if (message.role !== 'system') return false;
  const lower = message.content.toLowerCase();
  return (
    lower.includes('earlier conversation context') ||
    lower.includes('auto-compacted') ||
    lower.includes('compacted context')
  );
}
