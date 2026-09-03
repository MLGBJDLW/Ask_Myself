export type VoiceDictationEvent =
  | { kind: 'start' }
  | { kind: 'interim'; text: string }
  | { kind: 'final'; text: string }
  | { kind: 'cancel' }
  | { kind: 'end' };

export interface VoiceDraftSession {
  providerText: string;
  spanStart: number;
  spanEnd: number;
  projectedDraft: string;
  userOwned: boolean;
}

export interface VoiceDraftProjection {
  draft: string;
  session: VoiceDraftSession | null;
}

const UNSPACED_SCRIPT_CHARACTER = /[\p{Script_Extensions=Han}\p{Script_Extensions=Hiragana}\p{Script_Extensions=Katakana}\p{Script_Extensions=Hangul}\p{Script_Extensions=Thai}\p{Script_Extensions=Lao}\p{Script_Extensions=Khmer}\p{Script_Extensions=Myanmar}]/u;

function separatorBeforeSpeech(draft: string): string {
  return draft.length > 0 && !/\s$/u.test(draft) ? ' ' : '';
}

function joinsUnspacedScript(draft: string, extension: string): boolean {
  const draftCharacters = Array.from(draft);
  const previous = draftCharacters[draftCharacters.length - 1] ?? '';
  const next = Array.from(extension)[0] ?? '';
  return UNSPACED_SCRIPT_CHARACTER.test(previous) && UNSPACED_SCRIPT_CHARACTER.test(next);
}

function appendSpeech(draft: string, text: string): string {
  if (!text) return draft;
  const separator = /^\s/u.test(text) || joinsUnspacedScript(draft, text)
    ? ''
    : separatorBeforeSpeech(draft);
  return `${draft}${separator}${text}`;
}

export function startVoiceDraftSession(draft: string): VoiceDraftSession {
  return {
    providerText: '',
    spanStart: draft.length,
    spanEnd: draft.length,
    projectedDraft: draft,
    userOwned: false,
  };
}

const MIN_SAFE_ANCHOR_CHARACTERS = 3;
const MAX_SAFE_ANCHOR_CHARACTERS = 256;

function extensionAfterTrailingAnchor(source: string, next: string): string | null {
  const sourceCharacters = Array.from(source);
  const earliestStart = Math.max(0, sourceCharacters.length - MAX_SAFE_ANCHOR_CHARACTERS);
  for (
    let start = earliestStart;
    start <= sourceCharacters.length - MIN_SAFE_ANCHOR_CHARACTERS;
    start += 1
  ) {
    const suffix = sourceCharacters.slice(start).join('');
    const anchor = next.lastIndexOf(suffix);
    if (anchor >= 0) return next.slice(anchor + suffix.length);
  }
  return null;
}

function joinsInsideWord(draft: string, extension: string): boolean {
  const draftCharacters = Array.from(draft);
  const previous = draftCharacters[draftCharacters.length - 1] ?? '';
  const next = Array.from(extension)[0] ?? '';
  return /[\p{L}\p{M}\p{N}]/u.test(previous)
    && /[\p{L}\p{M}\p{N}]/u.test(next);
}

function joinsInsideDelimitedWord(draft: string, extension: string): boolean {
  // Latin-like scripts need the fragment guard (`light` -> `lights`) because
  // a suffix can be part of a corrected word. CJK and other scripts that
  // commonly continue without spaces have no such boundary; once a trailing
  // user-owned anchor proves the extension, retaining it is the safe choice.
  return joinsInsideWord(draft, extension) && !joinsUnspacedScript(draft, extension);
}

function safeProviderExtension(
  previousProviderText: string,
  nextProviderText: string,
  userDraft: string,
  voiceSpanStart: number,
): string {
  // Prefer an anchor in the text the user actually owns. This handles edits at
  // the end of a provider snapshot ("light" -> "lights") without appending the
  // provider's old suffix a second time.
  const userVoiceTail = userDraft.slice(Math.min(voiceSpanStart, userDraft.length));
  const userAligned = extensionAfterTrailingAnchor(userVoiceTail, nextProviderText);
  if (userAligned !== null && !joinsInsideDelimitedWord(userDraft, userAligned)) return userAligned;

  // If a snapshot provider revised an earlier word, a trailing anchor in the
  // prior snapshot may still prove a genuinely new tail. Refuse an alphanumeric
  // fragment because it could be the remainder of a word the user corrected.
  const providerAligned = extensionAfterTrailingAnchor(previousProviderText, nextProviderText);
  if (providerAligned === null || joinsInsideWord(userDraft, providerAligned)) return '';
  return providerAligned;
}

/**
 * Project one cumulative provider transcript into the composer.
 *
 * Until the user edits the projected draft, the provider owns one replaceable
 * tail span and may correct interim text. Once the draft diverges, ownership
 * transfers to the user permanently; later provider updates may append only a
 * proven extension and can never overwrite the correction.
 */
export function projectVoiceTranscript(
  draft: string,
  session: VoiceDraftSession | null,
  providerText: string,
  final: boolean,
): VoiceDraftProjection {
  if (!session) {
    const nextDraft = appendSpeech(draft, providerText);
    return {
      draft: nextDraft,
      session: final ? null : {
        ...startVoiceDraftSession(draft),
        providerText,
        spanEnd: nextDraft.length,
        projectedDraft: nextDraft,
      },
    };
  }

  const userOwned = session.userOwned || draft !== session.projectedDraft;
  let nextDraft: string;
  let spanEnd = session.spanEnd;
  if (!userOwned) {
    const replacement = providerText
      ? `${separatorBeforeSpeech(draft.slice(0, session.spanStart))}${providerText}`
      : '';
    nextDraft = `${draft.slice(0, session.spanStart)}${replacement}${draft.slice(session.spanEnd)}`;
    spanEnd = session.spanStart + replacement.length;
  } else {
    nextDraft = appendSpeech(
      draft,
      safeProviderExtension(session.providerText, providerText, draft, session.spanStart),
    );
    spanEnd = nextDraft.length;
  }

  return {
    draft: nextDraft,
    session: final ? null : {
      providerText,
      spanStart: session.spanStart,
      spanEnd,
      projectedDraft: nextDraft,
      userOwned,
    },
  };
}

export function cancelVoiceDraft(
  draft: string,
  session: VoiceDraftSession | null,
): VoiceDraftProjection {
  if (!session) return { draft, session: null };
  if (session.userOwned || draft !== session.projectedDraft) {
    return { draft, session: null };
  }
  return {
    draft: `${draft.slice(0, session.spanStart)}${draft.slice(session.spanEnd)}`,
    session: null,
  };
}

export function applyVoiceDictationEvent(
  draft: string,
  session: VoiceDraftSession | null,
  event: VoiceDictationEvent,
): VoiceDraftProjection {
  switch (event.kind) {
    case 'start':
      return { draft, session: startVoiceDraftSession(draft) };
    case 'interim':
      return projectVoiceTranscript(draft, session, event.text, false);
    case 'final':
      return projectVoiceTranscript(draft, session, event.text, true);
    case 'cancel':
      return cancelVoiceDraft(draft, session);
    case 'end':
      return { draft, session: null };
  }
}
