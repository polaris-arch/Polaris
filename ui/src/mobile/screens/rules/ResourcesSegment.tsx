import type { ReactElement } from 'react';
import { fmtBytes } from '@/components/screens/shared/format';
import { relativeTimeTextIso } from '@/lib/relative-time';
import {
  ActionSheet,
  EmptyState,
  InlineError,
  MoreIcon,
  ProgressBar,
  RefreshIcon,
  type SheetAction,
} from './Primitives';
export type ResourceSource = 'all' | 'builtin' | 'external';

export interface ResourceRowModel {
  id: string;
  name: string;
  builtin: boolean;
  /** 由规则集反算的引用数（0 = 未引用）。内置资源恒 0，用「系统」变体表达。 */
  references: number;
  /** 下载中的完成比（0–1）；不在下载中为 undefined。 */
  progress?: number;
  /** 上一次更新失败 ⇒ 主动作文案换成「立即重试」。 */
  failed?: boolean;
  /** 已取消（非失败语义，不用 warn 色）。 */
  cancelled?: boolean;
  /** 文件字节数（`RuleResource.size`，`data-contract.json#sources.rule-resources`）。 */
  size?: number;
  /** 上一次成功下载的 ISO 时间戳（`RuleResource.downloadedAt`，同一条源）。 */
  updatedAt?: string;
}

export interface ResourcesSegmentProps {
  t: (key: string, vars?: Record<string, unknown>) => string;
  source: ResourceSource;
  onSourceChange: (next: ResourceSource) => void;
  loading: boolean;
  error: boolean;
  groups: ReadonlyArray<{ key: string; label: string; rows: readonly ResourceRowModel[] }>;
  updatingAll: boolean;
  updatingIds: ReadonlySet<string>;
  onUpdateAll: () => void;
  /** 重置内置资源（原地二次确认；`resetConfirming` 决定文案与 `.confirming` 类）。 */
  onResetBuiltin: () => void;
  resetConfirming: boolean;
  onUpdateOne: (id: string) => void;
  onCancel: (id: string) => void;
  onDelete: (id: string) => void;
  errorOf: (key: string) => string | undefined;
  /** 处于「再点一次即删」待定态的资源 id。 */
  deleteConfirmingId: string | null;
  /** 打开了动作面板的资源 id（单点持有：同屏两个面板同时开是误触面）。 */
  sheetId: string | null;
  onOpenSheet: (id: string | null) => void;

  /** 从内置目录挑一份资源加进来（开表单宿主的 `res-catalog` 那一层）。 */
  onCatalog: () => void;
  /** 按 URL 下载一份外部资源（开表单宿主的 `res-url` 那一层）。 */
  onUrlDownload: () => void;
  autoUpdatePolicy: string;
  onAutoUpdateSettings: () => void;
}


/** Mobile presentation: MobileRulesScreen still owns every
 * resource read, mutation, confirmation, staged edit and error decision. */
