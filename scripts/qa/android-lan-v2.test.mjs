import test from 'node:test';
import assert from 'node:assert/strict';
import { validatePlan,encodeFrame,decodeFrame,verifyAck,classifyObservations,collectDebugBatch,SCHEMA } from './android-lan-v2.mjs';

const secret = Buffer.from(Array.from({length:32},(_,i) => i));
const frame = {kind:'REQ',planSha:'a'.repeat(64),sessionId:'b'.repeat(32),nonce:'c'.repeat(48),instanceId:'d'.repeat(32),role:'data',protocol:'udp',sequence:1,challenge:'e'.repeat(32),deadline:5000};
test('request echo, role/key changes, malformed numbers and oversize cannot be ACK evidence',() => {
  const bytes = encodeFrame(secret,frame);
  assert.equal(bytes.toString().split('|').at(-1),'c63f3fefd5d7777831464c1c0a9a8afd1ad8255aa1eb1932d4b9ede205e2470e');
  assert.deepEqual(decodeFrame(secret,bytes),frame);
  assert.equal(verifyAck(secret,frame,bytes),false);
  assert.equal(verifyAck(secret,frame,encodeFrame(secret,{...frame,kind:'ACK'})),true);
  assert.equal(decodeFrame(Buffer.alloc(32),bytes),null);
  assert.equal(decodeFrame(secret,Buffer.from(bytes.toString().replace('|data|','|health|'))),null);
  assert.equal(decodeFrame(secret,Buffer.concat([bytes,Buffer.alloc(513)])),null);
});
test('plan rejects public/DNS peers, arbitrary extra fields, port and TTL drift',() => {
  const plan={peers:['192.168.10.1'],port:47100,ttlMillis:1000,apkSha256:'a'.repeat(64),expectedSourcePin:'b'.repeat(64),sessionSecret:'c'.repeat(64),runId:'run',birthNonce:'birth',nativeInputRevision:1};
  validatePlan(plan);
  for (const bad of [{peers:['example.com']},{peers:['8.8.8.8']},{port:80},{ttlMillis:300001},{arbitraryPath:'/tmp/file'}]) assert.throws(() => validatePlan({...plan,...bad}));
});
test('authenticated health and zero matched remains observed no match with attribution unknown',() => {
  const r=classifyObservations({tcp:{matched:0,health:2},udp:{matched:0,health:0}});
  assert.equal(r.tcpWitness,'ObservedNoMatch'); assert.equal(r.coreIngressAttribution,'Unknown'); assert.equal(r.policyNegative,'NotObserved');
  assert.ok(!Object.values(r).includes('PolicyRejected'));
});
test('collector rejects fixture sources, old APK and late A receipt for B',async () => {
  const report={schema:SCHEMA,source:'AppLive',apkSha256:'a'.repeat(64),expectedSourcePin:'b'.repeat(64),bootNonce:'c'.repeat(32),sessionId:'d'.repeat(32),nonce:'e'.repeat(48),tcp:{matched:1,health:0},udp:{matched:0,health:0}};
  const invoke=async () => ({success:true,data:JSON.stringify(report)});
  const expected={apkSha256:report.apkSha256,sourcePin:report.expectedSourcePin};
  assert.equal((await collectDebugBatch(invoke,{action:'snapshot',sessionId:report.sessionId},expected)).executionGate,'ActualDeviceAndIndependentPeerNotObserved');
  await assert.rejects(collectDebugBatch(invoke,{action:'snapshot',sessionId:'f'.repeat(32)},expected));
  await assert.rejects(collectDebugBatch(invoke,{action:'snapshot'},{...expected,apkSha256:'f'.repeat(64)}));
  await assert.rejects(collectDebugBatch(async()=>({success:true,data:JSON.stringify({...report,source:'Fixture'})}),{action:'snapshot'},expected));
});
