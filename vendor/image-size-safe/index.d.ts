export interface ImageDimensions {
  width: number;
  height: number;
  type: 'png' | 'jpg' | 'gif' | 'svg';
}

export declare function imageSize(input: Uint8Array | string): ImageDimensions;
export declare function imageSize(
  input: Uint8Array | string,
  callback: (error: Error | null, dimensions?: ImageDimensions) => void,
): void;
export declare function disableFS(disabled: boolean): void;
export declare function disableTypes(types: string[]): void;
export declare function setConcurrency(concurrency: number): void;
export declare const types: string[];
export default imageSize;
