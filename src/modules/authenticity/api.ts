import { invokeCommand } from "../../shared/tauri";
import type { CleanupReport } from "../../shared/fileCleanup";
import type {
  BranchPublication,
  CertificationRecord,
  DecodeResult,
  NormalizedRegion,
  PreviewImage,
  PublicationPreview,
  PublicationPreviewRequest,
  PublishBranchRequest,
  PublishResult,
} from "./types";

export const authenticityApi = {
  getPublication: (branchId: string) =>
    invokeCommand<BranchPublication>("get_branch_publication", { branchId }),
  enterPublication: (branchId: string, artifactPath: string) =>
    invokeCommand<BranchPublication>("enter_branch_publication", {
      request: { branchId, artifactPath },
    }),
  publish: (request: PublishBranchRequest) =>
    invokeCommand<PublishResult>("publish_branch_artifact", { request }),
  previewPublication: (request: PublicationPreviewRequest) =>
    invokeCommand<PublicationPreview>("preview_branch_artifact_output", { request }),
  cancelOperation: () =>
    invokeCommand<boolean>("cancel_authenticity_operation"),
  cancelPublication: (branchId: string) =>
    invokeCommand<CleanupReport>("cancel_branch_publication", { branchId }),
  previewExternal: (path: string) =>
    invokeCommand<PreviewImage>("preview_authenticity_image", { path }),
  previewArtifact: (branchId: string) =>
    invokeCommand<PreviewImage>("preview_branch_artifact", { branchId }),
  previewRecord: (recordId: string) =>
    invokeCommand<PreviewImage>("preview_certification_record", { recordId }),
  exportRecord: (recordId: string, outputPath: string) =>
    invokeCommand<void>("export_certification_record", { request: { recordId, outputPath } }),
  estimate: (branchId: string, jpegQuality: number, backgroundColor: string) =>
    invokeCommand<{ jpegBytes: number; sourceBytes: number }>("estimate_authenticity_output_size", { request: { branchId, jpegQuality, backgroundColor } }),
  decode: (inputPath: string, region: NormalizedRegion | null) =>
    invokeCommand<DecodeResult>("decode_authenticity", {
      request: { inputPath, region },
    }),
  searchRecords: (query: string) =>
    invokeCommand<CertificationRecord[]>("search_certification_records", { query }),
};
