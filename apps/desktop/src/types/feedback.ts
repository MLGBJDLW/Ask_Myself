export interface Feedback {
  id: string;
  chunkId: string;
  queryText: string;
  action: 'upvote' | 'downvote' | 'pin';
  createdAt: string;
}
