import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "../../shared/errors";
import { useAsyncOperation } from "../../shared/useAsyncOperation";
import { useRequestEpoch } from "../../shared/useRequestEpoch";
import { authenticityApi } from "./api";
import type {
  CertificationRecord,
  DecodeResult,
  NormalizedRegion,
  PreviewImage,
} from "./types";
import type { TileRequest } from "./previewTile";

interface IdentificationControllerOptions {
  onError: (message: string | null) => void;
}

export function useIdentificationController({ onError }: IdentificationControllerOptions) {
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<PreviewImage | null>(null);
  const [region, setRegion] = useState<NormalizedRegion | null>(null);
  const [result, setResult] = useState<DecodeResult | null>(null);
  const [query, setQuery] = useState("");
  const [records, setRecords] = useState<CertificationRecord[]>([]);
  const { busy: previewBusy, run: runPreview } = useAsyncOperation();
  const { busy: decodeBusy, run: runDecode } = useAsyncOperation();
  const { busy: searching, run: runSearch } = useAsyncOperation();
  const { begin: beginPreview, isCurrent: isPreviewCurrent } = useRequestEpoch();
  const { begin: beginDecode, invalidate: invalidateDecode, isCurrent: isDecodeCurrent } = useRequestEpoch();
  const { begin: beginSearch, invalidate: invalidateSearch, isCurrent: isSearchCurrent } = useRequestEpoch();

  const applyPreview = useCallback(async (selected: string, requestId: number) => {
    try {
      const nextPreview = await authenticityApi.previewExternal(selected);
      if (!isPreviewCurrent(requestId)) return;
      setPath(selected);
      setPreview(nextPreview);
      setRegion(null);
      setResult(null);
    } catch (error) {
      if (isPreviewCurrent(requestId)) onError(errorMessage(error));
    }
  }, [isPreviewCurrent, onError]);

  const choose = useCallback(async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }],
    });
    if (typeof selected !== "string") return;
    await runPreview(async () => {
      const requestId = beginPreview();
      invalidateDecode();
      await applyPreview(selected, requestId);
    });
  }, [applyPreview, beginPreview, invalidateDecode, runPreview]);

  // 拖放导入：前端读取原始字节后由后端落到系统临时目录并授权，再走同一条外部
  // 图片预览路径，因此识别仍使用受控的文件路径。
  const importDropped = useCallback(async (fileName: string, dataBase64: string) => {
    await runPreview(async () => {
      const requestId = beginPreview();
      invalidateDecode();
      onError(null);
      try {
        const staged = await authenticityApi.stageDroppedImage({ fileName, dataBase64 });
        if (!isPreviewCurrent(requestId)) return;
        await applyPreview(staged, requestId);
      } catch (error) {
        if (isPreviewCurrent(requestId)) onError(errorMessage(error));
      }
    });
  }, [applyPreview, beginPreview, invalidateDecode, isPreviewCurrent, onError, runPreview]);

  const decode = useCallback(async () => {
    if (!path) return;
    const inputPath = path;
    const inputRegion = region;
    await runDecode(async () => {
      const requestId = beginDecode();
      onError(null);
      try {
        const next = await authenticityApi.decode(inputPath, inputRegion);
        // 换图会推进代次（`choose` / `importDropped` 都调用 `invalidateDecode`），
        // 因此这里只需判断代次，不必再比较路径。
        if (isDecodeCurrent(requestId)) setResult(next);
      } catch (error) {
        if (isDecodeCurrent(requestId)) onError(errorMessage(error));
      }
    });
  }, [beginDecode, isDecodeCurrent, onError, path, region, runDecode]);

  const searchRecords = useCallback(async () => {
    const requestedQuery = query.trim();
    if (!requestedQuery) {
      invalidateSearch();
      setRecords([]);
      return;
    }
    await runSearch(async () => {
      const requestId = beginSearch();
      try {
        const next = await authenticityApi.searchRecords(requestedQuery);
        if (isSearchCurrent(requestId)) setRecords(next);
      } catch (error) {
        if (isSearchCurrent(requestId)) onError(errorMessage(error));
      }
    });
  }, [beginSearch, invalidateSearch, isSearchCurrent, onError, query, runSearch]);

  useEffect(() => {
    const timer = window.setTimeout(() => { void searchRecords(); }, 220);
    return () => window.clearTimeout(timer);
  }, [searchRecords]);

  // 框选放大镜：识别页的取样源是待识别图片本身（仓库之外的外部文件），
  // 由后端按同一条路径授权检查访问。
  const previewLoupe = useCallback((tile: TileRequest) => {
    if (!path) return Promise.reject(new Error("尚未选择待识别图片"));
    return authenticityApi.previewTile({ source: "external", cacheToken: null, branchId: null, path, ...tile });
  }, [path]);

  return {
    path,
    preview,
    region,
    setRegion,
    result,
    query,
    setQuery,
    records,
    busy: previewBusy || decodeBusy,
    searching: searching && query.trim().length > 0,
    choose,
    importDropped,
    decode,
    searchRecords,
    previewLoupe,
  };
}
