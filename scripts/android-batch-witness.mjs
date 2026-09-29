#!/usr/bin/env node
// PC-side controlled echo witness. The default CLI path never opens a socket.
import assert from 'node:assert/strict';
import { createServer, createConnection } from 'node:net';
import { createSocket } from 'node:dgram';
import { randomBytes } from 'node:crypto';
import { closeSync, ftruncateSync, openSync, readFileSync, writeSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PHASES, PROTOCOLS, challenge, expectedChallenges, requireApproval,
  sha256, validatePlan } from './android-batch-qa.mjs';

export class WitnessLedger {
  constructor(plan, side, protocol) {
    validatePlan(plan);
    assert.ok(['pc', 'android'].includes(side));
    assert.ok(PROTOCOLS.includes(protocol));
    this.plan = plan;
    this.protocol = protocol;
    this.instanceSha256 = sha256(randomBytes(32));
    this.received = 0;
    this.seen = new Set();
    this.alive = true;
    const direction = side === 'pc' ? 'android-to-pc' : 'pc-to-android';
    this.allowed = new Set(PHASES.flatMap(phase => expectedChallenges(plan, phase, direction, protocol)));
  }
  receive(payload) {
    // Loopback health does not alter data-plane counters; it has a distinct wire token.
    if (payload === `polaris-health-v1:${this.plan.nonce}:${this.protocol}\n`) return payload;
    this.received++;
    const value = sha256(payload);
    if (!this.allowed.has(value) || this.seen.has(value)) return null;
    this.seen.add(value);
    return payload;
  }
  snapshot() {
    return { instanceSha256: this.instanceSha256, nonceSha256: sha256(this.plan.nonce),
      protocol: this.protocol, port: this.plan[`${this.protocol}Port`], alive: this.alive,
      received: this.received, matched: this.seen.size, challengeSha256s: [...this.seen].sort() };
  }
}
function outcome(error) {
  if (error.code === 'ECONNREFUSED') return 'refused';
  if (error.code === 'ECONNRESET') return 'reset';
  if (error.code === 'QA_TIMEOUT') return 'timeout';
  return 'local-error'; // ENETUNREACH, DNS, bind and parsing errors never pass negative tests.
}
export function exchange(protocol, host, port, payload, timeoutMs) {
  return new Promise(resolveReply => {
    let finished = false;
    let timer;
    const socket = protocol === 'tcp' ? createConnection({ host, port }) : createSocket('udp4');
    const finish = reply => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      if (protocol === 'tcp') socket.destroy();
      else { try { socket.close(); } catch { /* close before UDP bind */ } }
      resolveReply(reply);
    };
    timer = setTimeout(() => finish({ outcome: 'timeout', ackSha256: null }), timeoutMs);
    socket.on('error', error => finish({ outcome: outcome(error), ackSha256: null }));
    if (protocol === 'tcp') {
      let response = '';
      socket.on('connect', () => socket.end(payload));
      socket.on('data', data => {
        response += data.toString('utf8');
        if (response.length > 1024) finish({ outcome: 'local-error', ackSha256: null });
      });
      socket.on('end', () => finish(response === payload
        ? { outcome: 'echo', ackSha256: sha256(response) }
        : { outcome: 'local-error', ackSha256: null }));
    } else {
      socket.on('message', (data, remote) => {
        if (remote.address !== host || remote.port !== port) return;
        const response = data.toString('utf8');
        finish(response === payload ? { outcome: 'echo', ackSha256: sha256(response) }
          : { outcome: 'local-error', ackSha256: null });
      });
      // Connect pins both source address and port, and reports ICMP refusal when available.
      socket.connect(port, host, () => socket.send(payload, error => {
        if (error) finish({ outcome: outcome(error), ackSha256: null });
      }));
    }
  });
}
export async function probe(plan, approval, phase, protocol, send = exchange) {
  requireApproval(plan, approval, 'traffic');
  assert.ok(PHASES.includes(phase) && PROTOCOLS.includes(protocol));
  const results = [];
  for (let i = 0; i < plan.attempts; i++) {
    // An expiring window cannot authorize the rest of a long-running batch.
    requireApproval(plan, approval, 'traffic');
    results.push(await send(protocol, plan.androidMeshIp, plan[`${protocol}Port`],
      challenge(plan, phase, 'pc-to-android', protocol, i), plan.timeoutMs));
    if (results.at(-1).outcome === 'local-error') break;
  }
  return { phase, direction: 'pc-to-android', protocol, attempted: results.length,
    outcomes: results.map(item => item.outcome), ackSha256s: results.map(item => item.ackSha256).filter(Boolean) };
}
export async function serve(plan, approval, counterFile) {
  requireApproval(plan, approval, 'witness');
  // The witness belongs to the local endpoint; existing mesh forwarding reaches loopback.
  // Never bind a physical LAN address or alter routes/firewall/PC endpoint configuration.
  const fd = openSync(counterFile, 'wx', 0o600);
  const ledgers = Object.fromEntries(PROTOCOLS.map(protocol => [protocol, new WitnessLedger(plan, 'pc', protocol)]));
  const sockets = [];
  let closed = false;
  const save = () => {
    const body = Buffer.from(`${JSON.stringify(Object.fromEntries(PROTOCOLS.map(p => [p, ledgers[p].snapshot()])), null, 2)}\n`);
    ftruncateSync(fd, 0);
    writeSync(fd, body, 0, body.length, 0);
  };
  const connections = new Set();
  let lease;
  const stop = () => {
    if (closed) return;
    closed = true;
    clearTimeout(lease);
    for (const connection of connections) connection.destroy();
    for (const socket of sockets) { try { socket.close(); } catch { /* partially bound */ } }
    for (const ledger of Object.values(ledgers)) ledger.alive = false;
    save();
    closeSync(fd);
    process.removeListener('SIGINT', stop);
    process.removeListener('SIGTERM', stop);
  };
  try {
    const tcp = createServer({ allowHalfOpen: true }, connection => {
      connections.add(connection);
      connection.on('close', () => connections.delete(connection));
      connection.on('error', () => connection.destroy());
      connection.setTimeout(plan.timeoutMs, () => connection.destroy());
      let data = '';
      connection.on('data', chunk => {
        data += chunk.toString('utf8');
        if (data.length > 1024) connection.destroy();
      });
      connection.on('end', () => {
        if (closed) return;
        const reply = ledgers.tcp.receive(data);
        save();
        connection.end(reply ?? '');
      });
    });
    sockets.push(tcp);
    await new Promise((ok, fail) => { tcp.once('error', fail); tcp.listen(plan.tcpPort, '127.0.0.1', ok); });
    tcp.on('error', stop);
    const udp = createSocket('udp4');
    sockets.push(udp);
    udp.on('message', (data, remote) => {
      if (closed || data.length > 1024) return;
      const reply = ledgers.udp.receive(data.toString('utf8'));
      save();
      if (reply) udp.send(reply, remote.port, remote.address, error => { if (error) stop(); });
    });
    await new Promise((ok, fail) => { udp.once('error', fail); udp.bind(plan.udpPort, '127.0.0.1', ok); });
    udp.on('error', stop);
    save();
    lease = setTimeout(stop, Math.min(900000, approval.expiresAtMs - Date.now()));
    process.once('SIGINT', stop);
    process.once('SIGTERM', stop);
    return { stop, ready: { state: 'READY', planSha256: plan.planSha256, leaseMs: Math.min(900000, approval.expiresAtMs - Date.now()) } };
  } catch (error) { stop(); throw error; }
}
export function androidArgs(plan, approval, mode, options) {
  requireApproval(plan, approval, mode === 'serve' ? 'witness' : 'traffic');
  assert.equal(plan.package, 'com.polaris2.app.debug', 'Android witness requires Debug run-as; Release needs a separate reviewed witness');
  const tail = ['--execute', plan.planSha256, String(approval.expiresAtMs)];
  if (mode === 'serve') {
    assert.equal(options.length, 0);
    const seconds = Math.min(900, Math.floor((approval.expiresAtMs - Date.now()) / 1000));
    assert.ok(seconds > 0, 'witness lease expired');
    return ['serve', plan.nonce, String(plan.tcpPort), String(plan.udpPort), String(plan.attempts),
      `cache/polaris-qa-${plan.nonce}/counters.json`, String(seconds), ...tail];
  }
  if (mode === 'probe') {
    const [phase, protocol] = options;
    assert.ok(options.length === 2 && PHASES.includes(phase) && PROTOCOLS.includes(protocol));
    return ['probe', plan.nonce, phase, protocol, plan.pcMeshIp, String(plan[`${protocol}Port`]),
      String(plan.attempts), String(plan.timeoutMs), 'android-to-pc', ...tail];
  }
  assert.ok(mode === 'health' && options.length === 1 && PROTOCOLS.includes(options[0]));
  return ['health', plan.nonce, options[0], String(plan[`${options[0]}Port`]), String(plan.timeoutMs), ...tail];
}
export async function main(args) {
  const [mode, planFile, approvalFile, ...options] = args;
  assert.ok(['serve', 'probe', 'android-serve', 'android-probe', 'android-health'].includes(mode));
  const plan = validatePlan(JSON.parse(readFileSync(planFile, 'utf8')));
  const execute = options.at(-1) === '--execute';
  if (execute) options.pop();
  const android = mode.startsWith('android-');
  assert.equal(options.length, android ? ({ 'android-serve': 0, 'android-probe': 2, 'android-health': 1 })[mode]
    : mode === 'serve' ? 1 : 2, 'wrong witness arguments');
  if (mode === 'probe') assert.ok(PHASES.includes(options[0]) && PROTOCOLS.includes(options[1]));
  if (!execute) return { mode: 'DRY_RUN', action: mode, planSha256: plan.planSha256,
    scope: mode.endsWith('serve') ? 'witness' : 'traffic', args: options,
    prerequisite: 'approved existing PC mesh path; no socket has been opened' };
  const approval = JSON.parse(readFileSync(approvalFile, 'utf8'));
  if (android) return { mode: 'APPROVED_COMMAND_ONLY', argv: androidArgs(plan, approval, mode.slice(8), options),
    note: 'No ADB command has run. Use only the matching package/serial and reviewed dex jar.' };
  if (mode === 'probe') return probe(plan, approval, ...options);
  return (await serve(plan, approval, options[0])).ready;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { console.log(JSON.stringify(await main(process.argv.slice(2)), null, 2)); }
  catch (error) { console.error(`FAIL: ${error.message.split('\n')[0]}`); process.exitCode = 1; }
}
