/**
 * 设置 → 备份（移动端）。桌面对照：`components/screens/settings/SettingsBackup.tsx`。
 *
 * 类目选择整块原样保留（8 类 + 全选，`domain/backup-categories` 是同一份真值源）。
 *
 * # 导出/导入两条腿（W-18 起是**真的接上了**，不再是禁用 + 理由）
 *
 * 上一版这里写着「两个系统面板都在、缺口在 Rust 侧 `into_path()`，故按钮禁用 + 写清为什么」。
 * 那句话今天已经**过期**：`commands/picked_file.rs` 给了 content URI 感知的读写，
 * `misc/backup.rs` 的导出/导入两条腿都改成按目标形态分派 —— Android SAF 交回的
 * `FilePath::Url(content://…)` 不再被 `into_path().ok()` 吃成「用户取消了」。
 * 于是这两颗按钮的 `disabled` 一起解开，理由那句话也改成实话（`pickerNote`：说的是
 * 「走的是哪个系统面板」，不再是「这条腿没接」）。
 *
 * 两个系统面板仍然是**两个不同的对象**、都不是分享面板：导出走
 * `Intent.ACTION_CREATE_DOCUMENT`（tauri-plugin-dialog 的 `DialogPlugin.kt:204 saveFileDialog`）＝
 * 文件保存器；导入走 `Intent.ACTION_GET_CONTENT`（`:57 showFilePicker`）＝文件选择器。
 *
 * # 导入预览与覆盖确认
 *
 * 桌面把 `importPick → 逐类目预览勾选 → importApply` 放进 `BackupImportDialog`。移动端把预览
 * 就地展开成本页下半截的一个组；「恢复所选」再经移动表单宿主的确认面板，确认后才 apply。
 * **不做「一步到位直接 apply」**：整类替换是破坏性的，看不见要替换什么就按下去，与桌面同一个
 * 动作在两端的风险等级会不一样。
 *
 * # 失败/成功都必须看得见
 *
 * 两条腿的失败走 `commit(行 id, promise, 取文)`，错误紧贴按钮；一次性成功回执走
 * `MobileToaster` 的短 toast，不常驻占设置行。导入成功时预览会一并收起。
 */

import { useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import {
  BACKUP_CATEGORIES,
  normalizeBackupSelection,
  toggleBackupCategory,
  type BackupCategory,
} from '@/domain/backup-categories';
import { backupErrorText } from '@/domain/action-error-text';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { closeMobileForm, openMobileForm, useMobileFormStore } from '../forms/form-store';
import { MobileButton, MobileSwitch, SettingsGroup, SettingsRow } from './SettingsChrome';
import { failureText, type CommitWrite } from './write-feedback';

/** 备份类别标签 → i18n 键。与桌面同一张表（文案逐字相同，不另造重复键）。 */
const CATEGORY_LABEL_KEYS: Readonly<Record<BackupCategory, string>> = {
  manualNodes: 'settings.advanced.backup.manualNodes',
  meshNodes: 'settings.advanced.backup.meshNodes',
  subscriptions: 'settings.backup.catSubscriptions',
  customRules: 'settings.backup.catCustomRules',
  dnsRules: 'settings.backup.catDnsRules',
  dnsResources: 'settings.backup.catDnsResources',
  appRules: 'settings.advanced.backup.appRules',
  meshRouting: 'settings.backup.catMeshRouting',
  generalSettings: 'settings.advanced.backup.generalSettings',
};

/** 导出/导入两颗按钮所在的行 id —— `commit` 的失败回显按它定位。 */
const ACTIONS_ROW = 'backup-actions';
/** 导入预览里「恢复所选 / 放弃」那一行的 id。 */
const IMPORT_ROW = 'backup-import-actions';

/** `importPick` 的回执里本页要用到的那几格。 */
interface PendingImport {
  readonly filePath: string;
  readonly fileName: string;
  readonly available: readonly BackupCategory[];
  readonly blockedCategories: readonly BackupCategory[];
  readonly meshRoutingOwnerServerIds: readonly string[];
  readonly counts: Partial<Record<BackupCategory, number>>;
  readonly unavailableInterfaceBindings: Partial<Record<BackupCategory, number>>;
}

/**
 * 回执里的 `filePath` 在桌面是路径、在 Android 是 `content://…`。
 * 展示名只取最后一段：全路径不该出现在界面上（桌面同口径），content URI 的最后一段是
 * document id，读起来不像文件名也照实显示 —— 不编一个好看的名字。
 */
function displayName(filePath: string): string {
  const segments = filePath.split(/[\\/]/).filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1]! : filePath;
}

