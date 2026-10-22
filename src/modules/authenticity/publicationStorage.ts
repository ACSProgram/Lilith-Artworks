import type { CertificationConfig } from "./types";

const SHARED_SIGNING_KEY = "lilith-artworks.certification-signing-v1";

/** 把用户输入的私钥整理成标准 PEM：缺首尾行时补全，正文按 64 列换行。 */
export function privateKeyPem(value: string): string {
  const trimmed = value.trim();
  const pem = trimmed.match(/-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----([\s\S]*?)-----END \1-----/i);
  const label = pem?.[1]?.toUpperCase() || "PRIVATE KEY";
  const body = (pem?.[2] || trimmed).replace(/\s+/g, "");
  const lines = body.match(/.{1,64}/g)?.join("\n") || "";
  return `-----BEGIN ${label}-----\n${lines}\n-----END ${label}-----`;
}

export function loadSharedSigning(): Partial<CertificationConfig> {
  try { return JSON.parse(localStorage.getItem(SHARED_SIGNING_KEY) ?? "{}"); }
  catch { return {}; }
}

export function saveSharedSigning(config: CertificationConfig): void {
  localStorage.setItem(SHARED_SIGNING_KEY, JSON.stringify({
    certificatePath: config.certificatePath,
    signingAlgorithm: config.signingAlgorithm,
    timestampUrl: config.timestampUrl,
  }));
}

export function clearSharedSigning(): void {
  localStorage.removeItem(SHARED_SIGNING_KEY);
}
