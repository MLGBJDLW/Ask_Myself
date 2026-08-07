export const MAX_VOICE_PARTIAL_CHARS = 4_096;

export function replaceBoundedVoicePartial(text: string): string {
  return text.length <= MAX_VOICE_PARTIAL_CHARS
    ? text
    : text.slice(-MAX_VOICE_PARTIAL_CHARS);
}

export function appendBoundedVoicePartial(current: string, delta: string): string {
  if (delta.length >= MAX_VOICE_PARTIAL_CHARS) {
    return delta.slice(-MAX_VOICE_PARTIAL_CHARS);
  }
  const retainedCurrentChars = MAX_VOICE_PARTIAL_CHARS - delta.length;
  return current.slice(-retainedCurrentChars) + delta;
}