export function ResourcesSegment({
  t,
  source,
  onSourceChange,
  loading,
  error,
  groups,
  updatingAll,
  updatingIds,
  onUpdateAll,
  onResetBuiltin,
  resetConfirming,
  onUpdateOne,
  onCancel,
  onDelete,
  errorOf,
  deleteConfirmingId,
  sheetId,
  onOpenSheet,
  onCatalog,
  onUrlDownload,
  autoUpdatePolicy,
  onAutoUpdateSettings,
}: ResourcesSegmentProps): ReactElement {
  const rowCount = groups.reduce((count, group) => count + group.rows.length, 0);
  const sources = [
    { id: 'all' as const, label: t('resources.src.all') },
    { id: 'builtin' as const, label: t('resources.src.builtin') },
    { id: 'external' as const, label: t('resources.src.external') },
  ];

  return (
    <section className="rc-section" aria-label={t('mobileRules.seg.resources')}>
      <div className="rc-toolbar">
        <div className="rc-toolbar-head">
          <div className="rc-sources" role="group" aria-label={t('resources.srcFilter')}>
            {sources.map((option) => (
              <button
                key={option.id}
                type="button"
                className="rc-source"
                aria-pressed={source === option.id}
                onClick={() => onSourceChange(option.id)}
              >
                {option.label}
              </button>
            ))}
          </div>
          <span className="rc-count" aria-label={`${t('resources.srcFilter')} ${rowCount}`}>{rowCount}</span>
        </div>
        <div className="rc-toolbar-actions">
          <button type="button" className="rc-action" onClick={onAutoUpdateSettings}>
            {t('resources.autoUpdateSettings')}
          </button>
          <button type="button" className="rc-action rc-action-primary" onClick={onCatalog}>
            {t('resources.catalog')}
          </button>
          <button type="button" className="rc-action" onClick={onUrlDownload}>
            {t('resources.urlDownload')}
          </button>
          <button type="button" className="rc-action rc-action-update" disabled={updatingAll || loading || error || rowCount === 0} onClick={onUpdateAll}>
            <RefreshIcon />
            {t('resources.updateAll')}
          </button>
        </div>
        <div className="rc-toolbar-foot">
          <span className="rc-group-note">{autoUpdatePolicy}</span>
          <button
            type="button"
            className={`rc-reset${resetConfirming ? ' confirming' : ''}`}
            onClick={onResetBuiltin}
          >
            {resetConfirming ? t('resources.resetBuiltinConfirm') : t('resources.resetBuiltin')}
          </button>
          <InlineError text={errorOf('res:all')} />
          <InlineError text={errorOf('res:reset')} />
          <InlineError text={errorOf('res:settings')} />
        </div>
      </div>

      {loading ? (
        <EmptyState text={t('resources.loading')} />
      ) : error ? (
        <EmptyState text={t('resources.loadError')} />
      ) : rowCount === 0 ? (
        <EmptyState text={t('resources.empty')} />
      ) : (
        <div className="rc-groups">
          {groups.map((group) => (
            <section className="rc-group" key={group.key} aria-label={group.label}>
              <h3 className="rc-group-title">
                <span>{group.label}</span>
                <small>{group.rows.length}</small>
              </h3>
              {group.rows.some((row) => row.builtin) && (
                <p className="rc-group-note">{t('resources.refSystemBaseline')}</p>
              )}
              <div className="rc-list">
                {group.rows.map((row) => {
                  const downloading = row.progress !== undefined;
                  const confirming = deleteConfirmingId === row.id;
                  const actions: Array<SheetAction | 'separator'> = downloading
                    ? [{ id: 'cancel', label: t('resources.cancel'), onSelect: () => onCancel(row.id) }]
                    : [
                        {
                          id: 'update',
                          label: row.failed ? t('resources.retryNow') : t('resources.update'),
                          icon: <RefreshIcon />,
                          disabled: updatingAll || updatingIds.has(row.id),
                          onSelect: () => { if (!updatingAll && !updatingIds.has(row.id)) onUpdateOne(row.id); },
                        },
                        ...(row.builtin ? [] : [{
                          id: 'delete',
                          label: confirming
                            ? row.references > 0
                              ? t('resources.deleteConfirmRefd', { count: row.references })
                              : t('resources.deleteConfirmPlain')
                            : t('common.delete'),
                          danger: true,
                          confirming,
                          onSelect: () => onDelete(row.id),
                        } satisfies SheetAction]),
                      ];
                  return (
                    <article className="rc-row" key={row.id}>
                      <div className="rc-row-body">
                        <div className="rc-title-line">
                          <h4>{row.name}</h4>
                          <span className="rc-origin">
                            {row.builtin ? t('resources.src.builtin') : t('resources.src.external')}
                          </span>
                        </div>
                        <div className="rc-meta" data-res-meta={row.id}>
                          <span>{t('resources.col.size')} {fmtBytes(row.size)}</span>
                          {!downloading && !row.failed && !row.cancelled && (
                            <span>{t('resources.col.updated')} {relativeTimeTextIso(row.updatedAt, t) || '—'}</span>
                          )}
                        </div>
                        <div className="rc-status" aria-live="polite">
                          {row.builtin ? (
                            <span className="rc-status-item">{t('resources.smartRouting')}</span>
                          ) : row.references > 0 ? (
                            <span className="rc-status-item">
                              {t('resources.refCountAria', { count: row.references })}
                            </span>
                          ) : (
                            <span className="rc-status-item">{t('resources.unreferenced')}</span>
                          )}
                          {row.failed && <span className="rc-status-item warn">{t('resources.updateFailed')}</span>}
                          {row.cancelled && <span className="rc-status-item">{t('resources.cancelled')}</span>}
                        </div>
                        <InlineError text={errorOf(`res:${row.id}`)} />
                        {downloading && <ProgressBar ratio={row.progress ?? 0} label={t('resources.update')} />}
                      </div>
                      <div className="rc-row-actions">
                        <button
                          type="button"
                          className="rc-more"
                          aria-label={`${row.name} · ${t('mobileRules.more')}`}
                          aria-haspopup="dialog"
                          aria-expanded={sheetId === row.id}
                          onClick={() => onOpenSheet(row.id)}
                        >
                          <MoreIcon />
                        </button>
                      </div>
                      {sheetId === row.id && (
                        <ActionSheet
                          title={row.name}
                          actions={actions}
                          onClose={() => onOpenSheet(null)}
                          closeLabel={t('common.cancel')}
                        />
                      )}
                    </article>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      )}
    </section>
  );
}
