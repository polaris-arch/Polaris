/**
 * 移动端**手动导入**（粘贴分享链接 / Base64 / Clash / sing-box 文本）。
 *
 * # 两步，不是一步
 *
 * 「解析」→ **停在预览**（计数 + 告警 + 节点清单）→ 「导入 N 项」才 `server.addBulk` 入库。
 * 拆两步的理由逐字取自桌面 `ImportDialog`：后端一直在返回 `stats` 与 `warnings`
 * （「4 个因缺字段没进来」「6 个协议本内核不认、已透传成 custom」），一步式会把它们整个丢掉 ——
 * 用户点完只知道「成功了」。
 *
 * # 复用
 *
 *  · **晚到结果的发布闸** —— `local-import-parse-state.ts` 的 `canPublishLocalImportParse`
 *    与 `localImportPrimaryActionDisabled`：本地解析不可中止，表单关掉之后它的结果只是不再属于
 *    这一层。判据是纯谓词，不靠「组件恰好卸载了所以没人 setState」。
 *  · **组网单例硬闸门·逐条** —— `domain/endpoint-routes#admitMeshSingletons`（准入者即刻占槽，
 *    同一批里的第二个 WARP 也拦得住）。整批拒绝是错的：一条撞槽的节点不该连累同批其余正常节点。
 *  · **暂存分流** —— `editRoute`（同一个闸门）。批量导入 ⇒ **逐节点一条**暂存条目，
 *    整批一条会让「逐条撤销」退化成「要么全留要么全撤」，而那正是导入最需要挑挑拣拣的场景。
 *
 * # 文件腿（2026-09-25 ζ 批 A7）
 *
 * 🔴 **这一段此前写的是「文件选择腿不在本批」**，理由是它与备份页的 `pickerPending` 是「同一件
 * 未接的工作」。那条理由早就过期了：备份页的 SAF 选择器已接好（`pickerPending` 键已从 locale
 * 消失），后端 `local_import_pick_file` 也已兼容 Android content URI（W-18：不再 `into_path()`，
 * 句柄由 `commands::picked_file` 按目标形态分派）。于是用户在手机上拿到一份配置文件，只能先
 * 打开别的应用复制全文再粘贴 —— 而那份文件往往大到剪贴板不好用。
 *
 * 现在「选择文件」一颗按钮，调用方式同桌面 `ImportDialog#pickFile`（与备份页一样由后端开系统
 * 选择器，取消静默、`too_large` / 读失败各一句已有的五语键）。读回的正文**落进同一个文本框**再走
 * **同一条** `parse` → 预览链（含 unsupported / skipped 计数）—— 不另起一条预览；用户看得到读进来
 * 的是什么，也还能改。拖拽那条在触屏上不存在，不移植。
 * 这条腿由 `import-preview-parity.test.ts` 的「文件腿」对差守着：桌面 `pickFile` 调的每个 `api.*`
 * 与渲染的每个文案键，移动端都要有。
 */

