import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProxyStatus, SaveOutcome, UserConfig } from '@/contracts/types';

const CONFIG = {
  proxyMode: 'smart',
  proxyModeType: 'systemProxy',
  selectedServerId: '__direct__',
  servers: [],
  subscriptions: [{ id: 'sub-1', etag: 'new-from-scheduler' }],
} as unknown as UserConfig;

const getMock = vi.fn(async () => CONFIG);
const saveMock = vi.fn(
  async (
    config: UserConfig,
    _deferRestart: boolean,
    _baseVersion: string
  ): Promise<SaveOutcome> => ({ status: 'saved', version: 'saved-version', config })
);
const patchMock = vi.fn(async (patch: Partial<UserConfig>) => ({ ...CONFIG, ...patch }));
const pendingMarkerMock = vi.fn(async (_pending: boolean) => undefined);
const startMock = vi.fn(async () => undefined);
const stopMock = vi.fn(async () => undefined);
const statusMock = vi.fn(async (): Promise<ProxyStatus> => ({ running: false }));
const switchMock = vi.fn(async (_serverId: string) => undefined);
const cleanExitMock = vi.fn(async () => false);
const configSignals = new Set<() => void>();

vi.mock('@/ipc', () => ({
  api: {
    config: {
      get: () => getMock(),
      onChangedReady: async (listener: () => void) => {
        configSignals.add(listener);
        return () => { configSignals.delete(listener); };
      },
      save: (config: UserConfig, deferRestart: boolean, baseVersion: string) =>
        saveMock(config, deferRestart, baseVersion),
      patch: (patch: Partial<UserConfig>) => patchMock(patch),
      setStagedPending: (pending: boolean) => pendingMarkerMock(pending),
    },
    proxy: {
      start: () => startMock(),
      stop: () => stopMock(),
      getStatus: () => statusMock(),
    },
    server: {
      switch: (serverId: string) => switchMock(serverId),
    },
    window: { takeCleanExitFlag: () => cleanExitMock() },
  },
}));

import { withConfigWriteLock } from '@/lib/config-write-lock';
import { configBaseVersion, type StagedEntry } from '@/lib/staged-config';
import { acquireConfigSync, refreshConfig, useAppStore } from './app-store';
import { useStagedConfigStore } from './staged-config-store';

const ENTRY: StagedEntry = {
  id: 'setting:logLevel',
  kind: 'setting',
  label: 'log level',
  entityPath: ['logLevel'],
  nextValue: 'debug',
};

function seed(entries: StagedEntry[]): void {
  useAppStore.setState({ config: CONFIG, proxyStarting: false });
  useStagedConfigStore.setState({
    enabled: true,
    entries,
    baseVersion: configBaseVersion(CONFIG),
    baseline: CONFIG,
    hydrated: true,
    saveStatus: 'idle',
    conflict: null,
    conflictBaseline: null,
  });
}

beforeEach(async () => {
  // 排空写队列：**用生产 API 本身**，不用「仅供单测」的复位钩子（那种导出进产物、是公开契约）。
  await withConfigWriteLock(async () => {});
  getMock.mockClear();
  saveMock.mockClear();
  saveMock.mockImplementation(async (config) => ({
    status: 'saved',
    version: 'saved-version',
    config,
  }));
  patchMock.mockClear();
  patchMock.mockImplementation(async (patch) => ({ ...CONFIG, ...patch }));
  pendingMarkerMock.mockClear();
  startMock.mockClear();
  stopMock.mockClear();
  statusMock.mockReset();
  statusMock.mockResolvedValue({ running: false });
  switchMock.mockClear();
  cleanExitMock.mockClear();
  cleanExitMock.mockImplementation(async () => false);
  seed([]);
});

