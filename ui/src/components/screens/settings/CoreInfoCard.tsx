/**
 * sing-box 内核只读信息卡。
 *
 * 内核随应用一同发布。本卡陈述随包版本与助手选定内容安装状态；
 * 完整身份收进只读诊断；内容损坏可显式核验随包恢复源，或重装可信应用。
 *
 * 第一格**不是**「正在运行的内核」的版本：经提权助手运行时（如 TUN 模式）执行的是助手目录里的
 * 那一份，可能与随包文件不同；本卡不对「实际运行的是哪一份」作任何断言。
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { revealOnToggle } from '@/components/reveal';
import { coreAdmissionIsDamage, coreAdmissionMessageKey } from '@/domain/core-admission';
import type { HelperStatus } from '@/contracts/types/runtime';
import type { CoreVersionInfo } from '@/contracts/types/update';
import { Button, SetBlock, SetRow } from './Primitives';
import CoreRecoveryActions from './CoreRecoveryActions';

export interface CoreInfoCardProps {
  /** `null` = 还没读到（或读取失败）：两个版本格显示占位。 */
  info: CoreVersionInfo | null;
  selectedCore?: HelperStatus['selectedCore'];
  onRecovered?: (result: 'restored' | 'notNeeded') => void;
  recoveryComplete?: boolean;
}

/**
 * `packagedVersion` 为空串表示后端没能从安装包内的内核文件读出版本；此时显示占位而不是拿
 * 清单声明的版本顶替 —— 两格同值只应来自真实读数。
 */
export default function CoreInfoCard({ info, selectedCore, onRecovered, recoveryComplete }: CoreInfoCardProps) {
  const { t } = useTranslation();
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const digest = (value: unknown) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value) ? value : null;
  const state = selectedCore?.state;
  const observedState = state === 'present' || state === 'absent' || state === 'invalid' || state === 'unavailable' ? state : null;
  // Do not serialize the source object: future wire fields may contain private data.
  const diagnostics = selectedCore ? {
    selectedCore: { id: digest(selectedCore.id), providerFingerprint: digest(selectedCore.providerFingerprint), state: observedState },
  } : null;
  async function copyDiagnostics() {
    if (!diagnostics) return;
    setCopyState('idle');
    try {
      await navigator.clipboard.writeText(JSON.stringify(diagnostics, null, 2));
      setCopyState('copied');
    } catch {
      setCopyState('failed');
    }
  }
  return (
    <SetBlock id="core-info-card" header={t('settings.update.coreCard')}>
      {recoveryComplete && <p role="status" style={{ padding: '0 18px 16px' }}>{t('coreRecovery.restored')}</p>}
      {info?.admission?.state === 'rejected' && (
        <div className="card-sub" role="status" data-core-admission={info.admission.kind} style={{ padding: '0 18px 16px' }}>
          <p>{t(coreAdmissionMessageKey(info.admission.kind))}</p>
          {((coreAdmissionIsDamage(info.admission.kind) && info.admission.recovery === 'reinstallCurrentApp')
            || ['UnsupportedExactObject', 'UnsupportedCapability', 'LoaderPolicy'].includes(info.admission.kind ?? '')) && (
            <CoreRecoveryActions onRecovered={onRecovered} />
          )}
          {info.admission.kind === 'UnsupportedCapability' && (
            <a href="#app-update-card">{t('settings.about.checkUpdate')}</a>
          )}
        </div>
      )}
      <SetRow label={t('settings.update.corePackaged')} tip={t('settings.update.coreInfoTip')}>
        <span className="mono" data-core-info="packaged">
          {info?.packagedVersion || '—'}
        </span>
      </SetRow>
      <SetRow label={t('settings.update.coreBundled')}>
        <span className="mono" data-core-info="bundled">
          {info?.bundledVersion || '—'}
        </span>
      </SetRow>
      <SetRow label={t('helper.selectedCoreVersion')}>
        <span className="mono" data-core-info="selected-version">{selectedCore?.version || '—'}</span>
      </SetRow>
      <SetRow label={t('helper.selectedCoreState')}>
        <span data-core-info="installed-state">{observedState ? t(`helper.coreState.${observedState}`) : t('helper.checkUnknown')}</span>
      </SetRow>
      <div className="card-sub" style={{ padding: '0 18px 16px' }}>
        <p>{t('helper.checkScope')}</p>
        <details data-testid="core-diagnostics" onToggle={revealOnToggle}>
          <summary>{t('helper.coreDiagnosticsTitle')}</summary>
          <dl style={{ overflowWrap: 'anywhere' }}>
            <dt>{t('helper.selectedCoreId')}</dt>
            <dd className="mono" data-core-info="selected-id">{diagnostics?.selectedCore.id ?? t('helper.checkUnknown')}</dd>
            <dt>{t('helper.selectedCoreProvider')}</dt>
            <dd className="mono" data-core-info="provider">{diagnostics?.selectedCore.providerFingerprint ?? t('helper.checkUnknown')}</dd>
          </dl>
          <p>{t('helper.providerFingerprintTip')}</p>
          <Button variant="ghost" size="sm" onClick={copyDiagnostics} disabled={!diagnostics}>
            <span>{t('helper.copyDiagnostics')}</span>
          </Button>
          {copyState !== 'idle' && <p role="status">{t(copyState === 'copied' ? 'helper.diagnosticsCopied' : 'helper.diagnosticsCopyFailed')}</p>}
        </details>
      </div>
    </SetBlock>
  );
}
