import { MobileInfo } from '../MobileInfo';
import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端**加一份规则资源**的两条腿：从内置目录挑（[`ResCatalogPanel`]）与按 URL 下载
 * （[`ResUrlPanel`]）。桌面对位是 `dialogs/ResCatalogDialog.tsx` 与 `dialogs/ResUrlDialog.tsx`。
 *
 * # 为什么一个文件两支
 *
 * 它们在界面上是并排的两颗按钮、回答的是同一个问题（「这份规则集从哪来」），且共用同一条写腿
 * （`api.ruleResources.download`）与同一组结果处置。分成两个文件会让那条共用的结果处置
 * 要么重复两遍、要么再抽一个只有两处调用的模块。
 *
 * # 复用的是判据
 *
 * 目录侧：`@/domain/rule-resource-catalog` 的 `catalogTabItems`（**排序在搜索之前**、
 * external 先排掉与 builtin 重号的条目）、`catalogItemStatus`（随包优先于已下载）、
 * `catalogEmptyKind`（四态判定序不可调）、`categoryLabel`。
 * URL 侧：`@/components/dialogs/res-url-logic` 的 `inferResource` / `validateResUrl` /
 * `validateResName`。
 *
 * ⚠️ **推断函数选的是 `inferResource`，不是 `deriveResourceMeta`**：仓里并存两个（后者认得
 * meta-rules-dat 的 `-lite` 路径与 ASN 编号），而桌面这颗按钮用的是前者。移动端跟桌面同一个，
 * 免得同一个 URL 在两端推出两个不同的名字/分类。
 *
 * # 写失败
 *
 * 走表单宿主那条机制（`FormSheet` 的 `notice` 槽 + `setNotice`）。两支都**不在失败时关表**：
 * 用户的下一步（换一个 URL、少选几项重试）就在这张表里。
 */

