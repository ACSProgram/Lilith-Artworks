import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "../../shared/errors";
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
  const [previewBusy, setPreviewBusy] = useState(false);
  const [decodeBusy, setDecodeBusy] = useState(false);
  const [searching, setSearching] = useState(false);
  const { begin: beginPreview, isCurrent: isPreviewCurrent } = useRequestEpoch();
  const { begin: beginDecode, invalidate: invalidateDecode, isCurrent: isDecodeCurrent } = useRequestEpoch();
  const { begin: beginSearch, isCurrent: isSearchCurrent } = useRequestEpoch();

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
    } finally {
      if (isPreviewCurrent(requestId)) setPreviewBusy(false);
    }
  }, [isPreviewCurrent, onError]);

  const choose = useCallback(async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }],
    });
    if (typeof selected !== "string") return;
    const requestId = beginPreview();
    invalidateDecode();
    setPreviewBusy(true);
    await applyPreview(selected, requestId);
  }, [applyPreview, beginPreview, invalidateDecode]);

  // 拖放导入：前端读取原始字节后由后端落到系统临时目录并授权，再走同一条外部
  // 图片预览路径，因此识别仍使用受控的文件路径。
  const importDropped = useCallback(async (fileName: string, dataBase64: string) => {
    const requestId = beginPreview();
    invalidateDecode();
    setPreviewBusy(true);
    onError(null);
    try {
      const staged = await authenticityApi.stageDroppedImage({ fileName, dataBase64 });
      if (!isPreviewCurrent(requestId)) return;
      await applyPreview(staged, requestId);
    } catch (error) {
      if (isPreviewCurrent(requestId)) {
        setPreviewBusy(false);
        onError(errorMessage(error));
      }
    }
  }, [applyPreview, beginPreview, invalidateDecode, isPreviewCurrent, onError]);

  const decode = useCallback(async () => {
    if (!path) return;
    const requestId = beginDecode();
    const inputPath = path;
    const inputRegion = region;
    setDecodeBusy(true);
    onError(null);
    try {
      const next = await authenticityApi.decode(inputPath, inputRegion);
      // 换图会推进代次（`choose` / `importDropped` 都调用 `invalidateDecode`），
      // 因此这里只需判断代次，不必再比较路径。
      if (isDecodeCurrent(requestId)) setResult(next);
    } catch (error) {
      if (isDecodeCurrent(requestId)) onError(errorMessage(error));
    } finally {
      if (isDecodeCurrent(requestId)) setDecodeBusy(false);
    }
  }, [beginDecode, isDecodeCurrent, onError, path, region]);

  const searchRecords = useCallback(async () => {
    const requestId = beginSearch();
    const requestedQuery = query.trim();
    if (!requestedQuery) {
      setRecords([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    try {
      const next = await authenticityApi.searchRecords(requestedQuery);
      if (isSearchCurrent(requestId)) setRecords(next);
    } catch (error) {
      if (isSearchCurrent(requestId)) onError(errorMessage(error));
    } finally {
      if (isSearchCurrent(requestId)) setSearching(false);
    }
  }, [beginSearch, isSearchCurrent, onError, query]);

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
    searching,
    choose,
    importDropped,
    decode,
    searchRecords,
    previewLoupe,
  };
}
