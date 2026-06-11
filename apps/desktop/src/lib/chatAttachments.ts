const ALLOWED_MIME_TYPES = new Set([
  "image/jpeg",
  "image/png",
  "image/gif",
  "image/webp",
  "application/pdf",
  "text/plain",
  "text/markdown",
  "text/x-markdown",
  "text/csv",
  "application/json",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  "application/msword",
  "application/vnd.ms-excel",
  "application/vnd.ms-powerpoint",
]);

const ALLOWED_EXTENSIONS = new Set([
  ".jpg",
  ".jpeg",
  ".png",
  ".gif",
  ".webp",
  ".pdf",
  ".txt",
  ".md",
  ".csv",
  ".json",
  ".docx",
  ".xlsx",
  ".pptx",
  ".doc",
  ".xls",
  ".ppt",
]);

const MIME_BY_EXTENSION = new Map<string, string>([
  [".jpg", "image/jpeg"],
  [".jpeg", "image/jpeg"],
  [".png", "image/png"],
  [".gif", "image/gif"],
  [".webp", "image/webp"],
  [".pdf", "application/pdf"],
  [".txt", "text/plain"],
  [".md", "text/markdown"],
  [".csv", "text/csv"],
  [".json", "application/json"],
  [".docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
  [".xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
  [".pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"],
  [".doc", "application/msword"],
  [".xls", "application/vnd.ms-excel"],
  [".ppt", "application/vnd.ms-powerpoint"],
]);

const EXTENSION_BY_IMAGE_MIME = new Map<string, string>([
  ["image/jpeg", "jpg"],
  ["image/png", "png"],
  ["image/gif", "gif"],
  ["image/webp", "webp"],
]);

export interface PastedImageFile {
  file: Blob;
  name: string;
}

export interface PastedImageDataUrl {
  dataUrl: string;
  name: string;
}

function getFileExtension(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot).toLowerCase() : "";
}

export function getAllowedAttachmentMediaType(mediaType: string | undefined, name: string): string | null {
  const normalized = (mediaType ?? "").trim().toLowerCase();
  if (normalized && ALLOWED_MIME_TYPES.has(normalized)) {
    return normalized;
  }

  const ext = getFileExtension(name);
  if (!ALLOWED_EXTENSIONS.has(ext)) {
    return null;
  }

  return MIME_BY_EXTENSION.get(ext) ?? "application/octet-stream";
}

function isAllowedImage(mediaType: string | undefined, name: string): boolean {
  return getAllowedAttachmentMediaType(mediaType, name)?.startsWith("image/") ?? false;
}

function pastedImageName(mediaType: string | undefined, name: string | undefined, now: () => number): string {
  const trimmedName = name?.trim();
  if (trimmedName) return trimmedName;
  const normalized = (mediaType ?? "").trim().toLowerCase();
  const ext = EXTENSION_BY_IMAGE_MIME.get(normalized) ?? "png";
  return `pasted-image-${now()}.${ext}`;
}

export function collectPastedImageFiles(
  clipboardData: Pick<DataTransfer, "files" | "items">,
  now: () => number = Date.now,
): PastedImageFile[] {
  const imageFiles: PastedImageFile[] = [];

  if (clipboardData.files.length > 0) {
    for (const file of Array.from(clipboardData.files)) {
      const name = pastedImageName(file.type, file.name, now);
      if (!isAllowedImage(file.type, name)) continue;
      imageFiles.push({ file, name });
    }
  }

  if (imageFiles.length > 0 || !clipboardData.items) {
    return imageFiles;
  }

  for (const item of Array.from(clipboardData.items)) {
    if (item.kind && item.kind !== "file") continue;
    const file = item.getAsFile();
    if (!file) continue;
    const mediaType = item.type || file.type;
    const name = pastedImageName(mediaType, file.name, now);
    if (!isAllowedImage(mediaType, name)) continue;
    imageFiles.push({ file, name });
  }

  return imageFiles;
}

export function getPastedImageDataUrl(
  clipboardData: Pick<DataTransfer, "getData">,
  now: () => number = Date.now,
): PastedImageDataUrl | null {
  const html = clipboardData.getData("text/html") ?? "";
  const htmlDataUrl = html.match(/src=["'](data:image\/[^"']+)["']/i)?.[1];
  const plainText = clipboardData.getData("text/plain")?.trim() ?? "";
  const plainDataUrl = /^data:image\/[^;]+;base64,/i.test(plainText) ? plainText : null;
  const dataUrl = htmlDataUrl ?? plainDataUrl;
  if (!dataUrl) return null;

  const mediaType = dataUrl.match(/^data:([^;]+);base64,/i)?.[1];
  const ext = EXTENSION_BY_IMAGE_MIME.get((mediaType ?? "").toLowerCase()) ?? "png";
  return {
    dataUrl,
    name: `pasted-image-${now()}.${ext}`,
  };
}
