import { open, save } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "../../shared/errors";
import type { CleanupFailure, CleanupReport } from "../../shared/fileCleanup";
import { fileName } from "../../shared/format";
import { useAsyncOperation } from "../../shared/useAsyncOperation";
import { useRequestEpoch } from "../../shared/useRequestEpoch";
import { authenticityApi } from "./api";
import {
  clearSharedSigning,
  loadSharedSigning,
  privateKeyPem,
  saveSharedSigning,
} from "./publicationStorage";
import type {
  AuthenticityBranch,
  BranchPublication,
  CertificationConfig,
  CertificationRecord,
  OutputPreviewTileSource,
  PreviewImage,
  PublicationPreview,
  PublishResult,
} from "./types";
import { publicationPreviewError, publicationPreviewSignature } from "./publicationValidation";
import type { TileRequest } from "./previewTile";
import { useAuthenticityOperation } from "./useAuthenticityOperation";

interface PublicationControllerOptions {
  artworkTitle: string;
  branches: AuthenticityBranch[];
  selectedBranchId: string | null;
  selectedRecordId?: string | null;
  recordNavigationKey?: number;
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
  onRetryFileCleanup: (ids: string[]) => Promise<CleanupReport>;
  onPublicationChanged?: () => Promise<void>;
}

export function usePublicationController({
  artworkTitle,
  branches,
  selectedBranchId,
  selectedRecordId,
  recordNavigationKey,
  onError,
  onNavigateRecord,
  onRetryFileCleanup,
  onPublicationChanged,
}: PublicationControllerOptions) {
  const [publication, setPublication] = useState<BranchPublication | null>(null);
  const [config, setConfig] = useState<CertificationConfig | null>(null);
  const [preview, setPreview] = useState<PreviewImage | null>(null);
  const [artifactPreviewBusy, setArtifactPreviewBusy] = useState(false);
  const [artifactPreviewError, setArtifactPreviewError] = useState<string | null>(null);
  const [outputPreview, setOutputPreview] = useState<PublicationPreview | null>(null);
  const [outputPreviewOpen, setOutputPreviewOpen] = useState(false);
  const [outputPreviewBusy, setOutputPreviewBusy] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [privateKey, setPrivateKey] = useState("");
  const [watermarkId, setWatermarkId] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CertificationRecord | null>(null);
  const [publishMetrics, setPublishMetrics] = useState<PublishResult | null>(null);
  const [sizeEstimate, setSizeEstimate] = useState<number | null>(null);
  const [viewingRecord, setViewingRecord] = useState<CertificationRecord | null>(null);
  const [viewingPreview, setViewingPreview] = useState<PreviewImage | null>(null);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [cleanupFailures, setCleanupFailures] = useState<CleanupFailure[]>([]);
  const { begin: beginLoad, invalidate: invalidateLoad, isCurrent: isLoadCurrent } = useRequestEpoch();
  const {
    begin: beginArtifactPreview,
    invalidate: invalidateArtifactPreview,
    isCurrent: isArtifactPreviewCurrent,
  } = useRequestEpoch();
  const {
    begin: beginEstimate,
    invalidate: invalidateEstimate,
    isCurrent: isEstimateCurrent,
  } = useRequestEpoch();
  const {
    begin: beginViewing,
    invalidate: invalidateViewing,
    isCurrent: isViewingCurrent,
  } = useRequestEpoch();
  const {
    begin: beginOutputPreview,
    invalidate: invalidateOutputPreview,
    isCurrent: isOutputPreviewCurrent,
  } = useRequestEpoch();
  const { busy: exporting, run: runExport } = useAsyncOperation();
  const { busy: deletingRecord, run: runDeleteRecord } = useAsyncOperation();
  const selectedBranchIdRef = useRef(selectedBranchId);
  const configRef = useRef(config);
  const watermarkIdRef = useRef(watermarkId);
  selectedBranchIdRef.current = selectedBranchId;
  configRef.current = config;
  watermarkIdRef.current = watermarkId;
  const selectedBranch = branches.find((branch) => branch.id === selectedBranchId) ?? null;
  const resetOperationBusy = useCallback(() => {
    setOutputPreviewBusy(false);
    setPublishing(false);
  }, []);
  const {
    cancelling,
    isActive: isOperationActive,
    isCancelRequested,
    begin: beginOperation,
    finish: finishOperation,
    resetCancelling: resetOperationCancelling,
    cancel: cancelAuthenticityOperation,
  } = useAuthenticityOperation({
    onError,
    cancelOperation: authenticityApi.cancelOperation,
    onCancelRejected: resetOperationBusy,
  });

  const applyPublication = useCallback((next: BranchPublication) => {
    setPublication(next);
    setConfig({
      ...next.config,
      ...loadSharedSigning(),
      trustmarkEnabled: next.modelsReady
        && next.config.trustmarkEnabled
        && next.config.additionalRegions.length > 0,
    });
  }, []);

  const loadArtifactPreview = useCallback(async (branchId: string) => {
    const requestId = beginArtifactPreview();
    setArtifactPreviewBusy(true);
    setArtifactPreviewError(null);
    setPreview(null);
    try {
      const next = await authenticityApi.previewArtifact(branchId);
      if (!isArtifactPreviewCurrent(requestId)
        || selectedBranchIdRef.current !== branchId) return;
      setPreview(next);
    } catch (error) {
      if (isArtifactPreviewCurrent(requestId)
        && selectedBranchIdRef.current === branchId) {
        setArtifactPreviewError(errorMessage(error));
      }
    } finally {
      if (isArtifactPreviewCurrent(requestId)) setArtifactPreviewBusy(false);
    }
  }, [beginArtifactPreview, isArtifactPreviewCurrent]);

  const load = useCallback(async () => {
    if (!selectedBranchId) return;
    const branchId = selectedBranchId;
    const requestId = beginLoad();
    setBusy(true);
    try {
      const next = await authenticityApi.getPublication(branchId);
      if (!isLoadCurrent(requestId) || selectedBranchIdRef.current !== branchId) return;
      applyPublication(next);
      setBusy(false);
      if (next.artifact) {
        await loadArtifactPreview(branchId);
      } else {
        invalidateArtifactPreview();
        setArtifactPreviewBusy(false);
        setArtifactPreviewError(null);
        setPreview(null);
      }
    } catch (error) {
      if (isLoadCurrent(requestId) && selectedBranchIdRef.current === branchId) {
        onError(errorMessage(error));
      }
    } finally {
      if (isLoadCurrent(requestId)) setBusy(false);
    }
  }, [
    applyPublication,
    beginLoad,
    invalidateArtifactPreview,
    isLoadCurrent,
    loadArtifactPreview,
    onError,
    selectedBranchId,
  ]);

  useEffect(() => {
    invalidateLoad();
    invalidateArtifactPreview();
    setPublication(null);
    setConfig(null);
    setPreview(null);
    setArtifactPreviewBusy(false);
    setArtifactPreviewError(null);
    setOutputPreview(null);
    setOutputPreviewOpen(false);
    setOutputPreviewBusy(false);
    setPublishing(false);
    resetOperationCancelling();
    invalidateOutputPreview();
    setViewingRecord(null);
    setResult(null);
    setPublishMetrics(null);
    setWatermarkId("");
    setBusy(Boolean(selectedBranchId));
    void load();
    return invalidateLoad;
  }, [invalidateArtifactPreview, invalidateLoad, invalidateOutputPreview, load, resetOperationCancelling, selectedBranchId]);

  // 切换分支或卸载时中断仍在运行的质量预览/签名任务；`selectedBranchId` 进入依赖
  // 就是为了在切换那一刻触发一次清理。
  useEffect(() => () => {
    if (isOperationActive()) void authenticityApi.cancelOperation();
  }, [isOperationActive, selectedBranchId]);

  useEffect(() => {
    if (config && (!config.trustmarkEnabled || config.additionalRegions.length === 0)) {
      setWatermarkId("");
    }
  }, [config?.additionalRegions.length, config?.trustmarkEnabled]);

  useEffect(() => {
    if (!publication?.artifact || !config) return;
    const requestId = beginEstimate();
    setSizeEstimate(null);
    const timer = window.setTimeout(async () => {
      try {
        const value = await authenticityApi.estimate(
          publication.branchId,
          config.jpegQuality,
          config.backgroundColor,
        );
        if (isEstimateCurrent(requestId)) setSizeEstimate(value.jpegBytes);
      } catch {
        if (isEstimateCurrent(requestId)) setSizeEstimate(null);
      }
    }, 180);
    return () => {
      window.clearTimeout(timer);
      invalidateEstimate();
    };
  }, [
    beginEstimate,
    config?.backgroundColor,
    config?.jpegQuality,
    invalidateEstimate,
    isEstimateCurrent,
    publication?.artifact,
  ]);

  useEffect(() => {
    if (config) saveSharedSigning(config);
  }, [config?.certificatePath, config?.signingAlgorithm, config?.timestampUrl]);

  useEffect(() => {
    if (!selectedRecordId || !publication) return;
    window.requestAnimationFrame(() => {
      document.querySelector(`[data-record-id="${selectedRecordId}"]`)
        ?.scrollIntoView({ block: "center" });
    });
  }, [publication, recordNavigationKey, selectedRecordId]);

  useEffect(() => {
    const requestId = beginViewing();
    setViewingPreview(null);
    if (!viewingRecord) return;
    authenticityApi.previewRecord(viewingRecord.id)
      .then((next) => {
        if (isViewingCurrent(requestId)) setViewingPreview(next);
      })
      .catch((error) => {
        if (isViewingCurrent(requestId)) onError(errorMessage(error));
      });
    return invalidateViewing;
  }, [beginViewing, invalidateViewing, isViewingCurrent, onError, viewingRecord]);

  const enterPublication = useCallback(async () => {
    if (!selectedBranch) return;
    const artifactPath = await open({
      multiple: false,
      filters: [{ name: "最终成品", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }],
    });
    if (typeof artifactPath !== "string") return;
    const branchId = selectedBranch.id;
    const requestId = beginLoad();
    setBusy(true);
    onError(null);
    try {
      const next = await authenticityApi.enterPublication(branchId, artifactPath);
      if (!isLoadCurrent(requestId) || selectedBranchIdRef.current !== branchId) return;
      applyPublication(next);
      setBusy(false);
      await Promise.all([
        next.artifact ? loadArtifactPreview(branchId) : Promise.resolve(),
        onPublicationChanged?.() ?? Promise.resolve(),
      ]);
    } catch (error) {
      if (isLoadCurrent(requestId)) onError(errorMessage(error));
    } finally {
      if (isLoadCurrent(requestId)) setBusy(false);
    }
  }, [
    applyPublication,
    beginLoad,
    isLoadCurrent,
    loadArtifactPreview,
    onError,
    onPublicationChanged,
    selectedBranch,
  ]);

  const retryArtifactPreview = useCallback(async () => {
    if (!selectedBranch || publication?.branchId !== selectedBranch.id || !publication.artifact) return;
    await loadArtifactPreview(selectedBranch.id);
  }, [loadArtifactPreview, publication, selectedBranch]);

  const chooseCertificate = useCallback(async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "证书链", extensions: ["pem", "crt", "cer"] }],
    });
    if (typeof path === "string") {
      setConfig((current) => current ? { ...current, certificatePath: path } : current);
    }
  }, []);

  const generateOutputPreview = useCallback(async () => {
    if (!selectedBranch || !config) return;
    if (publication?.branchId !== selectedBranch.id
      || publication.artifact?.branchId !== selectedBranch.id
      || config.branchId !== selectedBranch.id) {
      onError("当前分支的发布状态尚未加载完成，请稍后重试。");
      return;
    }
    const validationError = publicationPreviewError(config, privateKey);
    if (validationError) {
      onError(validationError);
      return;
    }
    const branchId = selectedBranch.id;
    const signature = publicationPreviewSignature(config, watermarkId);
    const requestId = beginOutputPreview();
    beginOperation("preview");
    setOutputPreviewBusy(true);
    setOutputPreview(null);
    onError(null);
    let cancelled = false;
    try {
      const next = await authenticityApi.previewPublication({
        branchId,
        config,
        watermarkId: watermarkId.trim() || null,
      });
      if (!isOutputPreviewCurrent(requestId)
        || selectedBranchIdRef.current !== branchId
        || !configRef.current
        || publicationPreviewSignature(configRef.current, watermarkIdRef.current) !== signature) return;
      if (next.watermarkId) setWatermarkId(next.watermarkId);
      setOutputPreview(next);
      setOutputPreviewOpen(true);
    } catch (error) {
      if (isOutputPreviewCurrent(requestId) && !isCancelRequested()) {
        onError(errorMessage(error));
      }
      cancelled = isCancelRequested();
    } finally {
      if (isOutputPreviewCurrent(requestId)) setOutputPreviewBusy(false);
      finishOperation("preview", cancelled);
    }
  }, [
    beginOperation,
    beginOutputPreview,
    config,
    finishOperation,
    isCancelRequested,
    isOutputPreviewCurrent,
    onError,
    privateKey,
    publication,
    selectedBranch,
    watermarkId,
  ]);

  const publish = useCallback(async () => {
    if (!selectedBranch || !config) return;
    if (publication?.branchId !== selectedBranch.id
      || publication.artifact?.branchId !== selectedBranch.id
      || config.branchId !== selectedBranch.id) {
      onError("当前分支的发布状态尚未加载完成，请稍后重试。");
      return;
    }
    if (!privateKey.trim()) {
      setOutputPreviewOpen(false);
      onError("请输入 PEM 私钥后再发布。");
      return;
    }
    const outputPath = await save({
      defaultPath: `${config.title.trim() || artworkTitle}-certified.jpg`,
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
    });
    if (!outputPath) return;
    const branchId = selectedBranch.id;
    beginOperation("publish");
    setBusy(true);
    setPublishing(true);
    setResult(null);
    setPublishMetrics(null);
    onError(null);
    let cancelled = false;
    try {
      const published = await authenticityApi.publish({
        branchId,
        outputPath,
        privateKeyPem: privateKeyPem(privateKey),
        config,
        watermarkId: watermarkId.trim() || null,
        previewCacheToken: outputPreview?.cacheToken ?? null,
      });
      setPrivateKey("");
      setWatermarkId("");
      if (selectedBranchIdRef.current === branchId) {
        setResult(published.record);
        setPublishMetrics(published);
        setOutputPreviewOpen(false);
        setOutputPreview(null);
        await load();
      }
      await onPublicationChanged?.();
    } catch (error) {
      setOutputPreviewOpen(false);
      if (!isCancelRequested()) onError(errorMessage(error));
      cancelled = isCancelRequested();
    } finally {
      setPublishing(false);
      if (selectedBranchIdRef.current === branchId) setBusy(false);
      finishOperation("publish", cancelled);
    }
  }, [artworkTitle, beginOperation, config, finishOperation, isCancelRequested, load, onError, onPublicationChanged, outputPreview, privateKey, publication, selectedBranch, watermarkId]);

  const cancelPublication = useCallback(async () => {
    if (!selectedBranch) return;
    const branchId = selectedBranch.id;
    setBusy(true);
    try {
      const report = await authenticityApi.cancelPublication(branchId);
      setCleanupFailures(report.failures);
      if (report.failures.length > 0) {
        onError(`分支已解除发布状态，但有 ${report.failures.length} 个文件清理失败；请重试清理。`);
      }
      clearSharedSigning();
      if (selectedBranchIdRef.current === branchId) {
        setDeleteConfirmOpen(false);
        setPublication(null);
        setConfig(null);
        setPreview(null);
        setOutputPreview(null);
        setOutputPreviewOpen(false);
        setPrivateKey("");
        setWatermarkId("");
        setResult(null);
        setPublishMetrics(null);
        setViewingRecord(null);
      }
      await onPublicationChanged?.();
    } catch (error) {
      onError(errorMessage(error));
    } finally {
      if (selectedBranchIdRef.current === branchId) setBusy(false);
    }
  }, [onError, onPublicationChanged, selectedBranch]);

  const retryCleanup = useCallback(async () => {
    if (cleanupFailures.length === 0) return;
    setBusy(true);
    onError(null);
    try {
      const report = await onRetryFileCleanup(cleanupFailures.map((failure) => failure.id));
      setCleanupFailures(report.failures);
      if (report.failures.length > 0) onError(`仍有 ${report.failures.length} 个文件无法清理。`);
    } catch (error) {
      onError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }, [cleanupFailures, onError, onRetryFileCleanup]);

  // 框选放大镜：发布页的取样源是分支最终成品，由后端按 branchId 解析受控路径裁剪，
  // 因此复用既有的「原始成品」局部通道，不引入新的路径参数。
  const previewLoupe = useCallback((tile: TileRequest) => {
    const branchId = selectedBranchIdRef.current;
    if (!branchId) return Promise.reject(new Error("当前分支尚未加载完成"));
    return authenticityApi.previewTile({ source: "original", cacheToken: null, branchId, ...tile });
  }, []);

  // 质量预览对话框的高清局部：压缩源绑定本次预览的缓存 token，原图绑定当前分支；
  // 各自缺少所需上下文时明确拒绝，不把半套参数交给后端。依赖只取 token，预览对象
  // 其他字段变化不会让对话框重建回调、重置去抖。
  const outputPreviewCacheToken = outputPreview?.cacheToken ?? null;
  const previewOutputTile = useCallback((
    source: OutputPreviewTileSource,
    tile: TileRequest,
  ) => {
    if (source === "compressed") {
      if (!outputPreviewCacheToken) return Promise.reject(new Error("导出预览尚未生成"));
      return authenticityApi.previewTile({
        source,
        cacheToken: outputPreviewCacheToken,
        branchId: null,
        ...tile,
      });
    }
    const branchId = selectedBranchIdRef.current;
    if (!branchId) return Promise.reject(new Error("当前分支尚未加载完成"));
    return authenticityApi.previewTile({ source, cacheToken: null, branchId, ...tile });
  }, [outputPreviewCacheToken]);

  const openRecord = useCallback((record: CertificationRecord) => {
    setViewingRecord(record);
    onNavigateRecord(record);
  }, [onNavigateRecord]);

  const exportRecord = useCallback(async (record: CertificationRecord) => {
    const outputPath = await save({
      defaultPath: fileName(record.outputPath),
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
    });
    if (!outputPath) return;
    onError(null);
    try {
      await runExport(
        () => authenticityApi.exportRecord(record.id, outputPath),
        { onError: (error) => onError(errorMessage(error)) },
      );
    } catch {
      // 错误已由 onError 提示。
    }
  }, [onError, runExport]);

  const deleteRecord = useCallback(async (record: CertificationRecord) => {
    onError(null);
    try {
      await runDeleteRecord(async () => {
        const report = await authenticityApi.deleteRecord(record.id);
        setCleanupFailures(report.failures);
        if (report.failures.length > 0) {
          onError(`记录已删除，但有 ${report.failures.length} 个文件清理失败；请重试清理。`);
        }
        if (selectedBranchIdRef.current === record.branchId) {
          setViewingRecord(null);
          await load();
        }
      }, { onError: (error) => onError(errorMessage(error)) });
    } catch {
      // 错误已由 onError 提示。
    }
  }, [load, onError, runDeleteRecord]);

  return {
    publication,
    config,
    setConfig,
    preview,
    artifactPreviewBusy,
    artifactPreviewError,
    outputPreview,
    outputPreviewOpen,
    setOutputPreviewOpen,
    outputPreviewBusy,
    publishing,
    cancelling,
    privateKey,
    setPrivateKey,
    watermarkId,
    setWatermarkId,
    busy,
    result,
    publishMetrics,
    sizeEstimate,
    viewingRecord,
    setViewingRecord,
    viewingPreview,
    exporting,
    deleteConfirmOpen,
    setDeleteConfirmOpen,
    cleanupFailures,
    selectedBranch,
    enterPublication,
    retryArtifactPreview,
    chooseCertificate,
    generateOutputPreview,
    cancelAuthenticityOperation,
    publish,
    cancelPublication,
    retryCleanup,
    previewLoupe,
    previewOutputTile,
    openRecord,
    exportRecord,
    deletingRecord,
    deleteRecord,
  };
}
