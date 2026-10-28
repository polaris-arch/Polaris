import { useTranslation } from 'react-i18next';
import type { SubscriptionUserAgentModel } from '@/hooks/use-subscription-user-agent';

/** Shared desktop/mobile read-only hint. Custom input remains optional and untouched. */
export function SubscriptionUserAgentHint({ id, model, className = 'fld-hint', retryClassName = 'btn ghost sm' }: {
  id: string;
  model: SubscriptionUserAgentModel;
  className?: string;
  retryClassName?: string;
}) {
  const { t } = useTranslation();
  return <div id={id} className={className} role="status" style={{ overflowWrap: 'anywhere', whiteSpace: 'normal' }}>
    {model.status === 'loading' ? t('sub.uaLoading') : model.status === 'failed' ? <>
      <span>{t('sub.uaReadFailed')}</span>{' '}
      <button type="button" className={retryClassName} onClick={model.refresh}>{t('common.retry')}</button>
    </> : <>
      <div>{t('sub.uaVersions', { app: model.info.appVersion, core: model.info.coreVersion })}</div>
      <div>{t('sub.uaDefault')}{' '}<bdi className="mono">{model.info.defaultUserAgent}</bdi></div>
      {model.info.effectiveStatus === 'ready' ?
        <div>{model.info.source === 'subscription' ? t('sub.uaEffectiveCustom')
          : model.info.source === 'global' ? t('sub.uaEffectiveGlobal') : t('sub.uaEffectiveDefault')}{' '}
          <bdi className="mono">{model.info.effectiveUserAgent}</bdi></div> : <div>
          <span>{t('sub.uaEffectiveReadFailed')}</span>{' '}
          <button type="button" className={retryClassName} onClick={model.refresh}>{t('common.retry')}</button>
        </div>}
    </>}
  </div>;
}