describe('配置保存与下一次启动的事务边界', () => {
  it('首次暂存恢复完成前不发布 config，且较新代际覆盖等待中的旧快照', async () => {
    const newer = {
      ...CONFIG,
      selectedServerId: 'server-newer',
    } as unknown as UserConfig;
    let releaseCleanExit!: (value: boolean) => void;
    cleanExitMock.mockImplementationOnce(
      () => new Promise<boolean>((resolve) => { releaseCleanExit = resolve; })
    );
    getMock.mockResolvedValueOnce(CONFIG).mockResolvedValueOnce(newer);
    useAppStore.setState({ config: null, configLoading: false });
    useStagedConfigStore.setState({
      entries: [],
      baseline: null,
      baseVersion: null,
      hydrated: false,
    });

    const first = useAppStore.getState().loadConfig(true);
    await vi.waitFor(() => expect(cleanExitMock).toHaveBeenCalledTimes(1));
    // 编辑入口以 config 是否存在为前提；gate 未决时不得先暴露没有 baseline 的配置。
    expect(useAppStore.getState().config).toBeNull();

    const second = useAppStore.getState().loadConfig(true);
    expect(getMock).toHaveBeenCalledTimes(1);
    releaseCleanExit(false);
    await Promise.all([first, second]);

    expect(useAppStore.getState().config).toBe(newer);
    expect(useStagedConfigStore.getState().hydrated).toBe(true);
    expect(useStagedConfigStore.getState().baseline).toBe(newer);
  });

  it('冷启动点连接时等待正在加载的配置和草稿恢复', async () => {
    useAppStore.setState({ config: null, configLoading: false });
    useStagedConfigStore.setState({ hydrated: false, baseline: null, baseVersion: null });
    let finishRead!: (config: UserConfig) => void;
    getMock.mockImplementationOnce(() => new Promise((resolve) => { finishRead = resolve; }));
    startMock.mockImplementationOnce(async () => {
      expect(useAppStore.getState().config).toBe(CONFIG);
      expect(useStagedConfigStore.getState().baseline).toBe(CONFIG);
      expect(useStagedConfigStore.getState().hydrated).toBe(true);
    });
    const release = acquireConfigSync({ scope: 'main' });
    try {
      await vi.waitFor(() => expect(getMock).toHaveBeenCalledOnce());
      const starting = useAppStore.getState().startProxy();
      expect(useAppStore.getState().proxyStarting).toBe(true);
      expect(startMock).not.toHaveBeenCalled();
      finishRead(CONFIG);
      await starting;
      expect(getMock).toHaveBeenCalledOnce();
      expect(startMock).toHaveBeenCalledOnce();
      expect(useAppStore.getState().proxyStarting).toBe(false);
    } finally {
      release();
    }
  });

  it('冷启动配置读取卡住时释放连接，迟到结果只恢复配置', async () => {
    vi.useFakeTimers();
    useAppStore.setState({ config: null, configLoading: false });
    let finishRead!: (config: UserConfig) => void;
    getMock.mockImplementationOnce(() => new Promise((resolve) => { finishRead = resolve; }));
    const release = acquireConfigSync({ scope: 'main' });
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(getMock).toHaveBeenCalledOnce();
      const starting = useAppStore.getState().startProxy();
      const timedOut = expect(starting).rejects.toMatchObject({ code: 'CONFIG_REFRESH_CANCELLED' });
      await useAppStore.getState().startProxy();
      expect(useAppStore.getState().proxyStarting).toBe(true);
      await vi.advanceTimersByTimeAsync(5_000);
      await timedOut;
      expect(useAppStore.getState().proxyStarting).toBe(false);
      expect(startMock).not.toHaveBeenCalled();
      expect(getMock).toHaveBeenCalledOnce();

      finishRead(CONFIG);
      await vi.advanceTimersByTimeAsync(0);
      expect(useAppStore.getState().config).toBe(CONFIG);
      expect(startMock).not.toHaveBeenCalled();
      await useAppStore.getState().startProxy();
      expect(startMock).toHaveBeenCalledOnce();
    } finally {
      release();
      vi.useRealTimers();
    }
  });

  it('停止态携带草稿时，连接严格先保存、清 marker，再启动', async () => {
    seed([ENTRY]);

    await useAppStore.getState().startProxy();

    expect(saveMock).toHaveBeenCalledTimes(1);
    expect(saveMock).toHaveBeenCalledWith(
      expect.objectContaining({ logLevel: 'debug' }),
      true,
      configBaseVersion(CONFIG)
    );
    expect(startMock).toHaveBeenCalledTimes(1);
    const saveOrder = saveMock.mock.invocationCallOrder[0];
    const clearOrder = pendingMarkerMock.mock.invocationCallOrder.slice(-1)[0];
    const startOrder = startMock.mock.invocationCallOrder[0];
    expect(clearOrder).toBeDefined();
    expect(saveOrder).toBeLessThan(clearOrder!);
    expect(clearOrder!).toBeLessThan(startOrder);
    expect(useStagedConfigStore.getState().entries).toEqual([]);
  });

  it('版本冲突时保持停止并保留草稿，不按旧磁盘配置启动', async () => {
    seed([ENTRY]);
    saveMock.mockResolvedValue({ status: 'conflict', diskVersion: 'newer' });

    await useAppStore.getState().startProxy();

    expect(startMock).not.toHaveBeenCalled();
    expect(useStagedConfigStore.getState().entries).toEqual([ENTRY]);
    expect(useStagedConfigStore.getState().saveStatus).toBe('saveFailed');
  });

  it('连接保留启动中直到读回新状态，较早发起的断开快照不可覆盖它', async () => {
    useAppStore.getState().setProxyStatus({ running: false });
    let finishOldRead!: (status: ProxyStatus) => void;
    let finishStartRead!: (status: ProxyStatus) => void;
    statusMock
      .mockImplementationOnce(() => new Promise((resolve) => { finishOldRead = resolve; }))
      .mockImplementationOnce(() => new Promise((resolve) => { finishStartRead = resolve; }));
    const oldRead = useAppStore.getState().refreshProxyStatus();
    const starting = useAppStore.getState().startProxy();
    await vi.waitFor(() => expect(statusMock).toHaveBeenCalledTimes(2));
    expect(startMock).toHaveBeenCalledOnce();
    expect(useAppStore.getState().proxyStarting).toBe(true);

    finishStartRead({ running: true, mainGeneration: 1 });
    await starting;
    expect(useAppStore.getState().proxyStarting).toBe(false);
    expect(useAppStore.getState().proxyStatus?.running).toBe(true);

    finishOldRead({ running: false });
    await oldRead;
    expect(useAppStore.getState().proxyStatus?.running).toBe(true);
  });

  it('连接前保存在途时产生新编辑，保留新草稿并拒绝按旧批次启动', async () => {
    seed([ENTRY]);
    let finishSave!: (outcome: SaveOutcome) => void;
    saveMock.mockImplementationOnce(
      () => new Promise<SaveOutcome>((resolve) => { finishSave = resolve; })
    );

    const starting = useAppStore.getState().startProxy();
    await vi.waitFor(() => expect(saveMock).toHaveBeenCalledTimes(1));
    const newer: StagedEntry = {
      ...ENTRY,
      nextValue: 'warn',
    };
    useStagedConfigStore.getState().stage(newer);
    finishSave({
      status: 'saved',
      version: 'saved-version',
      config: { ...CONFIG, logLevel: 'debug' } as unknown as UserConfig,
    });
    await starting;

    expect(startMock).not.toHaveBeenCalled();
    expect(useStagedConfigStore.getState().entries).toEqual([newer]);
    expect(useAppStore.getState().proxyStarting).toBe(false);
  });

  it('停止代理不是保存或放弃：未保存草稿原样保留', async () => {
    seed([ENTRY]);
    useAppStore.getState().setProxyStatus({ running: true });

    await useAppStore.getState().stopProxy();

    expect(stopMock).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().proxyStatus?.running).toBe(false);
    expect(saveMock).not.toHaveBeenCalled();
    expect(useStagedConfigStore.getState().entries).toEqual([ENTRY]);
  });

  it('即时编辑只发送本次字段补丁，并采用写后 fresh 读取的完整配置', async () => {
    const backendMerged = {
      ...CONFIG,
      logLevel: 'debug',
      subscriptions: [{ id: 'sub-1', etag: 'concurrent-newer' }],
    } as unknown as UserConfig;
    patchMock.mockResolvedValue(backendMerged);
    getMock.mockResolvedValueOnce(backendMerged);

    await useAppStore.getState().saveConfig({ logLevel: 'debug' });

    expect(patchMock).toHaveBeenCalledWith({ logLevel: 'debug' });
    expect(useAppStore.getState().config).toBe(backendMerged);
    expect(useAppStore.getState().config?.subscriptions?.[0]?.etag).toBe('concurrent-newer');
  });

  it('queues a return to Smart behind a pending Global write and preserves its error', async () => {
    getMock.mockResolvedValue(CONFIG);
    let rejectGlobal!: (error: unknown) => void;
    patchMock.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectGlobal = reject; }));
    const { updateProxyMode } = useAppStore.getState();
    const failure = { code: 'DNS_DEPENDENCY_INVALID' };
    const global = expect(updateProxyMode('global')).rejects.toBe(failure);
    const smart = updateProxyMode('smart');

    await vi.waitFor(() => expect(patchMock.mock.calls).toEqual([[{ proxyMode: 'global' }]]));
    rejectGlobal(failure);
    await global;
    await smart;
    expect(patchMock.mock.calls).toEqual([[{ proxyMode: 'global' }], [{ proxyMode: 'smart' }]]);
    expect(useAppStore.getState().config?.proxyMode).toBe('smart');
  });

  it('节点切换采用后端事务后的整份配置，不用旧前端快照乐观覆盖', async () => {
    const backendMerged = {
      ...CONFIG,
      selectedServerId: 'server-new',
      subscriptions: [{ id: 'sub-1', etag: 'scheduler-won-the-race' }],
    } as unknown as UserConfig;
    getMock.mockResolvedValueOnce(backendMerged);

    await useAppStore.getState().switchServer('server-new');

    expect(switchMock).toHaveBeenCalledWith('server-new');
    expect(getMock).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().config).toBe(backendMerged);
    expect(useAppStore.getState().selectedServerId).toBe('server-new');
    expect(useAppStore.getState().config?.subscriptions?.[0]?.etag).toBe(
      'scheduler-won-the-race'
    );
  });
});


