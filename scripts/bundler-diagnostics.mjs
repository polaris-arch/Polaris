#!/usr/bin/env node
// Observe the original Tauri invocation once. Never rerun a failed bundler.
// The workflow supplies the platform's CLI verbosity and an attempt-local log.
import { spawn } from 'node:child_process';
import { closeSync, openSync, writeFileSync } from 'node:fs';
import { constants } from 'node:os';

const log = process.env.POLARIS_TAURI_LOG;
if (!log) {
  console.error('POLARIS_TAURI_LOG must identify the diagnostic log');
  process.exit(1);
}
// Fail before building if the evidence destination cannot be opened.
const fd = openSync(log, 'w');

// Both child streams go to stdout and the log in one arrival order, like `2>&1 | tee`.
let logFailed = false;
const tee = (chunk) => {
  process.stdout.write(chunk);
  if (logFailed) return;
  try { writeFileSync(fd, chunk); } catch (error) {
    logFailed = true;
    console.error(`bundler-diagnostics: ${log}: ${error.message}`);
  }
};
const tauri = spawn('tauri', process.argv.slice(2), { stdio: ['inherit', 'pipe', 'pipe'] });
tauri.stdout.on('data', tee);
tauri.stderr.on('data', tee);
tauri.on('error', (error) => tee(`bundler-diagnostics: ${error.message}\n`));
tauri.on('close', (code, signal) => {
  closeSync(fd);
  // Keep the original Tauri failure, including when logging also fails. Otherwise a
  // logging failure must remain visible instead of claiming evidence was saved.
  const status = signal ? 128 + constants.signals[signal] : code < 0 ? 127 : code;
  process.exitCode = status || Number(logFailed);
});
