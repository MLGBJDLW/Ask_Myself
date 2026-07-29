export type SpeechPlaybackErrorCode =
  | 'asset_access'
  | 'decode'
  | 'unsupported_format'
  | 'autoplay_blocked'
  | 'provider'
  | 'playback';

export interface SpeechCacheIdentity {
  provider: string;
  model: string;
  voice: string;
  speed: number;
  outputFormat: string;
}

export function playableMediaType(mediaType: string, canPlayResult: CanPlayTypeResult): boolean {
  return mediaType.trim().toLowerCase().startsWith('audio/') && canPlayResult !== '';
}

export function classifyMediaError(code: number | undefined): SpeechPlaybackErrorCode {
  switch (code) {
    case 2: return 'asset_access';
    case 3: return 'decode';
    case 4: return 'unsupported_format';
    default: return 'playback';
  }
}

export function speechCacheKeyMaterial(identity: SpeechCacheIdentity, text: string): string {
  const normalizedText = text.trim().replace(/\s+/g, ' ');
  return [identity.provider, identity.model, identity.voice, String(identity.speed), identity.outputFormat, normalizedText].join('\0');
}

export function mediaErrorMessage(code: SpeechPlaybackErrorCode): string {
  switch (code) {
    case 'asset_access': return 'Nexa could not access the managed local audio file.';
    case 'decode': return 'The generated audio file is empty, damaged, or cannot be decoded.';
    case 'unsupported_format': return 'This system does not support the generated audio format.';
    case 'autoplay_blocked': return 'Playback was blocked. Press play again to allow audio.';
    case 'provider': return 'The speech provider could not generate this reply.';
    default: return 'The reply could not be played.';
  }
}
