import { MobileInfo } from '../MobileInfo';
import { MobileSelect as MobileSelect } from '../MobileSelect';
/**
 * 移动端**添加自定义应用**表单。桌面对位：`dialogs/AppAddDialog.tsx`。
 *
 * # 图标那一维只做 emoji 档，这不是「少做了」
 *
 * 桌面给四种图标来源（首字母 / 在线图库 / URL / emoji），前三种最终都落成一个 `iconUrl`。
 * 而**移动端的应用行只渲染第三态**（`AppsSegment.tsx` 的 `AppRowModel.emoji`：不走图标代理的
 * 网络腿，理由与登记逐字写在那份文件的 `AppIcon` 一条上）⇒ 在这里做图库与 URL，等于让用户配一个
 * 本端**永远不会被渲染**的字段。故本表只给 emoji 一档，且落盘时 `iconUrl` 干脆不写。
 * ⚠️ 这条与「桌面已经填过 `iconUrl` 的自定义应用」不冲突：编辑不在本表的射程里（桌面那张表
 * 同样只有新增、没有编辑态），本表只管新建，新建出来的那一条在两端都能显示 —— 桌面会走
 * emoji 兜底，那正是它三态里的最后一档。
 *
 * # 资源标签是必填，而且是这张表的**要害**
 *
 * 一个自定义应用在引擎里首先靠 `geositeTags` / `geoipTags` 命中 ⇒ 一条没有 geosite 标签的
 * 自定义应用在「指定节点」「阻断」两档上**什么都匹配不到**。故校验与桌面同：geosite 至少一个，
 * 否则不许提交。包名那条腿（见下）不能替代它 —— 它只覆盖「直连」一档。
 *
 * # 包名那一格（2026-09-13 批 16 接通）
 *
 * 本表此前顶上常驻一句「按应用身份匹配还没接上」，那是**真话**：`CustomAppPreset` 没有包名字段，
 * `get_app_preset` 把它硬写成 `Vec::new()`，于是「自定义应用设成直连」在 Android 上静默不生效
 * （`addDisallowedApplication` 认 applicationId，而 `processNames` 是桌面那条腿的值，一条都命不中）。
 *
 * 本批把那一格补上，三段一起：配置 schema（Rust `CustomAppPreset.packageNames`）、
 * 数据源（`api.system.listInstalledApps()` ⇄ `system_list_installed_apps` ⇄ Kotlin
 * `PolarisVpnPlugin.listInstalledApps`）、以及这张表上的选择面。
 *
 * 🔴 **射程**（2026-09-25 A5 改写）：包名在 Android 上有两个消费点 —— 「直连」进 `tun.exclude_package`
 * （系统边界整个排除出隧道，各版本有效）；「指定节点 / 阻断」进 route 的 `package_name` 规则
 * （`builder::route::app_owner_leg`，靠 `findConnectionOwner` 回填包名，**只在 Android 10+ 命中**）。
 * 这条边界由规则屏那句常驻说明（`mobileRules.appRoutingMatchNote`）如实讲着，本表的文案与它同向。
 *
 * ⚠️ 清单**拉不到时不画成空**：空表与「这台设备上真的一个应用都没有」在界面上不可分，而后者
 * 几乎不可能。后端对此一律 `success:false` + 原因（不返空表），这里据此说「读不到」并留下
 * geosite 那条腿可用 —— 包名是可选的，读不到不该把整张表卡死。
 *
 * # 复用
 *
 * 可用标签集走 `@/domain/rule-resource-refs#availableResourceTagSet`（与规则条件的候选池同一份），
 * 进程名归一走 `@/components/dialogs/process-selection#parseProcessNames`，
 * 写腿走 store 的 `mutateConfigEntities` + `editRoute('customAppPresets', …)` 暂存闸门 ——
 * 三样都与桌面同源。
 */

