import { createHmac, timingSafeEqual, randomBytes } from 'node:crypto';

export const SCHEMA = 'polaris-android-lan-v2';
export function validatePlan(plan) {
  const keys = ['peers', 'port', 'ttlMillis', 'apkSha256', 'expectedSourcePin', 'sessionSecret', 'runId', 'birthNonce', 'nativeInputRevision'];
  if (Object.keys(plan).sort().join() !== keys.sort().join()) throw Error('finite plan fields required');
  const literal = (ip) => {
    if (typeof ip !== 'string' || !/^(0|[1-9]\d{0,2})(\.(0|[1-9]\d{0,2})){3}$/.test(ip)) return false;
    const [a,b,c,d] = ip.split('.').map(Number);
    return [a,b,c,d].every(n => n <= 255) && d > 0 && d < 255 && (a === 10 || a === 172 && b >= 16 && b <= 31 || a === 192 && b === 168);
  };
  if (!Array.isArray(plan.peers) || plan.peers.length < 1 || plan.peers.length > 2 || new Set(plan.peers).size !== plan.peers.length || !plan.peers.every(literal)) throw Error('approved literal RFC1918 peers required');
  if (!Number.isInteger(plan.port) || plan.port < 47100 || plan.port > 47115 || !Number.isSafeInteger(plan.ttlMillis) || plan.ttlMillis < 1000 || plan.ttlMillis > 300000) throw Error('finite port/TTL required');
  for (const field of ['apkSha256','expectedSourcePin','sessionSecret']) if (!/^[0-9a-f]{64}$/.test(plan[field])) throw Error('pinned private handoff required');
  if (!Number.isSafeInteger(plan.nativeInputRevision) || plan.nativeInputRevision <= 0 || !plan.runId || !plan.birthNonce) throw Error('actual input revision required');
  return plan;
}
function canonical(f) {
  return ['2',f.kind,f.planSha,f.sessionId,f.nonce,f.instanceId,f.role,'reachability','peer-to-app',f.protocol,String(f.sequence),f.challenge,String(f.deadline)].join('|');
}
function key(secret, f) { return createHmac('sha256',secret).update(`polaris-lan-v2/key/${f.role}/peer-to-app/${f.protocol}`).digest(); }
export function encodeFrame(secret, frame) {
  const body = canonical(frame);
  const bytes = Buffer.from(`${body}|${createHmac('sha256',key(secret,frame)).update(body).digest('hex')}`,'ascii');
  if (bytes.length > 512) throw Error('frame too large');
  return bytes;
}
export function decodeFrame(secret, bytes) {
  if (bytes.length === 0 || bytes.length > 512 || [...bytes].some(n => n < 32 || n > 126)) return null;
  const p = bytes.toString('ascii').split('|');
  if (p.length !== 14 || p[0] !== '2' || !['REQ','ACK'].includes(p[1]) || p[7] !== 'reachability' || p[8] !== 'peer-to-app' || !['data','health'].includes(p[6]) || !['tcp','udp'].includes(p[9])) return null;
  if (!/^[0-9a-f]{64}$/.test(p[2]) || !/^[0-9a-f]{32}$/.test(p[3]) || !/^[0-9a-f]{48}$/.test(p[4]) || !/^[0-9a-f]{32}$/.test(p[5]) || !/^[0-9a-f]{32}$/.test(p[11])) return null;
  const f = {kind:p[1],planSha:p[2],sessionId:p[3],nonce:p[4],instanceId:p[5],role:p[6],protocol:p[9],sequence:Number(p[10]),challenge:p[11],deadline:Number(p[12])};
  if (!Number.isSafeInteger(f.sequence) || f.sequence < 1 || f.sequence > 20 || !Number.isSafeInteger(f.deadline) || f.deadline <= 0) return null;
  const encoded = encodeFrame(secret,f);
  return encoded.length === bytes.length && timingSafeEqual(encoded,bytes) ? f : null;
}
export function challenge(report, protocol, role, sequence) {
  if (!['tcp','udp'].includes(protocol) || !['health','data'].includes(role) || !Number.isInteger(sequence) || sequence < 1 || sequence > 20) throw Error('finite challenge required');
  return {kind:'REQ',planSha:report.planSha,sessionId:report.sessionId,nonce:report.nonce,
    instanceId:protocol === 'tcp' ? report.tcpInstanceId : report.udpInstanceId,role,protocol,sequence,
    challenge:randomBytes(16).toString('hex'),deadline:report.deadlineElapsedRealtime};
}
export function verifyAck(secret, request, reply) {
  const ack = decodeFrame(secret,reply);
  return ack !== null && canonical(ack) === canonical({...request,kind:'ACK'});
}
/** Source facts and runtime observations stay separate; health+zero is not policy rejection. */
export function classifyObservations(report) {
  return {tcpWitness:report.tcp.matched > 0 ? 'ObservedPass' : report.tcp.health > 0 ? 'ObservedNoMatch' : 'NotObserved',
    udpAppSocket:report.udp.matched > 0 ? 'ObservedPass' : report.udp.health > 0 ? 'ObservedNoMatch' : 'NotObserved',
    witnessEvidenceScope:'AppSocketAuthenticatedRequestReceived', independentPeerAck:'NotObserved',
    coreIngressAttribution:'Unknown', policyNegative:'NotObserved', tcpCoreOutbound:'NotObserved', handover:'Deferred',
    vpnUnderlying:'Unknown', completeOsVpnScope:'Unknown', dns:'Unknown', mainNativeCleanup:'Unknown'};
}
/** invoke is the existing authenticated Tauri Debug bridge, not an app_process/socket factory. */
export async function collectDebugBatch(invoke, args, expected) {
  if (!['prepare','arm','snapshot','probe','health','close','cleanupObserve'].includes(args.action)) throw Error('unknown batch action');
  const response = await invoke('debug_android_batch_qa',args);
  if (response?.success !== true || typeof response.data !== 'string' || response.data.length > 65536) throw Error('batch bridge unavailable');
  const report = JSON.parse(response.data);
  if (report.schema !== SCHEMA || report.source !== 'AppLive' || report.apkSha256 !== expected.apkSha256 || report.expectedSourcePin !== expected.sourcePin ||
      !/^[0-9a-f]{32}$/.test(report.bootNonce) || !/^[0-9a-f]{32}$/.test(report.sessionId) || !/^[0-9a-f]{48}$/.test(report.nonce) ||
      args.sessionId && report.sessionId !== args.sessionId) throw Error('actual app provenance mismatch');
  if (expected.sessionId && (report.sessionId !== expected.sessionId || report.bootNonce !== expected.bootNonce || report.nonce !== expected.nonce)) throw Error('late or different batch');
  return {sourceFacts:report,observations:classifyObservations(report),executionGate:'ActualDeviceAndIndependentPeerNotObserved'};
}
