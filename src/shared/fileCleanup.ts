export interface CleanupFailure {
  id: string;
  path: string;
  error: string;
}

export interface CleanupReport {
  cleanedCount: number;
  pendingCount: number;
  failures: CleanupFailure[];
}
