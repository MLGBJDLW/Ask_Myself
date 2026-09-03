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

function separatorBeforeSpeech(draft: string): string {
  return draft.length > 0 && !/\s$/u.test(draft) ? ' ' : '';
}

function appendSpeech(draft: string, text: string): string {
  if (!text) return draft;
  const separator = /^\s/u.test(text) ? '' : separatorBeforeSpeech(draft);
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

function safeProviderExtension(previous: string, next: string): string {
  if (next.startsWith(previous)) return next.slice(previous.length);

  // A snapshot provider may revise earlier words while adding a new tail. Once
  // the user owns the draft, use only a sufficiently long trailing anchor from
  // the previous provider snapshot; never copy the revised prefix back over the
  // user's correction.
  const previousCharacters = Array.from(previous);
  for (let start = 0; start <= previousCharacters.length - 3; start += 1) {
    const suffix = previousCharacters.slice(start).join('');
    const anchor = next.lastIndexOf(suffix);
    if (anchor >= 0) return next.slice(anchor + suffix.length);
  }
  return '';
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
    nextDraft = appendSpeech(draft, safeProviderExtension(session.providerText, providerText));
    spanEnd = nextDraft.length;
  }

  return {
    draft: nextDraft,
    session: final ? null : {
      providerText,
      spanStart: userOwned ? draft.length : session.spanStart,
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
