/** Subscription metadata is untrusted; preserve UTC dates and zero = no expiry. */
export function subscriptionExpiryDate(seconds: number | null | undefined): string | null {
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds <= 0) return null;
  const date = new Date(seconds * 1000);
  return Number.isFinite(date.getTime()) ? date.toISOString().split('T')[0] : null;
}
