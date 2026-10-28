import { execFileSync } from 'node:child_process';
export const DOWNLOAD_TIMEOUT_MS = 240_000;
/** Bound attempts, slow transfers, retries and the entire child, including backoff. */
export function boundedDownload(url, options = {}, run = execFileSync) {
  const { output, ...processOptions } = options;
  if (new URL(url).protocol !== 'https:') throw new Error('HTTPS download required');
  const args = ['--disable', '--fail', '--location', '--silent', '--show-error',
    '--proto', '=https', '--proto-redir', '=https', '--connect-timeout', '20',
    '--max-time', '90', '--speed-limit', '1024', '--speed-time', '30',
    '--retry', '2', '--retry-max-time', '210'];
  if (output !== undefined) args.push('--output', output);
  args.push(url);
  return run('curl', args, { ...processOptions, timeout: DOWNLOAD_TIMEOUT_MS, killSignal: 'SIGKILL' });
}
