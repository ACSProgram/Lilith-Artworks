/**
 * 素材板的传输类操作：导入（选择器 / 剪贴板 / 拖入）、导出图片、整画布导出、添加文字
 * 与笔迹落库队列的共同样板。
 *
 * busy 标记、进度状态、取消标记与「先保存再改」的 `replaceAfterImageMutation` 链都在
 * 这里收口，控制器只负责画板列表、选择与生命周期。渲染器、当前画板与状态设置器由
 * 调用方显式注入，不反向读取控制器的内部状态。
 */

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { errorMessage } from "../../shared/errors";
import { useRuntimeSubscription } from "../../shared/useRuntimeSubscription";
import {
  pinBoardApi,
  pickPinBoardExportDirectory,
  pickPinBoardImagePaths,
} from "./api";
import type { CanvasExportPlan } from "./exportPolicy";
import {
  DEFAULT_PIN_BOARD_INTERACTION,
  type PinBoardRenderer,
  type PinBoardInteractionState,
} from "./renderer";
import { strokeToolId, type StrokeCommitRequest } from "./strokeDrawing";
import { measurePinBoardText } from "./textLayout";
import type {
  PinBoardMutationResult,
  PinBoardTransferProgress,
} from "./types";

const IMAGE_PATH_PATTERN = /\.(?:png|jpe?g|webp|bmp|gif|tga|dds)$/i;

/**
 * 拖入导入走「原始字节解码」路径（`import_pin_board_clipboard_image`），只能接受
 * 可被内容嗅探识别的位图格式；DDS/TGA 需要文件路径，仍通过「导入图片」选择器处理。
 */
const DROP_IMAGE_PATTERN = /\.(?:png|jpe?g|webp|bmp|gif)$/i;

async function clipboardImageBytes(): Promise<number[] | null> {
  if (!navigator.clipboard?.read) return null;
  try {
    const items = await navigator.clipboard.read();
    for (const item of items) {
      const imageType = ["image/png", "image/jpeg", "image/webp", "image/bmp", "image/gif"]
        .find((type) => item.types.includes(type));
      if (!imageType) continue;
      const blob = await item.getType(imageType);
      return Array.from(new Uint8Array(await blob.arrayBuffer()));
    }
  } catch {
    // Clipboard access may be unavailable; import falls back to paths and the dialog.
  }
  return null;
}

export interface PinBoardTransfersOptions {
  active: boolean;
  rendererRef: MutableRefObject<PinBoardRenderer | null>;
  selected: number | null;
  /** 排队中的笔迹提交按入队时的画板写，切走之后不再动旧画板。 */
  selectedBoardRef: MutableRefObject<number | null>;
  arrangementGapRatio: number;
  /** 整画布导出的建议上限（取素材板设置的纹理缓存等级）。 */
  canvasExportLimit: number;
  beginLoad: () => number;
  isLoadCurrent: (sequence: number) => boolean;
  setStatus: (message: string | null) => void;
  setInteraction: Dispatch<SetStateAction<PinBoardInteractionState>>;
}

export interface PinBoardTransfers {
  transferProgress: PinBoardTransferProgress | null;
  /** 是否有传输在跑：画板排序之类的入口要据此让路。 */
  isTransferBusy: () => boolean;
  replaceAfterImageMutation: (
    action: (renderer: PinBoardRenderer, revision: string) => Promise<PinBoardMutationResult>,
    success: (count: number) => string,
  ) => Promise<PinBoardMutationResult | null>;
  importImages: () => Promise<void>;
  importDroppedFiles: (files: FileList, clientX: number, clientY: number) => Promise<void>;
  addText: (
    text: string,
    fontSize: number,
    color: string,
    placement: [number, number],
  ) => Promise<void>;
  commitStroke: (request: StrokeCommitRequest) => void;
  exportImages: () => Promise<void>;
  cancelTransfer: () => void;
  exportCanvas: (request: {
    directory: string;
    fileName: string;
    scale: number;
    margin: number;
  }) => Promise<void>;
  canvasExportPlan: (scale: number, margin: number) => CanvasExportPlan | null;
  canvasExportAutoScale: (margin: number) => number;
}

