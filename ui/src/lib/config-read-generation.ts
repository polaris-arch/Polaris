// Per-webview ordering for complete configuration snapshots. Starting a newer
// read or publishing any configuration invalidates every older read, even when
// the selection fields are unchanged. Backend events refresh each webview.
let generation = 0;
export const configReadGeneration = {
  beginRead: () => ++generation,
  published: () => { generation++; },
  current: () => generation,
  isCurrent: (read: number) => read === generation,
};
