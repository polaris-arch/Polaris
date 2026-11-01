import { describe, it, expect, afterEach, vi } from 'vitest';
import { createTopicSubscription, type TopicPort } from './topic-subscription';

/** 记录调用顺序的假 port；subscribe/unsubscribe 的 promise 由测试手动 resolve（模拟慢 IPC）。 */
function fakePort() {
  const calls: string[] = [];
  let emit: ((d: number) => void) | null = null;
  let resolveSub: (() => void) | null = null;
  const port: TopicPort<number> = {
    onFrame(cb) {
      calls.push('listen');
      emit = cb;
      return () => calls.push('unlisten');
    },
    subscribe() {
      calls.push('subscribe');
      return new Promise<void>((res) => {
        resolveSub = res;
      });
    },
    unsubscribe() {
      calls.push('unsubscribe');
      return Promise.resolve();
    },
  };
  return {
    port,
    calls,
    emit: (d: number) => emit?.(d),
    resolveSubscribe: () => resolveSub?.(),
  };
}

describe('createTopicSubscription', () => {
  it('监听在订阅之前挂上（否则后端首拍那一帧会被丢）', () => {
    const f = fakePort();
    createTopicSubscription(f.port, () => {}).setWanted(true);
    expect(f.calls).toEqual(['listen', 'subscribe']);
    // 变异：把 onFrame 挪到 subscribe 之后 → 顺序变 ['subscribe','listen'] → 本条转红。
  });

  it('subscribe 尚未 resolve 就 dispose，仍然退订', async () => {
    // 守的缺陷：订阅态若等 .then() 才置位，快速进出页面 → 退订永不发出 → 后端计数漏一个、
    // poller 不停机且签名残留 → 下次进页面等到内容变化才有帧（真机表现：拓扑不显示/滞后）。
    const f = fakePort();
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    sub.dispose(); // IPC 还在飞
    expect(f.calls).toEqual(['listen', 'subscribe', 'unlisten', 'unsubscribe']);
    f.resolveSubscribe();
    await Promise.resolve();
    expect(f.calls.filter((c) => c === 'unsubscribe')).toHaveLength(1);
  });

  it('同值重复 setWanted 不产生额外 IPC', () => {
    const f = fakePort();
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    sub.setWanted(true);
    sub.setWanted(false);
    sub.setWanted(false);
    expect(f.calls).toEqual(['listen', 'subscribe', 'unsubscribe']);
  });

  it('dispose 之后 setWanted 不再发 IPC，dispose 幂等', () => {
    const f = fakePort();
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    sub.dispose();
    sub.dispose();
    sub.setWanted(true);
    expect(f.calls).toEqual(['listen', 'subscribe', 'unlisten', 'unsubscribe']);
  });

  it('未订阅就 dispose：只注销监听，不发空退订', () => {
    const f = fakePort();
    createTopicSubscription(f.port, () => {}).dispose();
    expect(f.calls).toEqual(['listen', 'unlisten']);
  });

  it('帧经监听回调透传', () => {
    const f = fakePort();
    const got: number[] = [];
    const sub = createTopicSubscription(f.port, (d) => got.push(d));
    sub.setWanted(true);
    f.emit(42);
    expect(got).toEqual([42]);
  });
});

describe('native listener readiness', () => {
  it('waits for registration and applies latest demand only', async () => {
    const f = fakePort();
    let ready!: (off: () => void) => void;
    f.port.onFrame = () => new Promise((resolve) => { ready = resolve; });
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    sub.setWanted(false);
    expect(f.calls).toEqual([]);
    ready(() => f.calls.push('unlisten'));
    await Promise.resolve();
    expect(f.calls).toEqual([]);
    sub.setWanted(true);
    expect(f.calls).toEqual(['subscribe']);
    sub.dispose();
    expect(f.calls).toEqual(['subscribe', 'unlisten', 'unsubscribe']);
  });
  it('disposal during registration prevents subscribe and releases late listener', async () => {
    const f = fakePort();
    let ready!: (off: () => void) => void;
    f.port.onFrame = () => new Promise((resolve) => { ready = resolve; });
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    sub.dispose();
    ready(() => f.calls.push('unlisten'));
    await Promise.resolve();
    expect(f.calls).toEqual(['unlisten']);
  });
  it('registration failure never starts producer', async () => {
    const f = fakePort();
    f.port.onFrame = () => Promise.reject(new Error('registration failed'));
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    await Promise.resolve(); await Promise.resolve();
    sub.dispose();
    expect(f.calls).toEqual([]);
  });
});

describe('bounded recovery after native failure', () => {
  afterEach(() => vi.useRealTimers());

  it('recovers a rejected subscribe once while keeping listener-before-producer order', async () => {
    vi.useFakeTimers();
    const f = fakePort();
    let attempts = 0;
    f.port.subscribe = () => {
      f.calls.push('subscribe');
      return ++attempts === 1 ? Promise.reject(new Error('transport')) : Promise.resolve();
    };
    const got: number[] = [];
    const sub = createTopicSubscription(f.port, value => got.push(value));
    sub.setWanted(true);
    await vi.advanceTimersByTimeAsync(1000);
    f.emit(42);
    expect(f.calls).toEqual(['listen', 'subscribe', 'subscribe']);
    expect(got).toEqual([42]);
    sub.dispose();
    expect(f.calls.slice(-2)).toEqual(['unlisten', 'unsubscribe']);
  });

  it('stops automatic retries after one recovery attempt and permits renewed demand', async () => {
    vi.useFakeTimers();
    const f = fakePort();
    f.port.subscribe = () => { f.calls.push('subscribe'); return Promise.reject(new Error('unavailable')); };
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(f.calls.filter(call => call === 'subscribe')).toHaveLength(2);
    expect(vi.getTimerCount()).toBe(0);
    sub.setWanted(false);
    sub.setWanted(true);
    expect(f.calls.filter(call => call === 'subscribe')).toHaveLength(3);
    sub.dispose();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(f.calls.filter(call => call === 'subscribe')).toHaveLength(3);
  });

  it('cancels a scheduled retry on disposal and still releases attempted native ownership', async () => {
    vi.useFakeTimers();
    const f = fakePort();
    f.port.subscribe = () => { f.calls.push('subscribe'); return Promise.reject(new Error('transport')); };
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    await Promise.resolve();
    expect(vi.getTimerCount()).toBe(1);
    sub.dispose();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(f.calls).toEqual(['listen', 'subscribe', 'unlisten', 'unsubscribe']);
  });

  it('ignores a stale rejected subscribe after a newer demand was accepted', async () => {
    vi.useFakeTimers();
    const f = fakePort();
    let rejectOld!: (error: Error) => void;
    let attempts = 0;
    f.port.subscribe = () => {
      f.calls.push('subscribe');
      return ++attempts === 1 ? new Promise((_, reject) => { rejectOld = reject; }) : Promise.resolve();
    };
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    sub.setWanted(false);
    sub.setWanted(true);
    rejectOld(new Error('old transport'));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(attempts).toBe(2);
    sub.dispose();
  });

  it('retries failed listener registration before subscribing, using the same finite budget', async () => {
    vi.useFakeTimers();
    const f = fakePort();
    let registrations = 0;
    f.port.onFrame = () => {
      f.calls.push('listen');
      return ++registrations === 1
        ? Promise.reject(new Error('event transport'))
        : Promise.resolve(() => f.calls.push('unlisten'));
    };
    const sub = createTopicSubscription(f.port, () => {});
    sub.setWanted(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.calls).toEqual(['listen', 'listen', 'subscribe']);
    sub.dispose();
  });
});
