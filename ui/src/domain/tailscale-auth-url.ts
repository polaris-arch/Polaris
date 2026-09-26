/** Control servers may choose any host; only web URLs are suitable for browser authorization. */
export function validatedTailscaleAuthUrl(raw: string | null | undefined): string | null {
  if (!raw) return null;
  try {
    const url = new URL(raw.trim());
    return (url.protocol === 'https:' || url.protocol === 'http:') && url.hostname
      ? raw.trim() : null;
  } catch {
    return null;
  }
}
