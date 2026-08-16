import { open, save } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import type { CleanupFailure, CleanupReport } from "../../shared/fileCleanup";
import { authenticityApi } from "./api";
import type {
  AuthenticityBranch,
  BranchPublication,
  CertificationConfig,
  CertificationRecord,
  DecodeResult,
  NormalizedRegion,
  PreviewImage,
  PublicationPreview,
  PublishResult,
} from "./types";
import { publicationPreviewError, publicationPreviewSignature } from "./publicationValidation";

const SHARED_SIGNING_KEY = "lilith-artworks.certification-signing-v1";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function privateKeyPem(value: string): string {
  const trimmed = value.trim();
  const pem = trimmed.match(/-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----([\s\S]*?)-----END \1-----/i);
  const label = pem?.[1]?.toUpperCase() || "PRIVATE KEY";
  const body = (pem?.[2] || trimmed).replace(/\s+/g, "");
  const lines = body.match(/.{1,64}/g)?.join("\n") || "";
  return `-----BEGIN ${label}-----\n${lines}\n-----END ${label}-----`;
}

function loadSharedSigning(): Partial<CertificationConfig> {
  try { return JSON.parse(localStorage.getItem(SHARED_SIGNING_KEY) ?? "{}"); }
  catch { return {}; }
}

function saveSharedSigning(config: CertificationConfig) {
  localStorage.setItem(SHARED_SIGNING_KEY, JSON.stringify({
    certificatePath: config.certificatePath,
    signingAlgorithm: config.signingAlgorithm,
    timestampUrl: config.timestampUrl,
  }));
}

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
  const [cancelling, setCancelling] = useState(false);
  const [privateKey, setPrivateKey] = useState("");
  const [watermarkId, setWatermarkId] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CertificationRecord | null>(null);
  const [publishMetrics, setPublishMetrics] = useState<PublishResult | null>(null);
  const [sizeEstimate, setSizeEstimate] = useState<number | null>(null);
  const [viewingRecord, setViewingRecord] = useState<CertificationRecord | null>(null);
  const [viewingPreview, setViewingPreview] = useState<PreviewImage | null>(null);
  const [exporting, setExporting] = useState(false);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [cleanupFailures, setCleanupFailures] = useState<CleanupFailure[]>([]);
  const loadRequest = useRef(0);
  const artifactPreviewRequest = useRef(0);
  const estimateRequest = useRef(0);
  const viewingRequest = useRef(0);
  const outputPreviewRequest = useRef(0);
  const activeOperation = useRef(false);
  const operationCancelRequested = useRef(false);
  const selectedBranchIdRef = useRef(selectedBranchId);
  const configRef = useRef(config);
  const watermarkIdRef = useRef(watermarkId);
  selectedBranchIdRef.current = selectedBranchId;
  configRef.current = config;
  watermarkIdRef.current = watermarkId;
  const selectedBranch = branches.find((branch) => branch.id === selectedBranchId) ?? null;

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
    const requestId = ++artifactPreviewRequest.current;
    setArtifactPreviewBusy(true);
    setArtifactPreviewError(null);
    setPreview(null);
    try {
      const next = await authenticityApi.previewArtifact(branchId);
      if (requestId !== artifactPreviewRequest.current
        || selectedBranchIdRef.current !== branchId) return;
      setPreview(next);
    } catch (error) {
      if (requestId === artifactPreviewRequest.current
        && selectedBranchIdRef.current === branchId) {
        setArtifactPreviewError(message(error));
      }
    } finally {
      if (requestId === artifactPreviewRequest.current) setArtifactPreviewBusy(false);
    }
  }, []);

  const load = useCallback(async () => {
    if (!selectedBranchId) return;
    const branchId = selectedBranchId;
    const requestId = ++loadRequest.current;
    setBusy(true);
    try {
      const next = await authenticityApi.getPublication(branchId);
      if (requestId !== loadRequest.current || selectedBranchIdRef.current !== branchId) return;
      applyPublication(next);
      setBusy(false);
      if (next.artifact) {
        await loadArtifactPreview(branchId);
      } else {
        artifactPreviewRequest.current += 1;
        setArtifactPreviewBusy(false);
        setArtifactPreviewError(null);
        setPreview(null);
      }
    } catch (error) {
      if (requestId === loadRequest.current && selectedBranchIdRef.current === branchId) {
        onError(message(error));
      }
    } finally {
      if (requestId === loadRequest.current) setBusy(false);
    }
  }, [applyPublication, loadArtifactPreview, onError, selectedBranchId]);

  useEffect(() => {
    loadRequest.current += 1;
    artifactPreviewRequest.current += 1;
    setPublication(null);
    setConfig(null);
    setPreview(null);
    setArtifactPreviewBusy(false);
    setArtifactPreviewError(null);
    setOutputPreview(null);
    setOutputPreviewOpen(false);
    setOutputPreviewBusy(false);
    setPublishing(false);
    setCancelling(false);
    outputPreviewRequest.current += 1;
    setViewingRecord(null);
    setResult(null);
    setPublishMetrics(null);
    setWatermarkId("");
    setBusy(Boolean(selectedBranchId));
    void load();
    return () => { loadRequest.current += 1; };
  }, [load, selectedBranchId]);

  useEffect(() => () => {
    if (activeOperation.current) void authenticityApi.cancelOperation();
  }, [selectedBranchId]);

  useEffect(() => {
    if (config && (!config.trustmarkEnabled || config.additionalRegions.length === 0)) {
      setWatermarkId("");
    }
  }, [config?.additionalRegions.length, config?.trustmarkEnabled]);

  useEffect(() => {
    if (!publication?.artifact || !config) return;
    const requestId = ++estimateRequest.current;
    setSizeEstimate(null);
    const timer = window.setTimeout(() => {
      authenticityApi.estimate(
        publication.branchId,
        config.jpegQuality,
        config.backgroundColor,
      )
        .then((value) => {
          if (requestId === estimateRequest.current) setSizeEstimate(value.jpegBytes);
        })
        .catch(() => {
          if (requestId === estimateRequest.current) setSizeEstimate(null);
        });
    }, 180);
    return () => {
      window.clearTimeout(timer);
      estimateRequest.current += 1;
    };
  }, [config?.backgroundColor, config?.jpegQuality, publication?.artifact]);

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
    const requestId = ++viewingRequest.current;
    setViewingPreview(null);
    if (!viewingRecord) return;
    authenticityApi.previewRecord(viewingRecord.id)
      .then((next) => {
        if (requestId === viewingRequest.current) setViewingPreview(next);
      })
      .catch((error) => {
        if (requestId === viewingRequest.current) onError(message(error));
      });
    return () => { viewingRequest.current += 1; };
  }, [onError, viewingRecord]);

  const enterPublication = useCallback(async () => {
    if (!selectedBranch) return;
    const artifactPath = await open({
      multiple: false,
      filters: [{ name: "最终成品", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }],
    });
    if (typeof artifactPath !== "string") return;
    const branchId = selectedBranch.id;
    const requestId = ++loadRequest.current;
    setBusy(true);
    onError(null);
    try {
      const next = await authenticityApi.enterPublication(branchId, artifactPath);
      if (requestId !== loadRequest.current || selectedBranchIdRef.current !== branchId) return;
      applyPublication(next);
      setBusy(false);
      await Promise.all([
        next.artifact ? loadArtifactPreview(branchId) : Promise.resolve(),
        onPublicationChanged?.() ?? Promise.resolve(),
      ]);
    } catch (error) {
      if (requestId === loadRequest.current) onError(message(error));
    } finally {
      if (requestId === loadRequest.current) setBusy(false);
    }
  }, [applyPublication, loadArtifactPreview, onError, onPublicationChanged, selectedBranch]);

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
    const requestId = ++outputPreviewRequest.current;
    operationCancelRequested.current = false;
    activeOperation.current = true;
    setOutputPreviewBusy(true);
    setOutputPreview(null);
    onError(null);
    try {
      const next = await authenticityApi.previewPublication({
        branchId,
        config,
        watermarkId: watermarkId.trim() || null,
      });
      if (requestId !== outputPreviewRequest.current
        || selectedBranchIdRef.current !== branchId
        || !configRef.current
        || publicationPreviewSignature(configRef.current, watermarkIdRef.current) !== signature) return;
      if (next.watermarkId) setWatermarkId(next.watermarkId);
      setOutputPreview(next);
      setOutputPreviewOpen(true);
    } catch (error) {
      if (requestId === outputPreviewRequest.current && !operationCancelRequested.current) {
        onError(message(error));
      }
    } finally {
      activeOperation.current = false;
      if (requestId === outputPreviewRequest.current) setOutputPreviewBusy(false);
      setCancelling(false);
    }
  }, [config, onError, privateKey, publication, selectedBranch, watermarkId]);

  const cancelAuthenticityOperation = useCallback(async () => {
    operationCancelRequested.current = true;
    setCancelling(true);
    try {
      const accepted = await authenticityApi.cancelOperation();
      if (!accepted) {
        activeOperation.current = false;
        setOutputPreviewBusy(false);
        setPublishing(false);
        setCancelling(false);
      }
    } catch (error) {
      operationCancelRequested.current = false;
      setCancelling(false);
      onError(message(error));
    }
  }, [onError]);

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
    setBusy(true);
    setPublishing(true);
    setCancelling(false);
    operationCancelRequested.current = false;
    activeOperation.current = true;
    setResult(null);
    setPublishMetrics(null);
    onError(null);
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
      if (!operationCancelRequested.current) onError(message(error));
    } finally {
      activeOperation.current = false;
      setPublishing(false);
      setCancelling(false);
      if (selectedBranchIdRef.current === branchId) setBusy(false);
    }
  }, [artworkTitle, config, load, onError, onPublicationChanged, outputPreview, privateKey, publication, selectedBranch, watermarkId]);

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
      localStorage.removeItem(SHARED_SIGNING_KEY);
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
      onError(message(error));
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
      onError(message(error));
    } finally {
      setBusy(false);
    }
  }, [cleanupFailures, onError, onRetryFileCleanup]);

  const openRecord = useCallback((record: CertificationRecord) => {
    setViewingRecord(record);
    onNavigateRecord(record);
  }, [onNavigateRecord]);

  const exportRecord = useCallback(async (record: CertificationRecord) => {
    const outputPath = await save({
      defaultPath: record.outputPath.split(/[\\/]/).pop() || record.outputPath,
      filters: [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
    });
    if (!outputPath) return;
    setExporting(true);
    onError(null);
    try { await authenticityApi.exportRecord(record.id, outputPath); }
    catch (error) { onError(message(error)); }
    finally { setExporting(false); }
  }, [onError]);

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
    openRecord,
    exportRecord,
  };
}

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
  const previewRequest = useRef(0);
  const decodeRequest = useRef(0);
  const searchRequest = useRef(0);

  const choose = useCallback(async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "tif", "tiff"] }],
    });
    if (typeof selected !== "string") return;
    const requestId = ++previewRequest.current;
    decodeRequest.current += 1;
    setPreviewBusy(true);
    try {
      const nextPreview = await authenticityApi.previewExternal(selected);
      if (requestId !== previewRequest.current) return;
      setPath(selected);
      setPreview(nextPreview);
      setRegion(null);
      setResult(null);
    } catch (error) {
      if (requestId === previewRequest.current) onError(message(error));
    } finally {
      if (requestId === previewRequest.current) setPreviewBusy(false);
    }
  }, [onError]);

  const decode = useCallback(async () => {
    if (!path) return;
    const requestId = ++decodeRequest.current;
    const inputPath = path;
    const inputRegion = region;
    setDecodeBusy(true);
    onError(null);
    try {
      const next = await authenticityApi.decode(inputPath, inputRegion);
      if (requestId === decodeRequest.current && inputPath === path) setResult(next);
    } catch (error) {
      if (requestId === decodeRequest.current) onError(message(error));
    } finally {
      if (requestId === decodeRequest.current) setDecodeBusy(false);
    }
  }, [onError, path, region]);

  const searchRecords = useCallback(async () => {
    const requestId = ++searchRequest.current;
    const requestedQuery = query.trim();
    if (!requestedQuery) {
      setRecords([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    try {
      const next = await authenticityApi.searchRecords(requestedQuery);
      if (requestId === searchRequest.current) setRecords(next);
    } catch (error) {
      if (requestId === searchRequest.current) onError(message(error));
    } finally {
      if (requestId === searchRequest.current) setSearching(false);
    }
  }, [onError, query]);

  useEffect(() => {
    const timer = window.setTimeout(() => { void searchRecords(); }, 220);
    return () => window.clearTimeout(timer);
  }, [searchRecords]);

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
    decode,
    searchRecords,
  };
}
