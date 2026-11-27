import { spawnSync } from 'node:child_process';
import { closeSync, openSync, readSync } from 'node:fs';
import { cpus, loadavg } from 'node:os';
import { join } from 'node:path';

const GIB = 1024 ** 3;
const SAMPLE_BYTES = 16_384;
const digits = value => /^[0-9]+$/.test(value);
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
export const exactJSONInteger = value => value <= BigInt(Number.MAX_SAFE_INTEGER)
  ? Number(value) : JSON.rawJSON(String(value));

function capacityText(path) {
  let fd;
  try {
    fd = openSync(path, 'r');
    const raw = Buffer.alloc(SAMPLE_BYTES + 1);
    let at = 0;
    while (at < raw.length) {
      const count = readSync(fd, raw, at, raw.length - at);
      if (count === 0) return utf8.decode(raw.subarray(0, at));
      at += count;
    }
  } catch (error) {
    if (!error.code && !(error instanceof TypeError)) throw error;
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
  return '';
}

function controllerPaths(root, reported) {
  const parts = reported.split('/').filter(part => part && part !== '.');
  if (parts.includes('..')) return [];
  return Array.from({ length: parts.length + 1 }, (_, n) => join(root, ...parts.slice(0, parts.length - n)));
}

function linuxAffinity(read) {
  const row = /^Cpus_allowed_list:\s*([^\s]+)/m.exec(read('/proc/self/status'));
  if (!row) return null;
  let count = 0, previous = -1;
  for (const range of row[1].split(',')) {
    const match = /^([0-9]+)(?:-([0-9]+))?$/.exec(range);
    if (!match) return null;
    const start = Number(match[1]), end = Number(match[2] ?? match[1]);
    if (!Number.isSafeInteger(end) || start <= previous || end < start) return null;
    count += end - start + 1;
    previous = end;
  }
  return Number.isSafeInteger(count) && count > 0 ? count : null;
}

function linuxCapacity(facts, read) {
  let availableMemory;
  function memory(bytes, source, kind) {
    if (availableMemory !== undefined && bytes >= availableMemory) return;
    availableMemory = bytes;
    facts.memorySource = source;
    if (kind) {
      facts.osMemoryObservationBytes = exactJSONInteger(bytes);
      facts.osMemoryObservationKind = kind;
    }
  }
  function observeCPU(quota, period, unlimited, version) {
    if (!(quota === unlimited || digits(quota)) || !digits(period) || BigInt(period) === 0n) return;
    if (quota !== unlimited) {
      const count = Number(quota) / Number(period);
      if (!Number.isFinite(count)) return;
      facts.quotaCPUs = facts.quotaCPUs === null ? count : Math.min(facts.quotaCPUs, count);
    }
    facts.cgroupVersion = version;
    facts.quotaObserved = true;
  }
  function observeMemory(path, limit, usage, version) {
    if (!(digits(limit) || (version === 2 && limit === 'max')) || !digits(usage)) return;
    facts.cgroupMemoryObserved = true;
    if (limit === 'max') return;
    const maximum = BigInt(limit), used = BigInt(usage);
    const available = maximum > used ? maximum - used : 0n;
    facts.cgroupMemoryConstraints.push({
      version, path, limitBytes: exactJSONInteger(maximum), usageBytes: exactJSONInteger(used),
      headroomBytes: exactJSONInteger(available),
    });
    memory(available, `cgroup-v${version}-headroom`);
    facts.cgroupVersion = version;
  }

  for (const row of read('/proc/meminfo').split('\n')) {
    const [key, count, unit, extra] = row.trim().split(/\s+/);
    if (key === 'MemAvailable:' && digits(count) && unit === 'kB' && extra === undefined) {
      memory(BigInt(count) * 1024n, 'proc-meminfo-MemAvailable', 'kernel-MemAvailable-estimate');
      break;
    }
  }
  const cgroups = read('/proc/self/cgroup').split('\n');
  const unified = cgroups.find(row => row.startsWith('0::'));
  for (const path of controllerPaths('/sys/fs/cgroup', unified?.slice(3) ?? '')) {
    const cpu = read(join(path, 'cpu.max')).trim().split(/\s+/);
    if (cpu.length === 2) observeCPU(...cpu, 'max', 2);
    observeMemory(path, read(join(path, 'memory.max')).trim(), read(join(path, 'memory.current')).trim(), 2);
  }
  const controllers = cgroups.map(row => /^\d+:([^:]+):(.*)$/.exec(row)).filter(Boolean);
  for (const [, names, leaf] of controllers) {
    if (!names.split(',').includes('cpu')) continue;
    for (const root of ['/sys/fs/cgroup/cpu', '/sys/fs/cgroup/cpu,cpuacct']) {
      for (const path of controllerPaths(root, leaf)) {
        observeCPU(read(join(path, 'cpu.cfs_quota_us')).trim(), read(join(path, 'cpu.cfs_period_us')).trim(), '-1', 1);
      }
    }
  }
  for (const [, names, leaf] of controllers) {
    if (!names.split(',').includes('memory')) continue;
    for (const path of controllerPaths('/sys/fs/cgroup/memory', leaf)) {
      observeMemory(path, read(join(path, 'memory.limit_in_bytes')).trim(), read(join(path, 'memory.usage_in_bytes')).trim(), 1);
    }
  }
  return availableMemory;
}

function vmStat() {
  const result = spawnSync('/usr/bin/vm_stat', [], {
    timeout: 2000, maxBuffer: SAMPLE_BYTES, env: { PATH: '/usr/bin:/bin', LC_ALL: 'C' },
  });
  return result.status === 0 && result.stdout.length <= SAMPLE_BYTES && !result.stdout.some(byte => byte > 127)
    ? result.stdout.toString('ascii') : '';
}

function darwinCapacity(facts, observe) {
  facts.quotaObserved = true;
  const [header, ...rows] = observe().split('\n');
  const size = /page size of ([0-9]+) bytes/.exec(header);
  if (!size || BigInt(size[1]) === 0n) return;
  const pages = new Map();
  for (const row of rows) {
    const fields = /^([^:]+):\s*([0-9]+)\.*\s*$/.exec(row);
    if (fields) pages.set(fields[1], BigInt(fields[2]));
  }
  const available = ['Pages free', 'Pages inactive', 'Pages speculative'].map(name => pages.get(name));
  if (available.some(value => value === undefined)) return;
  const bytes = available.reduce((sum, value) => sum + value, 0n) * BigInt(size[1]);
  facts.osMemoryObservationBytes = exactJSONInteger(bytes);
  facts.osMemoryObservationKind = 'reclaimable-page-subset-not-complete-available-memory';
  facts.memorySource = 'vm-stat-free-inactive-speculative';
  return bytes;
}

// Capacity estimates constrain concurrency. The filesystem lease is separate.
export function capacityLimit(facts, memory = facts.availableMemoryBytes) {
  const cpuCandidates = [facts.logicalCPUs, facts.affinityCPUs, facts.quotaCPUs]
    .filter(value => Number.isFinite(value) && value > 0);
  const effectiveCPUs = cpuCandidates.length ? Math.max(1, Math.floor(Math.min(...cpuCandidates))) : 1;
  const loadJobCap = facts.load1 === null ? 1 : Math.max(1, effectiveCPUs - Math.ceil(facts.load1));
  const estimate = memory === null ? null : Math.max(0,
    Number((BigInt(memory) - BigInt(facts.memoryHeadroomBytes)) / BigInt(facts.memoryPerJobBytes)));
  const memoryJobCap = Math.max(1, estimate ?? 0);
  const memoryEstimateRisk = estimate === null ? 'unknown-memory'
    : estimate === 0 ? 'below-unmeasured-single-job-estimate' : 'within-unmeasured-concurrency-estimate';
  return {
    ...facts, effectiveCPUs, loadJobCap, memoryJobCap, estimatedMemoryJobCap: estimate, memoryEstimateRisk,
    memoryPolicy: 'concurrency-guidance-only; no memory admission or reservation',
    peakRSSBytes: null,
    jobCap: Math.min(effectiveCPUs, loadJobCap, memoryJobCap),
    unknown: [
      ...(!cpuCandidates.length ? ['cpu'] : []),
      ...(facts.load1 === null ? ['load'] : []),
      ...(memory === null ? ['memory'] : []),
    ],
  };
}

export function nativeCapacity({
  platform = process.platform, read = capacityText, observeVM = vmStat,
  logicalCPUs = cpus().length || null,
  affinityCPUs = platform === 'linux' ? linuxAffinity(read) : null,
  load1 = loadavg()[0],
} = {}) {
  const facts = {
    schema: 'polaris-native-runner-capacity-v1',
    scope: 'current runner only; no cross-runner admission',
    memoryPerJobBytes: 2 * GIB, memoryHeadroomBytes: GIB,
    logicalCPUs, affinityCPUs, quotaCPUs: null,
    load1: Number.isFinite(load1) && load1 >= 0 ? load1 : null,
    availableMemoryBytes: null, memorySource: null, cgroupVersion: null,
    quotaObserved: false, cgroupMemoryObserved: false, osMemoryObservationBytes: null,
    osMemoryObservationKind: null, cgroupMemoryConstraints: [],
  };
  let memory;
  if (platform === 'linux') memory = linuxCapacity(facts, read);
  if (platform === 'darwin') memory = darwinCapacity(facts, observeVM);
  if (memory !== undefined) facts.availableMemoryBytes = exactJSONInteger(memory);
  const result = capacityLimit(facts, memory ?? null);
  if (platform === 'linux' && !facts.quotaObserved) {
    result.jobCap = 1;
    result.unknown.push('cpu-quota');
  }
  if (platform === 'linux' && !facts.cgroupMemoryObserved) {
    result.jobCap = 1;
    result.unknown.push('container-memory');
  }
  // Hosted macOS load1 (14-88 on 3 CPUs) and the vm_stat page subset each pin the cap to 1.
  // Both stay recorded; an observed darwin host keeps at least min(CPUs, 3) jobs.
  if (platform === 'darwin' && memory !== undefined) result.jobCap = Math.max(result.jobCap, Math.min(result.effectiveCPUs, 3));
  return result;
}

export function nativeEnvironment(env = process.env, observation = nativeCapacity()) {
  const { jobCap } = observation;
  if (!Number.isSafeInteger(jobCap) || jobCap < 1) {
    throw new Error('native compiler concurrency observation must be a positive integer');
  }
  const child = { ...env }, appliedEnvironment = {};
  for (const key of ['CARGO_BUILD_JOBS', 'GOMAXPROCS']) {
    const prior = child[key];
    if (prior !== undefined && (typeof prior !== 'string' || !digits(prior) || BigInt(prior) < 1n)) {
      throw new Error('explicit native compiler concurrency must be a positive integer');
    }
    appliedEnvironment[key] = prior === undefined ? jobCap : Math.min(jobCap, Number(prior));
    child[key] = String(appliedEnvironment[key]);
  }
  return { env: child, observation: { ...observation, appliedEnvironment } };
}
