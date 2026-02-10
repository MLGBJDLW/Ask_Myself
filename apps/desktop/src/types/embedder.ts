export interface EmbedderConfig {
  provider: 'local' | 'api' | 'tfidf';
  apiKey: string;
  apiBaseUrl: string;
  apiModel: string;
  modelPath: string;
  vectorDimensions: number;
}
