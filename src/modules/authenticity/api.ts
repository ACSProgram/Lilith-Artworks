import { invokeCommand } from "../../shared/tauri";
import type {
  BranchPublication,
  CertificationRecord,
  DecodeResult,
  NormalizedRegion,
  PreviewImage,
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
  preview: (path: string) =>
    invokeCommand<PreviewImage>("preview_authenticity_image", { path }),
  decode: (inputPath: string, region: NormalizedRegion | null) =>
    invokeCommand<DecodeResult>("decode_authenticity", {
      request: { inputPath, region },
    }),
  searchRecords: (query: string) =>
    invokeCommand<CertificationRecord[]>("search_certification_records", { query }),
};