import { useRef, useState, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import type { ImportParseResult } from '@/contracts/types';
import { admitMeshSingletons } from '@/domain/endpoint-routes';
import { isSupportedShareUrl } from '@/domain/protocol-url-schemes';
import {
  canPublishLocalImportFileRead,
  canPublishLocalImportParse,
  localImportPrimaryActionDisabled,
} from '@/components/dialogs/local-import-parse-state';
import { editRoute } from '@/lib/staged-config';
import { toast } from '@/lib/error-handler';
import { useAppStore, useEffectiveServers } from '@/store/app-store';
import { useStagedConfigStore } from '@/store/staged-config-store';
import { useStagingActive } from '@/store/use-staging-active';
import { FormSheet } from './FormSheet';
import { useMobileFormStore } from './form-store';

export function ImportFormPanel({
  instanceId,
  onAdded,
}: {
  instanceId: string;
  onAdded?: () => void;
}): ReactElement {
  const { t } = useTranslation();
  const open = useMobileFormStore((s) => s.open);
  const closeInstance = useMobileFormStore((s) => s.closeInstance);
  const hasInstance = useMobileFormStore((s) => s.hasInstance);
  // 展示面：单例槽判据必须含暂存节点 —— 否则暂存了一个 WARP 还能再导入第二个，重放后配置非法。
  const servers = useEffectiveServers();
  const loadConfig = useAppStore((s) => s.loadConfig);
  const stagingEnabled = useStagingActive();
  const stage = useStagedConfigStore((s) => s.stage);

  const [content, setContent] = useState('');
  const [parsing, setParsing] = useState(false);
  const [picking, setPicking] = useState(false);
  const [importing, setImporting] = useState(false);
  const [preview, setPreview] = useState<ImportParseResult | null>(null);
  const [notice, setNotice] = useState<{ tone: 'ok' | 'info' | 'err'; text: string } | undefined>();
  const generation = useRef(0);
  const contentRef = useRef('');

  const onContentChange = (v: string): void => {
    generation.current += 1;
    contentRef.current = v;
    setContent(v);
    // 内容一改，上一次的解析结果就不再描述它 —— 留着会让用户对着旧预览按「确认导入」。
    setPreview(null);
  };

  const requestClose = (): void => {
    if (importing) return; // 写 + 强刷是关键区，不留孤儿 continuation
    if (content.trim() === '') {
      generation.current += 1;
      closeInstance(instanceId);
      return;
    }
    const confirmId = open({
      kind: 'confirm',
      payload: {
        title: t('import.discardTitle'),
        message: t('import.discardMsg'),
        confirmLabel: t('node.discard'),
        danger: true,
        onConfirm: () => {
          closeInstance(confirmId);
          generation.current += 1;
          closeInstance(instanceId);
        },
      },
    });
  };

  /** `input` 缺省 = 文本框当下内容；文件腿把刚读回的正文直接喂进来（state 这一帧还没提交）。 */
  const parse = async (input: string = content): Promise<void> => {
    const text = input.trim();
    if (text === '') {
      setNotice({ tone: 'err', text: t('import.errEmpty') });
      return;
    }
    const attempt = { generation: generation.current, input };
    const publishable = (): boolean =>
      canPublishLocalImportParse(attempt, {
        generation: generation.current,
        input: contentRef.current,
        hasInstance: hasInstance(instanceId),
      });
    setParsing(true);
    setNotice(undefined);
    try {
      const result = await api.localImport.parse(text); // 0 节点 / 不可识别 → 后端 throw
      if (!publishable()) return;
      if (result.nodes.length === 0) {
        setNotice({ tone: 'err', text: t('import.errNoNodes') });
        return;
      }
      setPreview(result);
    } catch (e) {
      if (!publishable()) return;
      console.error('[mobile-import] parse failed:', e);
      setNotice({ tone: 'err', text: t('import.failed') });
    } finally {
      if (hasInstance(instanceId)) setParsing(false);
    }
  };

  /**
   * 文件腿：系统选择器（Android 上是 SAF）→ 正文落进文本框 → 同一条 `parse` 预览链。
   * 发布闸同桌面 `beginFileRead` / `canPublishFileRead`：选择器开着的时候用户改了文本、或者表单
   * 已经关了，晚到的文件内容就不再属于这一层。
   */
  const pickFile = async (): Promise<void> => {
    generation.current += 1;
    const attempt = { generation: generation.current };
    const publishable = (): boolean =>
      canPublishLocalImportFileRead(attempt, {
        generation: generation.current,
        hasInstance: hasInstance(instanceId),
      });
    setPreview(null);
    setNotice(undefined);
    setPicking(true);
    try {
      const r = await api.localImport.pickFile();
      if (!publishable()) return;
      if (r.canceled) return; // 用户取消 → 静默（同桌面）
      if (r.error === 'too_large') {
        setNotice({ tone: 'err', text: t('import.fileTooLarge') });
        return;
      }
      if (r.error || r.content == null) {
        setNotice({ tone: 'err', text: t('import.fileReadFail') });
        return;
      }
      onContentChange(r.content);
      await parse(r.content);
    } catch (e) {
      if (!publishable()) return;
      // 选择器 / IPC 的诊断里可能带本地路径或 content URI：可见提示固定走五语键，原文只进日志。
      console.error('[mobile-import] pick file failed:', e);
      setNotice({ tone: 'err', text: t('import.fileReadFail') });
    } finally {
      if (hasInstance(instanceId)) setPicking(false);
    }
  };

  const doImport = async (): Promise<void> => {
    const result = preview;
    if (result === null) return;
    setImporting(true);
    try {
      const { admitted, rejected } = admitMeshSingletons(result.nodes, servers);
      if (admitted.length === 0) {
        setNotice({ tone: 'err', text: t('nodes.importSingletonAllSkipped') });
        return;
      }
      const staged = editRoute('servers', stagingEnabled) === 'staged';
      if (staged) {
        for (const node of admitted) {
          const entityId = node.id !== '' ? node.id : crypto.randomUUID();
          stage({
            id: `server:${entityId}`,
            kind: 'server',
            label: `${t('import.title')} ${node.name}`,
            entityPath: ['servers', entityId],
            nextValue: { ...node, id: entityId },
          });
        }
      } else {
        await api.server.addBulk(admitted);
        // 写后端即刷 store（快路径；广播是慢路径，表单已经关了而列表还空着）。
        await loadConfig(true);
      }
      /*
       * 🔴 成功回执走**全局 toast**，不走面板自持的 `notice`（2026-09-06 三条复审同时点名）。
       *
       * `setNotice` 与下一行的 `closeInstance` 在同一个 async 续段里，React 18 自动批处理 ⇒
       * `MobileFormHost` 下一次提交时本实例已不在栈上、面板当帧卸载，那条回执**一帧都没画过**。
       * 丢掉的不是客套话：`nodes.importSingletonSkipped` 里「跳过了 M 条」是**只有这里说得出**
       * 的事实（撞组网单例槽被拒收），静默丢掉等于让用户以为 M 条也进来了。
       * 桌面 `ImportDialog.tsx:299-316` 走的就是 toast，且那段注释逐字记着它当初修的正是
       * 「导入完全静默」这个形态 —— 换成面板本地 notice 等于原样复发。
       * 通道在批 1 已经真注入（`MobileApp` 挂了 `MobileToaster`，`setToastImpl` 有实现）。
       * **失败仍留在面板内**（下面的 catch）：表还开着，错误该跟着表活着。
       * staged 与直落盘分开说：前者一个字节都没落盘，说「已导入」会让用户跳过待应用条上的「保存」。
       */
      if (rejected.length > 0) {
        toast.info(
          t('nodes.importSingletonSkipped', { count: admitted.length, skipped: rejected.length }),
        );
      } else {
        toast.success(
          staged
            ? t('nodes.importStagedOk', { count: admitted.length })
            : t('nodes.importOk', { count: admitted.length }),
        );
      }
      generation.current += 1;
      /* 产物一律落**自建**分组：当前 tab 停在某条订阅上时，不切过去屏上零变化（同桌面
         `NodesHeader.tsx:111` 的 `onAdded: () => setActiveTab('manual')`）。 */
      onAdded?.();
      closeInstance(instanceId);
    } catch (e) {
      console.error('[mobile-import] import failed:', e);
      setNotice({ tone: 'err', text: t('import.failed') });
    } finally {
      if (hasInstance(instanceId)) setImporting(false);
    }
  };

  /** 客户端轻量识别（只做预览提示；权威解析在 Rust `local_import_parse`）。 */
  const linkCount = content
    .trim()
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => isSupportedShareUrl(l)).length;

  return (
    <FormSheet
      title={t('import.title')}
      onRequestClose={requestClose}
      closeLocked={importing}
      closeLabel={t('common.close')}
      cancelLabel={t('common.cancel')}
      submitLabel={
        preview === null
          ? t('import.parse')
          : t('import.submitN', { count: preview.nodes.length })
      }
      submitDisabled={picking || localImportPrimaryActionDisabled(parsing, importing)}
      onSubmit={() => void (preview === null ? parse() : doImport())}
      notice={notice}
    >
      <div className="m-form-row">
        <label className="m-form-label" htmlFor="mif-text">
          {t('import.pasteLabel')}
        </label>
        <textarea
          id="mif-text"
          className="m-form-input m-form-area mono"
          rows={7}
          value={content}
          placeholder={t('import.textPlaceholder')}
          onChange={(e) => onContentChange(e.target.value)}
        />
        {linkCount > 0 && <p className="m-form-hint">{t('import.detLinks', { n: linkCount })}</p>}
        <div className="m-form-inline">
          <button
            type="button"
            className="m-form-btn"
            disabled={picking || localImportPrimaryActionDisabled(parsing, importing)}
            onClick={() => void pickFile()}
          >
            {t('import.pickFile')}
          </button>
        </div>
      </div>

      {preview !== null && (
        <div className="m-form-row">
          {/* 计数三格逐条对齐桌面 `ImportDialog` 预览块（`import-preview-parity.test.ts` 从那一块现场取材核对）。
              「跳过 / 失败」只在非零时出现 —— 恒显示会让「一切正常」与「丢了 3 个」长得一样。
              本地 sing-box 文件里生成侧会拒收的 MASQUE / Tailcat 节点就计在 `stats.failed` 上，
              这一行是用户在手机上唯一能看出「少了几个」的地方。 */}
          <p className="m-form-hint">
            {t('import.parsed', { nodes: preview.nodes.length, subs: preview.subscriptions.length })}
          </p>
          {preview.stats.unsupported > 0 && (
            <p className="m-form-hint">
              {t('import.resultUnsupported', { count: preview.stats.unsupported })}
              {/* 桌面把这句挂在悬浮提示上；触屏没有悬浮，直接作为说明跟在计数后面。 */}
              {' '}
              {t('import.unsupportedTip')}
            </p>
          )}
          {preview.stats.skipped + preview.stats.failed > 0 && (
            <p className="m-form-err">
              {t('import.skippedTitle', { count: preview.stats.skipped + preview.stats.failed })}
            </p>
          )}
          {/* 后端算出来的 `warnings` 逐条显示 —— 桌面此前把它们整个丢掉正是本表单拆两步的理由。
              证书固定「只比对服务器证书本身」的提示（Rust `tls_pin::cert_pin_import_warning`）
              与 MASQUE / Tailcat 被剥掉的键都走这一条。 */}
          {preview.warnings !== undefined &&
            preview.warnings.length > 0 &&
            preview.warnings.map((w) => (
              <p key={w} className="m-form-hint">
                {w}
              </p>
            ))}
          {preview.nodes.some((node) => node.protocol === 'tailscale') && (
            <p className="m-form-hint">{t('import.tailscaleJoinHint')}</p>
          )}
          <span className="m-form-label">{t('import.nodesTitle')}</span>
          <ul className="m-form-list">
            {preview.nodes.map((n, i) => (
              <li key={`${n.id === '' ? n.name : n.id}-${i}`}>
                <b>{n.name}</b>
                {/* 后端 `unsupported` 的判据就是 `protocol === 'custom'`（同桌面预览块）。 */}
                {n.protocol === 'custom' ? (
                  <span className="m-form-err">{t('import.unsupportedBadge')}</span>
                ) : (
                  <span className="m-form-hint">{n.protocol}</span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </FormSheet>
  );
}
