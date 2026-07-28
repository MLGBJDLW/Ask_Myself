export type LocalModelId = 'MultilingualMiniLM' | 'MultilingualE5Base' | 'Qwen3Embedding06B';

export interface EmbedderConfig {
  provider: 'local' | 'api' | 'tfidf';
  apiKey: string;
  apiBaseUrl: string;
  apiModel: string;
  modelPath: string;
  vectorDimensions: number;
  /** Which local ONNX model to use. Default: `"MultilingualMiniLM"`. */
  localModel: LocalModelId;
}
