export interface NormalizedRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PreviewImage {
  dataUrl: string;
  width: number;
  height: number;
  sourceBytes: number;
}

export interface CertificationConfig {
  branchId: string;
  title: string;
  creator: string;
  rightsStatement: string;
  authenticationContent: string;
  trustmarkEnabled: boolean;
  certificatePath: string;
  signingAlgorithm: string;
  timestampUrl: string | null;
  jpegQuality: number;
  backgroundColor: string;
  watermarkStrength: number;
  additionalRegions: NormalizedRegion[];
  updatedMs: number;
}

export interface FinalArtifact {
  id: string;
  branchId: string;
  historyId: string;
  sourcePath: string;
  sourceSha256: string;
  mediaType: string;
  byteSize: number;
  createdMs: number;
}

export interface CertificationRecord {
  id: string;
  artworkId: string;
  artworkTitle: string;
  branchId: string;
  branchTitle: string;
  historyId: string;
  watermarkId: string | null;
  trustmarkEnabled: boolean;
  outputPath: string;
  outputSha256: string;
  outputBytes: number;
  title: string;
  creator: string;
  rightsStatement: string;
  authenticationContent: string;
  additionalRegions: NormalizedRegion[];
  c2paManifestLabel: string | null;
  c2paManifestJson: string | null;
  validationState: string | null;
  createdMs: number;
}

export interface BranchPublication {
  branchId: string;
  artifact: FinalArtifact | null;
  config: CertificationConfig;
  records: CertificationRecord[];
  modelsReady: boolean;
}

export interface PublishBranchRequest {
  branchId: string;
  outputPath: string;
  privateKeyPem: string;
  config: CertificationConfig;
  watermarkId: string | null;
}

export interface PublishResult {
  record: CertificationRecord;
  width: number;
  height: number;
  watermarkRegionCount: number;
}

export interface ValidationItem {
  code: string;
  explanation: string;
}

export interface DecodeResult {
  watermarkPresent: boolean;
  watermarkId: string | null;
  decodedRegion: NormalizedRegion | null;
  c2paPresent: boolean;
  c2paValidationState: string | null;
  c2paValidationStatus: ValidationItem[];
  c2paWatermarkId: string | null;
  identifiersMatch: boolean | null;
  title: string | null;
  creator: string | null;
  rightsStatement: string | null;
  authenticationContent: string | null;
  manifestJson: string | null;
  matches: CertificationRecord[];
}