export function BackupPage({ commit }: { commit: CommitWrite }): ReactElement {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<Set<BackupCategory>>(() => new Set(BACKUP_CATEGORIES));
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<PendingImport | null>(null);
  const [importPick, setImportPick] = useState<Set<BackupCategory>>(() => new Set());
  const previewRef = useRef({ pending, importPick });
  previewRef.current = { pending, importPick };
  const confirmIdRef = useRef<string | null>(null);

  const allOn = selected.size === BACKUP_CATEGORIES.length;
  const selectedArr = useMemo(() => Array.from(selected), [selected]);

  /** 勾选的类里，有多少条网卡绑定在本机不存在（导入后会被回退为自动/继承）。 */
  const pendingUnavailableBindings = useMemo(() => {
    if (pending === null) return 0;
    return Array.from(importPick).reduce(
      (sum, cat) => sum + (pending.unavailableInterfaceBindings[cat] ?? 0),
      0,
    );
  }, [pending, importPick]);

  function doExport(): void {
    setBusy(true);
    commit(
      ACTIONS_ROW,
      (async () => {
        try {
          const res = await api.backup.export(selectedArr.length > 0 ? selectedArr : undefined);
          // 用户在系统保存器里按了取消不是失败 —— 什么都不说。
          // （W-18 之前，Android 上「选好了位置」也会走到这一支，那才是要修的静默。）
          if (res.errorCode === 'cancelled') return;
          if (!res.success) throw new Error(backupErrorText(res.errorCode, t));
          toast.success(t('settings.advanced.backup.exportSuccess'));
        } finally {
          setBusy(false);
        }
      })(),
      (err) =>
        failureText(t, err, {
          withReason: 'mobileSettings.backup.exportFailedWithReason',
          plain: 'settings.advanced.backup.exportFail',
        }),
    );
  }

  function doImportPick(): void {
    setBusy(true);
    commit(
      ACTIONS_ROW,
      (async () => {
        try {
          const res = await api.backup.importPick();
          if (res.canceled) return;
          if (res.errorCode !== undefined || res.filePath === undefined) {
            throw new Error(backupErrorText(res.errorCode, t));
          }
          const available = res.available ?? [];
          const blockedCategories = res.blockedCategories ?? [];
          setPending({
            filePath: res.filePath,
            fileName: displayName(res.filePath),
            available,
            blockedCategories,
            meshRoutingOwnerServerIds: res.meshRoutingOwnerServerIds ?? [],
            counts: res.counts ?? {},
            unavailableInterfaceBindings: res.unavailableInterfaceBindings ?? {},
          });
          // 默认全选备份里真的有的那些类；依赖闭包（规则 ⇒ DNS 资源）交给同一份真值源维持。
          const selectable = available.filter((cat) => !blockedCategories.includes(cat));
          setImportPick(normalizeBackupSelection(selectable, selectable));
        } finally {
          setBusy(false);
        }
      })(),
      (err) =>
        failureText(t, err, {
          withReason: 'mobileSettings.backup.importFailedWithReason',
          plain: 'backupImport.errParse',
        }),
    );
  }

  function doImportApply(source: PendingImport, categories: readonly BackupCategory[]): void {
    setBusy(true);
    commit(
      IMPORT_ROW,
      (async () => {
        try {
          const res = await api.backup.importApply(source.filePath, [...categories]);
          if (!res.success) throw new Error(backupErrorText(res.errorCode, t));
          const fallback = res.unavailableInterfaceBindings ?? 0;
          setPending(null);
          toast.success(
            fallback > 0
              ? t('backupImport.interfaceFallbackDone', { n: fallback })
              : t('mobileSettings.backup.importDone'),
          );
        } finally {
          setBusy(false);
        }
      })(),
      (err) =>
        failureText(t, err, {
          withReason: 'mobileSettings.backup.importFailedWithReason',
          plain: 'backupImport.errApply',
        }),
    );
  }

  function confirmImport(source: PendingImport): void {
    if (busy) return;
    if (confirmIdRef.current !== null && useMobileFormStore.getState().hasInstance(confirmIdRef.current)) return;
    const categories = BACKUP_CATEGORIES.filter((cat) => importPick.has(cat));
    if (categories.length === 0) return;
    let submitted = false;
    const confirmId = openMobileForm({
      kind: 'confirm',
      payload: {
        title: t('backupImport.confirmTitle'),
        message: `${t('backupImport.replaceWarn')} ${t('backupImport.confirmScope', {
          categories: categories.map((cat) => t(CATEGORY_LABEL_KEYS[cat])).join(' · '),
        })}`,
        confirmLabel: t('backupImport.restoreSelected'),
        danger: true,
        onConfirm: () => {
          if (submitted || !useMobileFormStore.getState().hasInstance(confirmId)) return;
          const current = previewRef.current;
          closeMobileForm(confirmId);
          confirmIdRef.current = null;
          if (current.pending !== source || current.importPick.size !== categories.length ||
              categories.some((cat) => !current.importPick.has(cat))) return;
          submitted = true;
          doImportApply(source, categories);
        },
      },
    });
    confirmIdRef.current = confirmId;
  }

  return (
    <>
      <SettingsGroup>
        <SettingsRow
          first
          id="backup-select-all"
          label={t('settings.advanced.backup.selectAll')}
          control={
            <MobileSwitch
              checked={allOn}
              ariaLabel={t('settings.advanced.backup.selectAll')}
              onChange={() => setSelected(allOn ? new Set() : new Set(BACKUP_CATEGORIES))}
            />
          }
        />
        {BACKUP_CATEGORIES.map((cat) => (
          <SettingsRow
            key={cat}
            id={`backup-${cat}`}
            label={t(CATEGORY_LABEL_KEYS[cat])}
            control={
              <MobileSwitch
                checked={selected.has(cat)}
                ariaLabel={t(CATEGORY_LABEL_KEYS[cat])}
                onChange={() => setSelected((prev) => toggleBackupCategory(prev, cat))}
              />
            }
          />
        ))}
        {/* 两颗按钮住在一条 `SettingsRow` 里而不是一个裸 `<div>`：`commit` 的行内红字是**按行 id**
            渲染的，留在裸 div 里等于「记了错但没人显示」—— 那正是裁定 #14 要消掉的形态。 */}
        <SettingsRow
          stacked
          id={ACTIONS_ROW}
          label={t('settings.nav.backup')}
          desc={t('mobileSettings.backup.pickerNote')}
          control={
            <div style={{ display: 'flex', gap: '8px' }}>
              <MobileButton disabled={busy || selectedArr.length === 0} onClick={doExport}>
                {t('settings.backup.exportSelected')}
              </MobileButton>
              <MobileButton disabled={busy} onClick={doImportPick}>
                {t('settings.backup.import')}
              </MobileButton>
            </div>
          }
        />
      </SettingsGroup>

      {pending !== null && (
        <SettingsGroup header={t('backupImport.title')}>
          <SettingsRow
            first
            id="backup-import-file"
            label={t('backupImport.fileLabel', { name: pending.fileName })}
            desc={t('backupImport.replaceWarn')}
            hint={
              pendingUnavailableBindings > 0
                ? t('backupImport.interfaceFallbackWarn', { n: pendingUnavailableBindings })
                : undefined
            }
          />
          {BACKUP_CATEGORIES.filter((cat) => pending.available.includes(cat)).map((cat) => (
            <SettingsRow
              key={cat}
              id={`backup-import-${cat}`}
              label={t(CATEGORY_LABEL_KEYS[cat])}
              desc={cat === 'meshRouting' && pending.blockedCategories.includes(cat)
                ? t('backupImport.meshRoutingBlocked')
                : undefined}
              hint={cat === 'meshRouting' && pending.meshRoutingOwnerServerIds.length > 0
                ? t('backupImport.meshRoutingOwners', { ids: pending.meshRoutingOwnerServerIds.join(', ') })
                : undefined}
              control={
                <>
                  <span style={{ fontFamily: 'var(--mono)', color: 'hsl(var(--fg-dim))' }}>
                    {pending.counts[cat] ?? '—'}
                  </span>
                  <MobileSwitch
                    checked={importPick.has(cat)}
                    disabled={pending.blockedCategories.includes(cat)}
                    ariaLabel={t(CATEGORY_LABEL_KEYS[cat])}
                    onChange={() =>
                      setImportPick((prev) =>
                        toggleBackupCategory(prev, cat, pending.available),
                      )
                    }
                  />
                </>
              }
            />
          ))}
          <SettingsRow
            stacked
            id={IMPORT_ROW}
            label={t('backupImport.categories')}
            control={
              <div style={{ display: 'flex', gap: '8px' }}>
                <MobileButton
                  tone="primary"
                  disabled={busy || importPick.size === 0}
                  onClick={() => confirmImport(pending)}
                >
                  {t('backupImport.restoreSelected')}
                </MobileButton>
                <MobileButton disabled={busy} onClick={() => setPending(null)}>
                  {t('common.cancel')}
                </MobileButton>
              </div>
            }
          />
        </SettingsGroup>
      )}

      <SystemBackupGroup commit={commit} />
    </>
  );
}

