import { useTranslation } from 'react-i18next';
import { InfoIcon } from '@/components/InfoIcon';
import type { SubscriptionUserAgentModel } from '@/hooks/use-subscription-user-agent';

/** Desktop presentation of the shared read model; mobile controls stay in the mobile tree. */
export function SubscriptionUserAgentHint({ id, model, className = 'fld-hint', retryClassName = 'btn ghost sm' }: {
  id: string;
  model: SubscriptionUserAgentModel;
  className?: string;
  retryClassName?: string;
}) {
  const { t } = useTranslation();
  const metadata = model.status === 'ready' ? [
    t('sub.uaVersions', { app: model.info.appVersion, core: model.info.coreVersion }),
    model.info.effectiveStatus !== 'ready' || model.info.source !== 'default'
      ? `${t('sub.uaDefault')} ${model.info.defaultUserAgent}` : null,
  ].filter(Boolean).join(' · ') : '';
  return <div id={id} className={className} role="status" style={{ overflowWrap: 'anywhere', whiteSpace: 'normal' }}>
    {model.status === 'loading' ? t('sub.uaLoading') : model.status === 'failed' ? <>
      <span>{t('sub.uaReadFailed')}</span>{' '}
      <button type="button" className={retryClassName} onClick={model.refresh}>{t('common.retry')}</button>
    </> : <>
      {model.info.effectiveStatus === 'ready' ?
        <div>{model.info.source === 'subscription' ? t('sub.uaEffectiveCustom')
          : model.info.source === 'global' ? t('sub.uaEffectiveGlobal') : t('sub.uaEffectiveDefault')}{' '}
          <bdi className="mono">{model.info.effectiveUserAgent}</bdi>{' '}<InfoIcon tip={metadata} /></div> : <div>
          <span>{t('sub.uaEffectiveReadFailed')}</span>{' '}<InfoIcon tip={metadata} />{' '}
          <button type="button" className={retryClassName} onClick={model.refresh}>{t('common.retry')}</button>
        </div>}
    </>}
  </div>;
}
