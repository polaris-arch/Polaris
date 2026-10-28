import { useTranslation } from 'react-i18next';
import type { SubscriptionUserAgentModel } from '@/hooks/use-subscription-user-agent';

/** Mobile presentation: both retry controls belong to the mobile scan and CSS surface. */
export function MobileSubscriptionUserAgentHint({ id, model }: {
  id: string;
  model: SubscriptionUserAgentModel;
}) {
  const { t } = useTranslation();
  return <div id={id} className="m-form-hint" role="status" style={{ overflowWrap: 'anywhere', whiteSpace: 'normal' }}>
    {model.status === 'loading' ? t('sub.uaLoading') : model.status === 'failed' ? <>
      <span>{t('sub.uaReadFailed')}</span>{' '}
      <button type="button" className="m-form-btn" onClick={model.refresh}>{t('common.retry')}</button>
    </> : <>
      <div>{t('sub.uaVersions', { app: model.info.appVersion, core: model.info.coreVersion })}</div>
      <div>{t('sub.uaDefault')}{' '}<bdi className="mono">{model.info.defaultUserAgent}</bdi></div>
      {model.info.effectiveStatus === 'ready' ?
        <div>{model.info.source === 'subscription' ? t('sub.uaEffectiveCustom')
          : model.info.source === 'global' ? t('sub.uaEffectiveGlobal') : t('sub.uaEffectiveDefault')}{' '}
          <bdi className="mono">{model.info.effectiveUserAgent}</bdi></div> : <div>
          <span>{t('sub.uaEffectiveReadFailed')}</span>{' '}
          <button type="button" className="m-form-btn" onClick={model.refresh}>{t('common.retry')}</button>
        </div>}
    </>}
  </div>;
}
