import type { Source } from '../types';

const MEDIA_GLOB_EXTENSION = /(?:\.|[,{])(?:mp4|mkv|webm|avi|mov|flv|mpeg|mpg|wmv|m4v|mp3|wav|flac|aac|ogg|wma|m4a|opus)(?:[,}\]]|$)/i;
const EXPLICIT_GLOB_EXTENSION = /\.[a-z0-9{][^/\\]*$/i;

/**
 * Whether a source filter explicitly admits media. An empty include list is
 * intentionally treated as unknown: scans still degrade safely, but the
 * Sources page does not show a speculative runtime warning for an empty tree.
 */
export function sourceMayIncludeMedia(source: Source): boolean {
  return source.includeGlobs.some((pattern) => (
    MEDIA_GLOB_EXTENSION.test(pattern) || !EXPLICIT_GLOB_EXTENSION.test(pattern)
  ));
}
