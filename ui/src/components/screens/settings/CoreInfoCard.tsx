/**
 * sing-box 内核只读信息卡。
 *
 * 内核随应用一同发布，应用内没有任何更换内核的入口，故本卡只陈述三个读数：
 * 安装包内内核文件自报的版本、随包清单声明的版本、补丁集标识。卡内不放按钮、开关或下拉。
 *
 * 第一格**不是**「正在运行的内核」的版本：经提权助手运行时（如 TUN 模式）执行的是助手目录里的
 * 那一份，可能与随包文件不同；本卡不对「实际运行的是哪一份」作任何断言。
 */
import { useTranslation } from 'react-i18next';
import type { CoreVersionInfo } from '@/contracts/types/update';
import { SetBlock, SetRow } from './Primitives';

export interface CoreInfoCardProps {
  /** `null` = 还没读到（或读取失败）：两个版本格显示占位，不画补丁集行。 */
  info: CoreVersionInfo | null;
}

/**
 * `packagedVersion` 为空串表示后端没能从安装包内的内核文件读出版本；此时显示占位而不是拿
 * 清单声明的版本顶替 —— 两格同值只应来自真实读数。
 */
export default function CoreInfoCard({ info }: CoreInfoCardProps) {
  const { t } = useTranslation();
  return (
    <SetBlock id="core-info-card" header={t('settings.update.coreCard')}>
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
      {info?.patchSet && (
        <SetRow label={t('settings.update.corePatchSet')}>
          <span className="mono" data-core-info="patch-set">
            {info.patchSet}
          </span>
        </SetRow>
      )}
    </SetBlock>
  );
}