describe('订阅元数据空信号重读最新磁盘且不引入待应用变更', () => {
  for (const draft of [false, true]) {
    it(`304/200等价刷新保留节点、选择和既有草稿（draft=${draft}）`, async () => {
      seed(draft ? [ENTRY] : []);
      const beforePending = useAppStore.getState().pendingChanges;
      const release = acquireConfigSync({ scope: 'main' });
      try {
        await refreshConfig();
        const baseline = useStagedConfigStore.getState().baseline;
        const latest = { ...CONFIG, subscriptions: [{ id: 'sub-1', etag: 'fresh-validator',
          lastUpdated: '2026-10-25T09:00:00Z', userInfo: { upload: 10, download: 20 } }] } as unknown as UserConfig;
        getMock.mockResolvedValue(latest);
        expect(configSignals.size).toBe(1);
        for (const signal of configSignals) signal();
        await refreshConfig();
        expect(useAppStore.getState().config).toBe(latest);
        expect(useAppStore.getState().servers).toEqual(CONFIG.servers);
        expect(useAppStore.getState().selectedServerId).toBe(CONFIG.selectedServerId);
        expect(useAppStore.getState().pendingChanges).toEqual(beforePending);
        expect(useStagedConfigStore.getState().entries).toEqual(draft ? [ENTRY] : []);
        if (draft) expect(useStagedConfigStore.getState().baseline).toEqual(baseline);
        else expect(useStagedConfigStore.getState().baseline).toBe(latest);
        expect(saveMock).not.toHaveBeenCalled();
        expect(patchMock).not.toHaveBeenCalled();
        expect(startMock).not.toHaveBeenCalled();
        expect(switchMock).not.toHaveBeenCalled();
      } finally { release(); }
    });
  }

  it('旧GET跨元数据signal回包不发布，尾读最新并保留草稿', async () => {
    seed([ENTRY]);
    const release = acquireConfigSync({ scope: 'main' });
    try {
      await refreshConfig();
      let resolveOld!: (value: UserConfig) => void;
      const latest = { ...CONFIG, subscriptions: [{ id: 'sub-1', etag: 'latest' }] } as unknown as UserConfig;
      getMock.mockImplementationOnce(() => new Promise<UserConfig>((resolve) => { resolveOld = resolve; }));
      const pending = refreshConfig({ fresh: true });
      await vi.waitFor(() => expect(resolveOld).toBeDefined());
      const stale = { ...CONFIG, subscriptions: [{ id: 'sub-1', etag: 'stale' }] } as unknown as UserConfig;
      getMock.mockResolvedValue(latest);
      const published: UserConfig[] = [];
      const unsubscribe = useAppStore.subscribe((state) => { if (state.config) published.push(state.config); });
      try {
        for (const signal of configSignals) signal();
        resolveOld(stale);
        await pending;
        await vi.waitFor(() => expect(useAppStore.getState().config).toBe(latest));
        expect(published).not.toContain(stale);
        expect(useStagedConfigStore.getState().entries).toEqual([ENTRY]);
        expect(useStagedConfigStore.getState().baseline).toEqual(CONFIG);
      } finally { unsubscribe(); }
    } finally { release(); }
  });
});


