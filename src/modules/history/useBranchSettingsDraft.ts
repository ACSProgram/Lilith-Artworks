import { useEffect, useRef, useState } from "react";
import type { ArtworkBranch, UpdateBranchBackupRequest } from "./types";

export type BranchSettingsSaveState = "saved" | "dirty" | "saving" | "error";

/**
 * 分支设置的草稿与 650ms 防抖保存状态机。
 *
 * 状态放在历史控制层而不是控件里：控件只负责把草稿画出来，保存时机（用户停顿、
 * 运行锁释放后重排）由这里统一决定。
 */
export function useBranchSettingsDraft(
  branch: ArtworkBranch,
  disabled: boolean,
  saveLocked: boolean,
  onSave: (request: UpdateBranchBackupRequest) => Promise<void>,
) {
  const [title, setTitle] = useState(branch.title);
  const [enabled, setEnabled] = useState(branch.backupEnabled);
  const [backupInterval, setBackupInterval] = useState(branch.backupIntervalMinutes);
  const [quick, setQuick] = useState(branch.backupQuickEnabled);
  const [sourcePath, setSourcePath] = useState(branch.sourcePath);
  const [saveState, setSaveState] = useState<BranchSettingsSaveState>("saved");
  const requestVersion = useRef(0);
  const persisted = useRef({
    id: branch.id,
    title: branch.title,
    enabled: branch.backupEnabled,
    interval: branch.backupIntervalMinutes,
    quick: branch.backupQuickEnabled,
    sourcePath: branch.sourcePath,
  });

  useEffect(() => {
    const previous = persisted.current;
    const branchChanged = previous.id !== branch.id;
    setTitle((current) => branchChanged || current === previous.title ? branch.title : current);
    setEnabled((current) => branchChanged || current === previous.enabled
      ? branch.backupEnabled
      : current);
    setBackupInterval((current) => branchChanged || current === previous.interval
      ? branch.backupIntervalMinutes
      : current);
    setQuick((current) => branchChanged || current === previous.quick
      ? branch.backupQuickEnabled
      : current);
    setSourcePath((current) => branchChanged || current === previous.sourcePath
      ? branch.sourcePath
      : current);
    persisted.current = {
      id: branch.id,
      title: branch.title,
      enabled: branch.backupEnabled,
      interval: branch.backupIntervalMinutes,
      quick: branch.backupQuickEnabled,
      sourcePath: branch.sourcePath,
    };
    requestVersion.current += 1;
    if (branchChanged) setSaveState("saved");
  }, [
    branch.backupEnabled,
    branch.backupIntervalMinutes,
    branch.backupQuickEnabled,
    branch.id,
    branch.sourcePath,
    branch.title,
  ]);

  // 没有工作文件的分支没有可读取的源，自动备份必须保持关闭：开关置灰、间隔禁用，
  // 保存时也强制写回 false（Rust 侧同样兜底），避免"看起来开着却什么也不做"。
  const hasSource = sourcePath.trim().length > 0;
  const effectiveEnabled = hasSource && enabled;

  const valid = title.trim().length > 0 && backupInterval >= 1 && backupInterval <= 10_080;
  const dirty = title.trim() !== branch.title
    || effectiveEnabled !== branch.backupEnabled
    || backupInterval !== branch.backupIntervalMinutes
    || quick !== branch.backupQuickEnabled
    || sourcePath !== branch.sourcePath;

  useEffect(() => {
    if (!dirty) {
      setSaveState("saved");
      return;
    }
    if (!valid) return;
    setSaveState("dirty");
    // 运行锁被后台任务占着时不提交：saveLocked 变回 false 后 effect 会重新计时。
    if (disabled || saveLocked) return;
    const version = ++requestVersion.current;
    const timer = window.setTimeout(() => {
      setSaveState("saving");
      void onSave({
        branchId: branch.id,
        title: title.trim(),
        expectedBackupEnabled: branch.backupEnabled,
        backupEnabled: effectiveEnabled,
        backupIntervalMinutes: backupInterval,
        backupQuickEnabled: quick,
        ...(sourcePath !== branch.sourcePath ? { sourcePath } : {}),
      }).then(() => {
        if (requestVersion.current === version) setSaveState("saved");
      }).catch(() => {
        setSourcePath(branch.sourcePath);
        if (requestVersion.current === version) setSaveState("error");
      });
    }, 650);
    return () => window.clearTimeout(timer);
  }, [backupInterval, branch.id, branch.sourcePath, dirty, disabled, effectiveEnabled, onSave, quick, saveLocked, sourcePath, title, valid]);

  return {
    title,
    setTitle,
    setEnabled,
    backupInterval,
    setBackupInterval,
    quick,
    setQuick,
    sourcePath,
    setSourcePath,
    hasSource,
    effectiveEnabled,
    saveState,
  };
}