/**
 * 「系统备份」开关（Android Auto Backup 的运行期闸门，2026-09-25 决策：默认关、用户可开）。
 *
 * # 为什么放在备份页、不放通用页
 *
 * 用户找「备份」会来这一页；它和上面的导出是**两件不同的事**（导出 = 用户自己选位置存一个文件；
 * 系统备份 = Android 把整个应用数据交给 Google 账号 / 换机迁移），并排放、各自说清，比拆到两处更不易混。
 *
 * # 真值只在 Kotlin，不进 user config
 *
 * 开关文件住 `noBackupFilesDir`（`PolarisBackupAgent`，备份时 Rust 不在、只有它在读）。这里**不**另存一份
 * 到配置：user config 自己就会随备份走，恢复到新设备后那份副本会说「开」，而本机真值是「关」——
 * 界面与实际行为分叉，偏偏分叉在凭据会不会出设备这件事上。故读写都直走执行侧（`api.systemBackup`）。
 *
 * 读不到时开关**禁用**并在行内说读不到（`commit` 落红字），不显示成「关」：「关着」与「不知道」是两句话。
 */
function SystemBackupGroup({ commit }: { commit: CommitWrite }): ReactElement {
  const { t } = useTranslation();
  /** `null` = 还没读到（或读失败）。 */
  const [enabled, setEnabled] = useState<boolean | null>(null);

  useEffect(() => {
    commit('system-backup', api.systemBackup.getStatus().then(setEnabled), (err) =>
      failureText(t, err, {
        withReason: 'mobileSettings.backup.systemBackupReadFailedWithReason',
        plain: 'mobileSettings.backup.systemBackupReadFailed',
      }),
    );
  }, [commit, t]);

  return (
    <SettingsGroup header={t('mobileSettings.backup.systemBackupGroup')}>
      <SettingsRow
        first
        id="system-backup"
        label={t('mobileSettings.backup.systemBackupTitle')}
        desc={t('mobileHelp.systemBackup')}
          descDetails={t('mobileSettings.backup.systemBackupDesc')}
        control={
          <MobileSwitch
            checked={enabled === true}
            disabled={enabled === null}
            ariaLabel={t('mobileSettings.backup.systemBackupTitle')}
            onChange={(v) =>
              commit(
                'system-backup',
                api.systemBackup.set(v).then(() => setEnabled(v)),
              )
            }
          />
        }
      />
    </SettingsGroup>
  );
}