import { useEffect, useMemo, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import type {
  RuleResourceCatalogItem,
  RuleResourceCategory,
  RuleResourceDownloadItem,
} from '@/contracts/types';
import {
  catalogEmptyKind,
  catalogItemStatus,
  catalogTabItems,
  categoryLabel,
} from '@/domain/rule-resource-catalog';
import {
  inferResource,
  validateResName,
  validateResUrl,
} from '@/components/dialogs/res-url-logic';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';
import { resourceUpdateOutcome } from '@/domain/resource-update-outcome';

/**
 * 一次下载的**结果处置**（纯函数，两支共用）。
 *
 * 三档分开不是话术：空数组 = 后端没受理，逐条 `ok:false` = 受理了但这几条失败，全成功才关表。
 * 把前两档并成一句「失败了」，用户分不出该换 URL 还是该等一等。
 *
 * 🔴 **发起调用那一行不在这里**，两支各自在自己的 `try` 里调 `api.ruleResources.download`。
 * 这不是冗余：跨屏门（`mobile/write-failure-visibility.test.ts`）按「写腿落在一个 try 里、
 * 且那个 try 的 catch 真的调了本层的 reporter（`setNotice`）」判，而 reporter 是**面板的** state
 * setter —— 把调用抽进模块级函数，它就落在了任何 catch 的辖区之外，失败时用户看不到任何东西。
 * 判据只有一份的是**结果怎么读**，那正是这个函数。
 */
export function downloadOutcome(
  results: unknown,
  t: (key: string) => string,
  expectedCount = Array.isArray(results) ? results.length : 0,
): { ok: true } | { ok: false; text: string } {
  const outcome = resourceUpdateOutcome(results, expectedCount);
  if (outcome.status === 'empty') {
    /*
     * 🔴 **不用桌面那条 `resCatalog.errUnavailable`**（「下载后端尚未接入」）。
     * 那是**桌面自己**的一条缺席声明，把它渲染到移动端等于把桌面的债记到移动端的账上 ——
     * 接线完成度门的 A 面对这批键有明文收窄（「桌面自己也没接」的同形文案不算移动端的账），
     * 而收窄的条件是「移动端源码里没有消费点」：在这里消费它，就把它拽进了移动端的账。
     * 这一档的事实是「后端没受理这次请求」，用中性的失败文案说它。
     */
    return { ok: false, text: t('errors.operationFailed') };
  }
  if (outcome.status !== 'success') {
    return { ok: false, text: t('resCatalog.downloadAllFailed') };
  }
  return { ok: true };
}

export function ResCatalogPanel({ instanceId }: { instanceId: string }): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);

  const [tab, setTab] = useState<'builtin' | 'external'>('builtin');
  const [builtin, setBuiltin] = useState<RuleResourceCatalogItem[]>([]);
  const [external, setExternal] = useState<RuleResourceCatalogItem[] | null>(null);
  const [downloadedIds, setDownloadedIds] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [query, setQuery] = useState('');
  const [sel, setSel] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [loading, setLoading] = useState(true);
  const [loadErr, setLoadErr] = useState(false);
  const [extBusy, setExtBusy] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        /* 🔴 预载那条**必须就地 catch**（同桌面）：缓存目录读不到是常态（从没拉过外置清单），
           让它 reject 会把整个面板打成错误态，而内置目录其实好好的。 */
        const [catalog, list, cached] = await Promise.all([
          api.ruleResources.getCatalog(),
          api.ruleResources.list(),
          api.ruleResources.getCachedCatalog().catch(() => null),
        ]);
        if (!live) return;
        setBuiltin(catalog?.items ?? []);
        setDownloadedIds(new Set((Array.isArray(list) ? list : []).map((item) => item.id)));
        if (cached?.items != null) setExternal(cached.items);
        setLoadErr(false);
      } catch (err) {
        console.error('[mobile-res-catalog] load failed:', err);
        if (!live) return;
        /* 两条可见通道都给：列表位置换成失败文案（`emptyKind === 'error'`），脚上方再落一条
           notice —— 前者答「这个列表为什么是空的」，后者答「刚才那次操作怎么了」。 */
        setLoadErr(true);
        setNotice({ tone: 'err', text: tr('resCatalog.loadFailed') });
      } finally {
        if (live) setLoading(false);
      }
    })();
    return () => {
      live = false;
    };
  }, []);

  const refreshExternal = (): void => {
    setExtBusy(true);
    setNotice(undefined);
    void (async () => {
      try {
        const catalog = await api.ruleResources.refreshCatalog();
        if (hasInstance(instanceId)) setExternal(catalog?.items ?? []);
      } catch (err) {
        console.error('[mobile-res-catalog] refresh failed:', err);
        if (hasInstance(instanceId)) {
          setNotice({ tone: 'err', text: tr('resCatalog.loadFailed') });
        }
      } finally {
        if (hasInstance(instanceId)) setExtBusy(false);
      }
    })();
  };

  /* 排序（已具备在前）**先于**搜索：颠倒过来，搜出来的那几条会按原始顺序排，与不搜时不一致。 */
  const tabItems = useMemo(
    () => catalogTabItems(tab, builtin, external ?? [], downloadedIds),
    [tab, builtin, external, downloadedIds],
  );
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === '') return tabItems;
    return tabItems.filter(
      (item) =>
        item.name.toLowerCase().includes(needle) || item.id.toLowerCase().includes(needle),
    );
  }, [tabItems, query]);

  /** 已具备（随包 / 已下载）的条目恒显勾选，但**不计入下载目标** —— 再下一遍是白跑一趟。 */
  const targets = useMemo(
    () => filtered.filter((item) => sel.has(item.id) && catalogItemStatus(item, downloadedIds) === null),
    [filtered, sel, downloadedIds],
  );

  const emptyKind = catalogEmptyKind({
    error: loadErr ? 'err' : null,
    notFetched: tab === 'external' && external === null,
    total: tabItems.length,
    count: filtered.length,
  });

  const submit = (): void => {
    if (submitting || targets.length === 0) return;
    setSubmitting(true);
    setNotice(undefined);
    void (async () => {
      try {
        const items: RuleResourceDownloadItem[] = targets.map((item) => ({
          catalogId: item.id,
          name: item.name,
          category: item.category,
        }));
        const outcome = downloadOutcome(await api.ruleResources.download(items), tr, items.length);
        if (outcome.ok) {
          closeInstance(instanceId);
          toast.success(tr('resCatalog.downloaded'));
          return;
        }
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: outcome.text });
      } catch (err) {
        console.error('[mobile-res-catalog] download failed:', err);
        if (hasInstance(instanceId)) {
          setNotice({ tone: 'err', text: tr('resCatalog.downloadAllFailed') });
        }
      } finally {
        if (hasInstance(instanceId)) setSubmitting(false);
      }
    })();
  };

  return (
    <FormSheet
      title={tr('resCatalog.title')}
      onRequestClose={() => {
        if (!submitting) closeInstance(instanceId);
      }}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      /* 计数**恒渲染**（同桌面）：条件渲染会让按钮在选中第一项时忽然变宽，整排脚跟着跳一下。 */
      submitLabel={`${tr('resCatalog.download')} (${targets.length})`}
      submitDisabled={submitting || targets.length === 0}
      onSubmit={submit}
      notice={notice}
    >
      <div className="m-form-foot">
        <button
          type="button"
          className={tab === 'builtin' ? 'm-form-btn primary' : 'm-form-btn'}
          aria-pressed={tab === 'builtin'}
          onClick={() => setTab('builtin')}
        >
          {tr('resCatalog.builtin')}
        </button>
        <button
          type="button"
          className={tab === 'external' ? 'm-form-btn primary' : 'm-form-btn'}
          aria-pressed={tab === 'external'}
          onClick={() => setTab('external')}
        >
          {tr('resCatalog.external')}
        </button>
      </div>

      <div className="m-form-row">
        <input
          className="m-form-input"
          value={query}
          placeholder={tr('resCatalog.searchPh')}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      {tab === 'external' && (
        <button type="button" className="m-form-btn" disabled={extBusy} onClick={refreshExternal}>
          {extBusy ? tr('resCatalog.extFetching') : tr('resCatalog.refreshList')}
        </button>
      )}

      {loading ? (
        <p className="m-form-hint">{tr('common.loading')}</p>
      ) : emptyKind !== null ? (
        <p className="m-form-hint">
          {emptyKind === 'error'
            ? tr('resCatalog.loadFailed')
            : emptyKind === 'notFetched'
              ? tr('resCatalog.clickRefresh')
              : tr('resCatalog.noMatch')}
        </p>
      ) : (
        <div className="mr-sel-list" role="listbox" aria-label={tr('resCatalog.title')} aria-multiselectable>
          {filtered.map((item) => {
            const status = catalogItemStatus(item, downloadedIds);
            const checked = sel.has(item.id) || status !== null;
            return (
              <button
                key={item.id}
                type="button"
                className="mr-sel-opt"
                role="option"
                aria-selected={checked}
                /* 已具备的条目点不动：它已经在那儿了，勾掉/再勾都不改变任何事。 */
                disabled={status !== null}
                onClick={() =>
                  setSel((prev) => {
                    const next = new Set(prev);
                    if (next.has(item.id)) next.delete(item.id);
                    else next.add(item.id);
                    return next;
                  })
                }
              >
                <span className="mr-sel-opt-tx">
                  <span>{item.name}</span>
                  <span className="mr-sheet-item-sub">
                    {categoryLabel(item.category, tr('resources.categoryCustom'))}
                    {status === 'bundled'
                      ? ` · ${tr('resCatalog.bundled')}`
                      : status === 'downloaded'
                        ? ` · ${tr('resCatalog.downloaded')}`
                        : ''}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      )}
    </FormSheet>
  );
}

export function ResUrlPanel({ instanceId }: { instanceId: string }): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);

  const [url, setUrl] = useState('');
  const [name, setName] = useState('');
  /** 手改过的字段不再被 URL 推断覆盖 —— 覆盖一个用户刚打进去的名字比不推断更坏。 */
  const [nameDirty, setNameDirty] = useState(false);
  const [category, setCategory] = useState<RuleResourceCategory>('custom');
  const [catDirty, setCatDirty] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  const dirty = url.trim() !== '' || name.trim() !== '';

  const onUrlChange = (next: string): void => {
    setUrl(next);
    const inferred = inferResource(next);
    if (!nameDirty) setName(inferred.name);
    if (!catDirty) setCategory(inferred.category);
  };

  const requestClose = (): void => {
    if (submitting) return;
    if (!dirty) {
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: tr('resources.urlDlgDiscardTitle'),
        message: tr('resources.urlDlgDiscardMsg'),
        confirmLabel: tr('resources.urlDlgDiscard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          closeInstance(instanceId);
        },
      },
    });
  };

  const submit = (): void => {
    if (submitting) return;
    const urlError = validateResUrl(url);
    if (urlError !== null) {
      setNotice({
        tone: 'err',
        text: tr(
          urlError === 'urlEmpty'
            ? 'resources.urlDlgErrUrlEmpty'
            : 'resources.urlDlgErrUrlInvalid',
        ),
      });
      return;
    }
    if (validateResName(name) !== null) {
      setNotice({ tone: 'err', text: tr('resources.urlDlgErrNameEmpty') });
      return;
    }
    setSubmitting(true);
    setNotice(undefined);
    void (async () => {
      try {
        const items: RuleResourceDownloadItem[] = [
          { url: url.trim(), name: name.trim(), category },
        ];
        const outcome = downloadOutcome(await api.ruleResources.download(items), tr, items.length);
        if (outcome.ok) {
          closeInstance(instanceId);
          toast.success(tr('resCatalog.downloaded'));
          return;
        }
        if (hasInstance(instanceId)) {
          setNotice({ tone: 'err', text: outcome.text });
        }
      } catch (err) {
        console.error('[mobile-res-url] download failed:', err);
        if (hasInstance(instanceId)) {
          setNotice({ tone: 'err', text: tr('resources.urlDlgErrFailed') });
        }
      } finally {
        if (hasInstance(instanceId)) setSubmitting(false);
      }
    })();
  };

  return (
    <FormSheet
      title={tr('resources.urlDownload')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      submitLabel={tr('resources.urlDownload')}
      submitDisabled={submitting}
      onSubmit={submit}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mru-url">
          {tr('resources.urlDlgUrlLabel')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mru-url"
          className="m-form-input mono"
          value={url}
          placeholder="https://example/geosite-cn.srs"
          onChange={(e) => onUrlChange(e.target.value)}
        />
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mru-name">
          {tr('resources.urlDlgNameLabel')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="mru-name"
          className="m-form-input"
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            setNameDirty(true);
          }}
        />
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mru-cat">
          {tr('resources.urlDlgCatLabel')}
        </label>
        <MobileSelect
          id="mru-cat"
          className="m-form-select"
          value={category}
          onChange={(e) => {
            setCategory(e.target.value as RuleResourceCategory);
            setCatDirty(true);
          }}
        >
          <option value="geosite">{tr('resources.urlDlgCatGeosite')}</option>
          <option value="geoip">{tr('resources.urlDlgCatGeoip')}</option>
          {/* 两档 lite 在桌面就是字面量（不是 i18n 键）：它们是上游 meta-rules-dat 的分区名。 */}
          <option value="geosite-lite">Geosite Lite</option>
          <option value="geoip-lite">GeoIP Lite</option>
          <option value="custom">{tr('resources.urlDlgCatCustom')}</option>
        </MobileSelect>
      </div>

      <div className="m-form-hint"><MobileInfo title={tr('resources.urlDlgTitle')} summary={tr('mobileHelp.resourceUrl')} details={tr('resources.urlDlgHint')} /></div>
    </FormSheet>
  );
}
