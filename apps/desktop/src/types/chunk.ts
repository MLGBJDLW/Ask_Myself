export interface Chunk {
  id: string;
  documentId: string;
  content: string;
  chunkIndex: number;
  startByte: number;
  endByte: number;
  headingPath: string[];
  createdAt: string;
}