export function usePinBoardTransfers({
  active,
  rendererRef,
  selected,
  selectedBoardRef,
  arrangementGapRatio,
  canvasExportLimit,
  beginLoad,
  isLoadCurrent,
  setStatus,
  setInteraction,
}: PinBoardTransfersOptions): PinBoardTransfers {
  const [transferProgress, setTransferProgress] = useState<PinBoardTransferProgress | null>(null);
  const transferBusyRef = useRef(false);
  /**
   * 传输的取消标记：导入/导出图片由原生循环检查，整画布导出由渲染器逐块检查。
   * 一次只跑一个传输，因此一个布尔就够，每次传输开始时复位。
   */
  const transferCancelRef = useRef(false);
  /** 当前传输的 operation id：首个进度事件到达后锁定，其它操作的进度直接丢弃。 */
  const transferOperationId = useRef<string | null>(null);
  /** 笔迹创建的串行队列：两笔并发落库会撞上同一个 revision。 */
  const strokeQueueRef = useRef<Promise<void>>(Promise.resolve());

  /**
   * 离开工作区（切标签页、切作品）视为「用户离开该画板」：中止在途的传输，
   * 由渲染器在下一块之前停下并丢弃临时文件。
   */
  useEffect(() => {
    if (!active) transferCancelRef.current = true;
  }, [active]);

  // 导入/导出经 `CommandContext` 广播进度；只在本模块确实有传输时消费。
  // 首个进度事件锁定 operation id，避免并发/迟到的其它操作覆盖当前进度。
  useRuntimeSubscription((event) => {
    if (!transferBusyRef.current || event.domain !== "pinBoard") return;
    if (event.type !== "operationProgress") return;
    if (transferOperationId.current === null && event.operationId) {
      transferOperationId.current = event.operationId;
    }
    if (transferOperationId.current
      && event.operationId
      && event.operationId !== transferOperationId.current) {
      return;
    }
    setTransferProgress((current) => ({
      label: event.phase ?? "正在处理图片",
      current: event.current ?? 0,
      total: event.total ?? 0,
      // 可取消性由发起方决定，原生进度事件只更新数字。
      cancellable: current?.cancellable ?? false,
    }));
  });

  const isTransferBusy = useCallback(() => transferBusyRef.current, []);

  const replaceAfterImageMutation = useCallback(async (
    action: (renderer: PinBoardRenderer, revision: string) => Promise<PinBoardMutationResult>,
    success: (count: number) => string,
  ): Promise<PinBoardMutationResult | null> => {
    const current = rendererRef.current;
    if (!current
      || current.interactionState.locked
      || current.interactionState.textEditing) return null;
    const sequence = beginLoad();
    if (!await current.save() || !isLoadCurrent(sequence)) return null;
    try {
      const result = await action(current, current.boardRevision);
      if (!isLoadCurrent(sequence)) return null;
      current.applyMutationResult(result.view, result.itemIds);
      setInteraction(current.interactionState);
      setStatus(success(result.itemIds.length));
      return result;
    } catch (error) {
      if (isLoadCurrent(sequence)) setStatus(errorMessage(error));
      return null;
    }
  }, [beginLoad, isLoadCurrent, rendererRef, setInteraction, setStatus]);

  const importImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current || current.interactionState.locked || selected === null || transferBusyRef.current) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    try {
      const bytes = await clipboardImageBytes();
      if (bytes) {
        setTransferProgress({ label: "正在处理剪贴板图片", current: 0, total: 3 });
        await replaceAfterImageMutation((renderer, revision) => {
          const [centerX, centerY] = renderer.placementPoint;
          return pinBoardApi.importPinBoardClipboardImage(
            selected,
            bytes,
            centerX,
            centerY,
            revision,
          );
        }, (count) => `已从剪贴板导入 ${count} 张图片`);
        return;
      }

      // 路径来源只剩两条：剪贴板里的文件（由 `read_pin_board_clipboard_paths`
      // 读出并加入文件系统 scope）与文件选择器。粘贴纯文本路径不再导入——那种
      // 路径没有任何授权来源，Rust 层会以「未由文件选择器授权」拒绝。
      let paths: string[] = [];
      try {
        paths = (await pinBoardApi.readPinBoardClipboardPaths())
          .filter((path) => IMAGE_PATH_PATTERN.test(path));
      } catch {
        // Fall back to the file picker when the native clipboard is unavailable.
      }
      if (paths.length === 0) {
        paths = await pickPinBoardImagePaths();
        if (paths.length === 0) return;
      }
      transferCancelRef.current = false;
      setTransferProgress({
        label: "正在导入图片",
        current: 0,
        total: paths.length,
        cancellable: true,
      });
      await replaceAfterImageMutation((renderer, revision) => {
        const [centerX, centerY] = renderer.placementPoint;
        return pinBoardApi.importPinBoardImages(
          selected,
          paths,
          centerX,
          centerY,
          arrangementGapRatio,
          revision,
        );
      }, (count) => `已导入 ${count} 张图片`);
      // 取消时原生侧整笔回滚（不会留下半批条目），这里只把状态文案说清楚。
      if (transferCancelRef.current) setStatus("已取消导入");
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [arrangementGapRatio, rendererRef, replaceAfterImageMutation, selected, setStatus]);

  const importDroppedFiles = useCallback(async (
    files: FileList,
    clientX: number,
    clientY: number,
  ) => {
    const current = rendererRef.current;
    if (!current || selected === null || transferBusyRef.current) return;
    if (current.interactionState.locked || current.interactionState.exporting) {
      setStatus("画板已锁定，无法导入图片");
      return;
    }
    // 拖入没有经过画布的按下，编辑会话不会自行提交；先提交再导入，与「点击外部
    // 提交后照常执行这次动作」保持一致。
    const textError = current.commitTextEdit();
    if (textError) {
      setStatus(textError);
      return;
    }
    const images = Array.from(files).filter((file) => DROP_IMAGE_PATTERN.test(file.name));
    if (images.length === 0) {
      setStatus("请拖入 PNG、JPEG、WebP、BMP 或 GIF 图片");
      return;
    }
    transferBusyRef.current = true;
    transferOperationId.current = null;
    transferCancelRef.current = false;
    current.setPlacementFromClient(clientX, clientY);
    let [centerX, centerY] = current.placementPoint;
    try {
      let imported = 0;
      for (const file of images) {
        // 拖入的图片是前端逐张落库的，取消就是不再发起下一张（已落库的保留）。
        if (transferCancelRef.current) break;
        const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
        if (bytes.length === 0) continue;
        setTransferProgress({
          label: "正在导入图片",
          current: imported,
          total: images.length,
          cancellable: true,
        });
        const result = await replaceAfterImageMutation(
          (_renderer, revision) => pinBoardApi.importPinBoardClipboardImage(
            selected,
            bytes,
            centerX,
            centerY,
            revision,
          ),
          (count) => `已导入 ${count} 张图片`,
        );
        if (!result) break;
        imported += result.itemIds.length;
        const placed = result.view.items.find((image) => result.itemIds.includes(image.itemId));
        if (placed) {
          // 逐张落位时按「图高的比例」递进，与阵列间距同一口径。
          const placedWidth = Math.hypot(
            placed.points[1][0] - placed.points[0][0],
            placed.points[1][1] - placed.points[0][1],
          );
          const placedHeight = Math.hypot(
            placed.points[0][0] - placed.points[3][0],
            placed.points[0][1] - placed.points[3][1],
          );
          centerX += placedWidth + placedHeight * arrangementGapRatio;
        }
      }
      if (transferCancelRef.current) setStatus(`已取消导入，本次导入 ${imported} 张图片`);
      else if (imported > 0) setStatus(`已导入 ${imported} 张图片`);
    } catch (error) {
      setStatus(errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [arrangementGapRatio, rendererRef, replaceAfterImageMutation, selected, setStatus]);

  const addText = useCallback(async (
    text: string,
    fontSize: number,
    color: string,
    placement: [number, number],
  ) => {
    const current = rendererRef.current;
    if (!current
      || current.interactionState.locked
      || current.interactionState.textEditing
      || selected === null
      || transferBusyRef.current) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    setTransferProgress({ label: "正在添加文字", current: 0, total: 1 });
    try {
      // 度量与渲染共用 `textLayout`；Rust 只持久化度量结果与载荷，不生成图片。
      // 对话框不提供对齐选项，条目统一按居中排版（Rust 侧已校验取值）。
      const metrics = measurePinBoardText(text, fontSize);
      await replaceAfterImageMutation((_renderer, revision) => {
        const [centerX, centerY] = placement;
        return pinBoardApi.createPinBoardText(
          selected,
          text,
          fontSize,
          color,
          "center",
          metrics.width,
          metrics.height,
          centerX,
          centerY,
          revision,
        );
      }, () => "已添加文字");
    } catch (error) {
      setStatus(`无法添加文字：${errorMessage(error)}`);
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [rendererRef, replaceAfterImageMutation, selected, setStatus]);

  /**
   * 提交一笔刚画完的笔迹。
   *
   * 与文字条目同一条写入链路，但不复用 `replaceAfterImageMutation`：它会在成功后
   * 写一条状态栏文案，而画板状态栏是 34px 的独立一行——每画一笔弹一次会让画布
   * 上下跳动。失败仍然走状态栏。
   *
   * 串行化是必需的：`create_pin_board_stroke` 带 revision 校验，两笔并发落库会
   * 拿着同一个 revision，后一笔被判成「画板已被其他窗口修改」。
   */
  const commitStroke = useCallback((request: StrokeCommitRequest) => {
    const boardId = selectedBoardRef.current;
    if (boardId === null) return;
    strokeQueueRef.current = strokeQueueRef.current.then(async () => {
      const current = rendererRef.current;
      if (!current
        || current.currentBoardId !== boardId
        || current.interactionState.locked
        || current.interactionState.textEditing) {
        current?.clearPendingStrokePreview();
        return;
      }
      // 先落地未保存的变换：创建命令会推进 revision，若把内存里未写库的改动一起
      // 记成「已保存」，切换画板时它们就会被丢掉。
      if (!await current.save()) {
        current.clearPendingStrokePreview();
        return;
      }
      if (current.currentBoardId !== boardId) return;
      try {
        const result = await pinBoardApi.createPinBoardStroke(
          boardId,
          strokeToolId(request.tool),
          request.color,
          request.strokeWidth,
          request.width,
          request.height,
          request.geometry,
          request.centerX,
          request.centerY,
          current.boardRevision,
        );
        if (current.currentBoardId !== boardId) return;
        current.applyMutationResult(result.view, result.itemIds);
        setInteraction(current.interactionState);
      } catch (error) {
        if (current.currentBoardId === boardId) {
          // 提交失败就把预览撤掉，别在画布上留一条不存在的笔迹。
          current.clearPendingStrokePreview();
          setStatus(errorMessage(error));
        }
      }
    }).catch((error) => setStatus(errorMessage(error)));
  }, [rendererRef, selectedBoardRef, setInteraction, setStatus]);

  const exportImages = useCallback(async () => {
    const current = rendererRef.current;
    if (!current
      || current.interactionState.locked
      || current.interactionState.textEditing
      || selected === null
      || transferBusyRef.current) return;
    // 只导出位图：文字与笔迹的单条导出已删除，统一由整画布导出覆盖。
    const itemIds = current.exportableImageIds();
    if (itemIds.length === 0) return;
    transferBusyRef.current = true;
    transferOperationId.current = null;
    try {
      const directory = await pickPinBoardExportDirectory();
      if (!directory || !await current.save()) return;
      transferCancelRef.current = false;
      setTransferProgress({
        label: "正在导出图片",
        current: 0,
        total: itemIds.length,
        cancellable: true,
      });
      const count = await pinBoardApi.exportPinBoardImages(
        selected,
        itemIds,
        directory,
      );
      setStatus(`已导出 ${count} 个条目`);
    } catch (error) {
      setStatus(transferCancelRef.current ? "已取消导出" : errorMessage(error));
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
    }
  }, [rendererRef, selected, setStatus]);

  /**
   * 取消当前传输。
   *
   * 两条链路的取消方式不同：整画布导出由渲染器逐块检查标记（下一块之前停下并丢弃
   * 临时文件），导入/导出图片则由原生循环在条目边界检查；这里同时置位，多调一次
   * 原生取消是无害的（没有传输时它只留下一个会在下次开始时复位的标记）。
   */
  const cancelTransfer = useCallback(() => {
    transferCancelRef.current = true;
    void pinBoardApi.cancelPinBoardTransfer().catch(() => undefined);
  }, []);

  const canvasExportPlan = useCallback((scale: number, margin: number) => (
    rendererRef.current?.canvasExportPlan(scale, margin, canvasExportLimit) ?? null
  ), [canvasExportLimit, rendererRef]);

  const canvasExportAutoScale = useCallback((margin: number) => (
    rendererRef.current?.canvasExportAutoScale(margin, canvasExportLimit) ?? 1
  ), [canvasExportLimit, rendererRef]);

  /**
   * 整画布导出：可见内容离屏渲染成一块块 RGBA，由原生端流式拼成一张透明 PNG。
   *
   * 导出的是渲染器内存里的当前状态，**不落库**，因此整段流程不产生仓库写操作、
   * 也不取仓库写锁。期间渲染器置 `exporting`：画板锁定、指针与快捷键不响应。
   */
  const exportCanvas = useCallback(async (request: {
    directory: string;
    fileName: string;
    scale: number;
    margin: number;
  }) => {
    const current = rendererRef.current;
    if (!current
      || transferBusyRef.current
      || current.interactionState.locked
      || current.interactionState.textEditing) return;
    const plan = current.canvasExportPlan(request.scale, request.margin, canvasExportLimit);
    if (!plan) {
      setStatus("画板没有可导出的内容");
      return;
    }
    // 文件名由对话框给出，原生端会清掉非法字符并在重名时追加序号。
    const fileName = request.fileName.trim();
    transferBusyRef.current = true;
    transferCancelRef.current = false;
    setTransferProgress({
      label: "正在导出画板",
      current: 0,
      total: plan.totalTiles,
      cancellable: true,
    });
    try {
      const path = await current.exportCanvas(plan, {
        directory: request.directory,
        fileName,
        onProgress: (sent, total) => setTransferProgress({
          label: "正在导出画板",
          current: sent,
          total,
          cancellable: true,
        }),
        isCancelled: () => transferCancelRef.current,
      });
      setStatus(`已导出 ${plan.width}×${plan.height} 的 PNG：${path}`);
    } catch (error) {
      setStatus(transferCancelRef.current ? "已取消导出" : `导出失败：${errorMessage(error)}`);
    } finally {
      setTransferProgress(null);
      transferBusyRef.current = false;
      transferCancelRef.current = false;
      setInteraction(rendererRef.current?.interactionState ?? DEFAULT_PIN_BOARD_INTERACTION);
    }
  }, [canvasExportLimit, rendererRef, setInteraction, setStatus]);

  return {
    transferProgress,
    isTransferBusy,
    replaceAfterImageMutation,
    importImages,
    importDroppedFiles,
    addText,
    commitStroke,
    exportImages,
    cancelTransfer,
    exportCanvas,
    canvasExportPlan,
    canvasExportAutoScale,
  };
}
