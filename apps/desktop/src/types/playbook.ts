export interface PlaybookCitation {
  id: string;
  playbookId: string;
  chunkId: string;
  annotation: string;
  order: number;
}

export interface Playbook {
  id: string;
  title: string;
  description: string;
  queryText?: string | null;
  citations: PlaybookCitation[];
  createdAt: string;
  updatedAt: string;
}