import { useEffect, useMemo, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import type { CustomAppPreset, InstalledApp } from '@/contracts/types';
import { availableResourceTagSet } from '@/domain/rule-resource-refs';
import { parseProcessNames } from '@/components/dialogs/process-selection';
import { editRoute } from '@/lib/staged-config';
import { useAppStore } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

/**
 * emoji 候选。与桌面 `AppAddDialog.tsx` 的 `EMOJI_PALETTE` 同值。
 *
 * 它是**候选调色板不是判据**：用户可以挑任何一个，落盘的就是那个字符串；两端各有一份即便漂了，
 * 后果也只是「候选不一样」，不改变任何已保存数据的含义。故不为它开一个共享模块。
 */
const EMOJI_PALETTE = ['🌐', '📺', '🎬', '🎮', '💬', '🤖', '🛒', '🎵'] as const;
const DEFAULT_EMOJI = '🌐';

/** 分类档位。与桌面同 6 档（5 个内置 + 自建）。`ai` 的标签在桌面就是字面量 'AI'。 */
const CATEGORY_IDS = ['video', 'social', 'ai', 'tools', 'game', 'custom'] as const;

type TagKind = 'geosite' | 'geoip';

export function AppAddPanel({ instanceId }: { instanceId: string }): ReactElement {
  const { t } = useTranslation();
  const tr = (key: string, vars?: Record<string, unknown>): string => t(key, vars ?? {}) as string;
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);

  const [name, setName] = useState('');
  const [emoji, setEmoji] = useState<string>(DEFAULT_EMOJI);
  const [category, setCategory] = useState<string>('video');
  const [customCategory, setCustomCategory] = useState('');
  const [proc, setProc] = useState('');
  const [tagKind, setTagKind] = useState<TagKind>('geosite');
  const [tagQuery, setTagQuery] = useState('');
  const [geositeSel, setGeositeSel] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [geoipSel, setGeoipSel] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [pool, setPool] = useState<{ geosite: string[]; geoip: string[] }>({ geosite: [], geoip: [] });
  const [available, setAvailable] = useState<ReadonlySet<string>>(() => new Set<string>());
  /* 已装应用清单三态：`null` = 还没回来（画「加载中」）、`[]`+`appsFailed` = 读不到、
     `[]`+!`appsFailed` = 这台设备真的枚举不出应用。三者必须分得开，见文件头注那条 ⚠️。 */
  const [apps, setApps] = useState<readonly InstalledApp[] | null>(null);
  const [appsFailed, setAppsFailed] = useState(false);
  const [appQuery, setAppQuery] = useState('');
  const [pkgSel, setPkgSel] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [errName, setErrName] = useState(false);
  const [errGeosite, setErrGeosite] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();

  /**
   * 标签池。与桌面 `AppAddDialog.tsx:170-201` 同义：内置目录里该 category 的条目名，
   * **并上**已下载资源里以 `<kind>-` 起头的那些去掉前缀后的裸名。
   *
   * 🔴 两侧都产出**裸名**（`youtube` 而不是 `geosite-youtube`）：预设存的 `geositeTags` 就是裸名，
   * 带前缀存进去会在生成配置时拼成 `geosite-geosite-youtube`，那条规则集永远找不到。
   * 「哪些 tag 本地可用」这条判据本身走共享的 `availableResourceTagSet`，不在这里重写。
   */
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [catalog, resources] = await Promise.all([
          api.ruleResources.getCatalog(),
          api.ruleResources.list(),
        ]);
        if (!alive) return;
        const avail = availableResourceTagSet(Array.isArray(resources) ? resources : []);
        const poolOf = (kind: TagKind): string[] => {
          const names = (catalog?.items ?? [])
            .filter((item) => item.category === kind)
            .map((item) => item.name);
          for (const tag of avail) {
            if (tag.startsWith(`${kind}-`)) names.push(tag.slice(kind.length + 1));
          }
          return [...new Set(names)].sort((a, b) => a.localeCompare(b));
        };
        setPool({ geosite: poolOf('geosite'), geoip: poolOf('geoip') });
        setAvailable(avail);
      } catch (err) {
        console.error('[mobile-app-add] load catalog/resources failed:', err);
        /* 标签池拉不到 ⇒ 那张必填的清单会是空的，而空清单与「搜不到」长得一样。
           必须说出来：不说的话用户会以为自己搜错了词，而实际上这张表此刻提交不了。 */
        if (alive) setNotice({ tone: 'err', text: tr('resCatalog.loadFailed') });
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  /**
   * 已装应用清单。**只拉一次**（它随「用户装/卸应用」变，而本表的生命周期以分钟计）。
   *
   * 排序：非系统应用在前、同组按显示名本地化排序 —— 用户要找的几乎总是自己装的那些，
   * 而 Android 上系统包在数量上占大头。`system` 这一列在本表里的全部用途就是这个次序。
   */
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const list = await api.system.listInstalledApps();
        if (!alive) return;
        setApps(
          [...(Array.isArray(list) ? list : [])].sort(
            (a, b) =>
              Number(a.system) - Number(b.system) || a.label.localeCompare(b.label),
          ),
        );
      } catch (err) {
        console.error('[mobile-app-add] list installed apps failed:', err);
        /* 读不到 ≠ 一个都没有。**不落 notice** —— 包名是可选的一格，把它的失败提到表级
           红字上会盖住真正卡住提交的那条（geosite 必填）。就地一行说清，表照样能提交。 */
        if (alive) {
          setApps([]);
          setAppsFailed(true);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const filteredApps = useMemo(() => {
    const list = apps ?? [];
    const needle = appQuery.trim().toLowerCase();
    if (needle === '') return list;
    /* 名字与包名两列都搜：用户可能记得「Chrome」，也可能记得 `com.android.chrome`。 */
    return list.filter(
      (a) =>
        a.label.toLowerCase().includes(needle) || a.packageName.toLowerCase().includes(needle),
    );
  }, [apps, appQuery]);

  const curPool = tagKind === 'geoip' ? pool.geoip : pool.geosite;
  const curSel = tagKind === 'geoip' ? geoipSel : geositeSel;
  const setCurSel = tagKind === 'geoip' ? setGeoipSel : setGeositeSel;
  const filteredTags = useMemo(() => {
    const needle = tagQuery.trim().toLowerCase();
    if (needle === '') return curPool;
    return curPool.filter((tag) => tag.toLowerCase().includes(needle));
  }, [curPool, tagQuery]);

  const dirty =
    name.trim() !== ''
    || geositeSel.size > 0
    || geoipSel.size > 0
    || pkgSel.size > 0
    || proc.trim() !== ''
    || emoji !== DEFAULT_EMOJI;

  const togglePkg = (pkg: string): void => {
    setPkgSel((prev) => {
      const next = new Set(prev);
      if (next.has(pkg)) next.delete(pkg);
      else next.add(pkg);
      return next;
    });
  };

  const toggleTag = (tag: string): void => {
    setCurSel((prev) => {
      const next = new Set(prev);
      if (next.has(tag)) next.delete(tag);
      else next.add(tag);
      return next;
    });
    setErrGeosite(false);
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
        title: tr('appAdd.discardTitle'),
        message: tr('appAdd.discardMsg'),
        confirmLabel: tr('node.discard'),
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
    const nameEmpty = name.trim() === '';
    const geositeEmpty = geositeSel.size === 0;
    setErrName(nameEmpty);
    setErrGeosite(geositeEmpty);
    if (nameEmpty || geositeEmpty) return;

    const processNames = parseProcessNames(proc);
    const preset: CustomAppPreset = {
      id: `custom-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`,
      name: name.trim(),
      emoji: emoji || DEFAULT_EMOJI,
      geositeTags: [...geositeSel],
      category: category === 'custom' ? (customCategory.trim() || 'custom') : category,
      ...(geoipSel.size > 0 ? { geoipTags: [...geoipSel] } : {}),
      ...(processNames.length > 0 ? { processNames } : {}),
      /* 没挑就不写这个键 —— 与 Rust 侧 `skip_serializing_if = "Vec::is_empty"` 同一口径：
         空表与缺省等价，写一个空数组只会让盘上多一个没有含义的键。 */
      ...(pkgSel.size > 0 ? { packageNames: [...pkgSel] } : {}),
    };

    /* 暂存闸门：`customAppPresets` 是 UserConfig 字段（Class B），条目按预设 id 寻址。 */
    if (editRoute('customAppPresets', stagingEnabled) === 'staged') {
      stage({
        id: `appPreset:${preset.id}`,
        kind: 'appPreset',
        label: `${tr('appAdd.title')} ${preset.name}`,
        entityPath: ['customAppPresets', preset.id],
        nextValue: preset,
      });
      closeInstance(instanceId);
      return;
    }

    setSubmitting(true);
    setNotice(undefined);
    void (async () => {
      try {
        await useAppStore
          .getState()
          .mutateConfigEntities([
            { collection: 'customAppPresets', entityId: preset.id, value: preset },
          ]);
        closeInstance(instanceId);
        toast.success(tr('common.saved'));
      } catch (err) {
        console.error('[mobile-app-add] save failed:', err);
        if (hasInstance(instanceId)) setNotice({ tone: 'err', text: tr('common.saveFailed') });
      } finally {
        if (hasInstance(instanceId)) setSubmitting(false);
      }
    })();
  };

  return (
    <FormSheet
      title={tr('appAdd.title')}
      onRequestClose={requestClose}
      closeLocked={submitting}
      closeLabel={tr('common.close')}
      cancelLabel={tr('common.cancel')}
      submitLabel={tr('appPolicy.addCustom')}
      submitDisabled={submitting}
      onSubmit={submit}
      notice={notice}
    >
      {/*
        自定义应用在这台设备上按什么匹配 —— **接通之后的事实**，不再是一条债。

        旧文案（`mobileRules.customAppUnavailable`）说的是「按应用身份匹配还没接上」，那在
        2026-09-13 之前是真话。本批把三段一起接上（配置 schema + 数据源 + 下面那张选择面）之后
        它就变成假话了，故整条键从五份 locale 里**删掉**，换成 `mobileRules.customAppMatch`。
        🔴 不是「把一句真话删成沉默」：同一位置仍常驻一句说明，讲的是接通之后的两条匹配腿
        **以及包名那条腿的射程上界**（只对「直连」生效）。键名换了是因为 `*Unavailable` 落在
        接线门 A2 的缺席词表上 —— 留着它，一条已经接上的能力会永远记在未接线账本里。

        为什么这句话必须在这张表的**最前面**：两条匹配腿覆盖的策略档不一样（geosite/geoip 四档
        都认，包名只有「直连」那一档），先说清楚，才谈得上填得对。
      */}
      <div className="m-form-hint"><MobileInfo title={tr('mobileRules.matchSummary')} summary={tr('mobileHelp.customAppMatch')} details={tr('mobileRules.customAppMatch')} /></div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="maa-name">
          {tr('appAdd.name')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </label>
        <input
          id="maa-name"
          className="m-form-input"
          value={name}
          placeholder={tr('appAdd.namePh')}
          onChange={(e) => {
            setName(e.target.value);
            setErrName(false);
          }}
        />
        {errName && <p className="m-form-err">{tr('appAdd.errName')}</p>}
      </div>

      <div className="m-form-row">
        <span className="m-form-label">{tr('appAdd.icon')}</span>
        <div className="m-form-foot">
          {EMOJI_PALETTE.map((candidate) => (
            <button
              key={candidate}
              type="button"
              className={emoji === candidate ? 'm-form-btn primary' : 'm-form-btn'}
              aria-pressed={emoji === candidate}
              onClick={() => setEmoji(candidate)}
            >
              {candidate}
            </button>
          ))}
        </div>
        {/* 这一行不是免责声明，是本端的事实：应用行画的就是这个字符。 */}
        <p className="m-form-hint">{tr('appAdd.emojiOnlyNote')}</p>
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="maa-cat">
          {tr('appAdd.category')}
        </label>
        <MobileSelect
          id="maa-cat"
          className="m-form-select"
          value={category}
          onChange={(e) => setCategory(e.target.value)}
        >
          {CATEGORY_IDS.map((id) => (
            <option key={id} value={id}>
              {/* `ai` 在桌面就是字面量 'AI'：它是产品名不是词，翻译它只会得到五个 'AI'。 */}
              {id === 'ai' ? 'AI' : id === 'custom' ? tr('appAdd.catCustom') : tr(`appPolicy.cat.${id}`)}
            </option>
          ))}
        </MobileSelect>
        {category === 'custom' && (
          <input
            className="m-form-input"
            value={customCategory}
            placeholder={tr('appAdd.catCustomPh')}
            onChange={(e) => setCustomCategory(e.target.value)}
          />
        )}
      </div>

      <div className="m-form-row">
        <span className="m-form-label">
          {tr('appAdd.resTags')}
          <span className="m-form-req" aria-hidden>
            *
          </span>
        </span>
        <p className="m-form-hint">{tr('appAdd.resTagsHint')}</p>
        <div className="m-form-foot">
          <button
            type="button"
            className={tagKind === 'geosite' ? 'm-form-btn primary' : 'm-form-btn'}
            aria-pressed={tagKind === 'geosite'}
            onClick={() => setTagKind('geosite')}
          >
            Geosite
          </button>
          <button
            type="button"
            className={tagKind === 'geoip' ? 'm-form-btn primary' : 'm-form-btn'}
            aria-pressed={tagKind === 'geoip'}
            onClick={() => setTagKind('geoip')}
          >
            GeoIP
          </button>
        </div>
        <input
          className="m-form-input"
          value={tagQuery}
          placeholder={tr('appAdd.tagSearchPh')}
          onChange={(e) => setTagQuery(e.target.value)}
        />
        <div className="mr-sel-list" role="listbox" aria-label={tr('appAdd.resTags')} aria-multiselectable>
          {filteredTags.length === 0 ? (
            <p className="m-form-hint">{tr('appAdd.noMatchTag')}</p>
          ) : (
            filteredTags.map((tag) => {
              const full = `${tagKind}-${tag}`;
              const missing = !available.has(full);
              return (
                <button
                  key={tag}
                  type="button"
                  className="mr-sel-opt"
                  role="option"
                  aria-selected={curSel.has(tag)}
                  onClick={() => toggleTag(tag)}
                >
                  <span className="mr-sel-opt-tx">
                    <span>{tag}</span>
                    {/* 本地没有这份规则集时不拦截、只提醒：资源可以事后在「规则资源」页补下载，
                        而拦住会让用户配不出一个明天才下载的标签。 */}
                    {missing && (
                      <span className="mr-sheet-item-sub">{tr('rules.candidateNotLocal')}</span>
                    )}
                  </span>
                </button>
              );
            })
          )}
        </div>
        {errGeosite && <p className="m-form-err">{tr('appAdd.errGeosite')}</p>}
      </div>

      <div className="m-form-row">
        <label className="m-form-label" htmlFor="maa-proc">
          {tr('appAdd.procNames')}
          <span className="m-form-opt">{tr('common.optional')}</span>
        </label>
        <input
          id="maa-proc"
          className="m-form-input mono"
          value={proc}
          placeholder="chrome.exe, slack"
          onChange={(e) => setProc(e.target.value)}
        />
        {/* 进程名在这台设备上永不命中 —— 它是同一件事的**桌面**那个平台形态，这台设备上的
            对应物是下面那格包名。留着这一格是因为同一份配置会同步到桌面，那边会用。 */}
      </div>

      {/*
        包名（applicationId）—— 进程名在 Android 上的对应物。

        这些包名三档都消费：「直连」发射进 `tun.exclude_package`（整个不进隧道），「指定节点 /
        阻断」发射进 route 的 `package_name` 规则（Android 10+ 命中，射程见规则屏常驻说明）。
      */}
      <div className="m-form-row">
        <span className="m-form-label">
          {tr('mobileRules.customAppPackages')}
          <span className="m-form-opt">{tr('common.optional')}</span>
        </span>
        {apps === null ? (
          <p className="m-form-hint">{tr('common.loading')}</p>
        ) : appsFailed ? (
          /* 读不到 ≠ 一个都没有：后端对这条腿的失败是显式的（不返空表），照实说。 */
          <p className="m-form-hint">{tr('mobileRules.customAppPackagesFailed')}</p>
        ) : (
          <>
            <input
              className="m-form-input"
              value={appQuery}
              placeholder={tr('common.search')}
              onChange={(e) => setAppQuery(e.target.value)}
            />
            <div
              className="mr-sel-list"
              role="listbox"
              aria-label={tr('mobileRules.customAppPackages')}
              aria-multiselectable
            >
              {filteredApps.length === 0 ? (
                <p className="m-form-hint">{tr('common.noResults')}</p>
              ) : (
                filteredApps.map((app) => (
                  <button
                    key={app.packageName}
                    type="button"
                    className="mr-sel-opt"
                    role="option"
                    aria-selected={pkgSel.has(app.packageName)}
                    onClick={() => togglePkg(app.packageName)}
                  >
                    <span className="mr-sel-opt-tx">
                      <span>{app.label}</span>
                      {/* 包名常驻，不只在选中时显示：同名应用（多个 "Camera"）只有这一列分得开。 */}
                      <span className="mr-sheet-item-sub">{app.packageName}</span>
                    </span>
                  </button>
                ))
              )}
            </div>
          </>
        )}
      </div>
    </FormSheet>
  );
}
