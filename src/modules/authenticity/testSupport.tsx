import type {
  BranchPublication, CertificationConfig, CertificationRecord, DecodeResult, PreviewImage, PublicationPreview,
} from "./types";

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

export class ResizeObserverMock {
  observe() {}
  disconnect() {}
}

export class PreloadedImageMock {
  decoding = "auto";
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;

  set src(_value: string) {
    queueMicrotask(() => this.onload?.());
  }

  decode() {
    return Promise.resolve();
  }
}

export const previewImage: PreviewImage = {
  dataUrl: "data:image/png;base64,cHJldmlldw==",
  width: 1200,
  height: 800,
  sourceBytes: 7,
};

export const publicationPreview: PublicationPreview = {
  branchId: "branch-1",
  image: {
    dataUrl: "data:image/jpeg;base64,cHJldmlldw==",
    width: 1000,
    height: 800,
    sourceBytes: 7,
  },
  originalImage: {
    dataUrl: "data:image/png;base64,b3JpZ2luYWw=",
    width: 1000,
    height: 800,
    sourceBytes: 8,
  },
  sourceWidth: 4000,
  sourceHeight: 3200,
  outputBytes: 7,
  watermarkId: null,
  cacheToken: "cache-token",
  cacheHit: false,
  renderMs: 12,
  encodeMs: 8,
};

export const config = (branchId: string): CertificationConfig => ({
  branchId,
  title: branchId,
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  trustmarkEnabled: false,
  certificatePath: "C:/certificate.pem",
  signingAlgorithm: "es256",
  timestampUrl: null,
  jpegQuality: 90,
  backgroundColor: "#ffffff",
  watermarkStrength: 1,
  additionalRegions: [],
  updatedMs: 0,
});

export const publication = (branchId: string): BranchPublication => ({
  branchId,
  artifact: null,
  config: config(branchId),
  records: [],
  modelsReady: false,
  modelVariant: "Q/BCH_SUPER",
  encoderSha256: null,
  decoderSha256: null,
});

export const publishedBranch = (branchId: string): BranchPublication => ({
  ...publication(branchId),
  artifact: {
    id: `artifact-${branchId}`,
    branchId,
    historyId: `head-${branchId}`,
    sourcePath: `artworks/${branchId}.png`,
    sourceSha256: "a".repeat(64),
    mediaType: "image/png",
    byteSize: 7,
    createdMs: 1,
  },
  config: {
    ...config(branchId),
    trustmarkEnabled: true,
    additionalRegions: [{ x: 0.1, y: 0.1, width: 0.5, height: 0.5 }],
  },
  modelsReady: true,
});

export const record = (branchId: string): CertificationRecord => ({
  id: "record-1",
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branchId,
  branchTitle: branchId,
  historyId: `head-${branchId}`,
  watermarkId: "1".repeat(40),
  trustmarkEnabled: true,
  outputPath: "C:/output.jpg",
  outputSha256: "b".repeat(64),
  outputBytes: 7,
  title: branchId,
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  additionalRegions: [{ x: 0.1, y: 0.1, width: 0.5, height: 0.5 }],
  c2paManifestLabel: "manifest",
  c2paManifestJson: "{}",
  validationState: "Valid",
  createdMs: 1,
});

export const outputPreview = (watermarkId: string): PublicationPreview => ({
  ...publicationPreview,
  branchId: "first",
  sourceWidth: previewImage.width,
  sourceHeight: previewImage.height,
  outputBytes: previewImage.sourceBytes,
  watermarkId,
});

export const decodeResult = (branchId: string): DecodeResult => ({
  watermarkPresent: true,
  watermarkId: "1".repeat(40),
  decodedRegion: null,
  c2paPresent: true,
  c2paValidationState: "Valid",
  c2paValidationStatus: [],
  c2paRecordId: "record-1",
  c2paWatermarkId: "1".repeat(40),
  identifiersMatch: true,
  title: branchId,
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  manifestJson: "{}",
  matches: [{ record: record(branchId), evidenceSources: ["c2pa", "trustmark"] }],
});

export const branches = [
  { id: "first", title: "First", headHistoryId: "head-first" },
  { id: "second", title: "Second", headHistoryId: "head-second" },
];

export const brokenPreviewPublication: BranchPublication = {
  branchId: "branch-1",
  artifact: {
    id: "artifact-1",
    branchId: "branch-1",
    historyId: "history-1",
    sourcePath: "artworks/final.png",
    sourceSha256: "a".repeat(64),
    mediaType: "image/png",
    byteSize: 7,
    createdMs: 1,
  },
  config: {
    branchId: "branch-1",
    title: "Artwork",
    creator: "Creator",
    rightsStatement: "",
    authenticationContent: "",
    trustmarkEnabled: false,
    certificatePath: "",
    signingAlgorithm: "es256",
    timestampUrl: null,
    jpegQuality: 90,
    backgroundColor: "#ffffff",
    watermarkStrength: 1,
    additionalRegions: [],
    updatedMs: 0,
  },
  records: [],
  modelsReady: false,
  modelVariant: "Q/BCH_SUPER",
  encoderSha256: null,
  decoderSha256: null,
};