it('代际检查后新编辑已保存，迟到空信号只读取最新URL和手选且保留草稿', async () => {
  seed([ENTRY]);
  const release = acquireConfigSync({ scope: 'main' });
  try {
    getMock.mockResolvedValue(CONFIG);
    await refreshConfig();
    const baseline = useStagedConfigStore.getState().baseline;
    const pending = useAppStore.getState().pendingChanges;
    // This barrier models a completed newer backend edit between the producer's
    // short generation check and delivery of its payload-free invalidation.
    const newerEdit = {
      ...CONFIG,
      selectedServerId: 'user-selected-new',
      servers: [{ id: 'user-selected-new', name: 'User selection' }],
      subscriptions: [{ id: 'sub-1', url: 'https://example.invalid/new', etag: 'new-edit' }],
    } as unknown as UserConfig;
    getMock.mockResolvedValue(newerEdit);
    for (const signal of configSignals) signal();
    await refreshConfig();
    expect(useAppStore.getState().config).toBe(newerEdit);
    expect(useAppStore.getState().config?.subscriptions?.[0]?.url).toBe('https://example.invalid/new');
    expect(useAppStore.getState().selectedServerId).toBe('user-selected-new');
    expect(useAppStore.getState().servers).toEqual(newerEdit.servers);
    expect(useAppStore.getState().pendingChanges).toEqual(pending);
    expect(useStagedConfigStore.getState().entries).toEqual([ENTRY]);
    expect(useStagedConfigStore.getState().baseline).toEqual(baseline);
    expect(saveMock).not.toHaveBeenCalled();
    expect(patchMock).not.toHaveBeenCalled();
    expect(startMock).not.toHaveBeenCalled();
    expect(switchMock).not.toHaveBeenCalled();
  } finally { release(); }
});
