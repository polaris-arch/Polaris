import { useTranslation } from 'react-i18next';
import { MobileInfo } from '../MobileInfo';
import type { SubscriptionUserAgentModel } from '@/hooks/use-subscription-user-agent';

/** Mobile presentation: both retry controls belong to the mobile scan and CSS surface. */
export function MobileSubscriptionUserAgentHint({ id, model }: {
  id: string;
  model: SubscriptionUserAgentModel;
}) {
  const { t } = useTranslation();
  const metadata = model.status === 'ready' ? <>
    <div>{t('sub.uaVersions', { app: model.info.appVersion, core: model.info.coreVersion })}</div>
    {(model.info.effectiveStatus !== 'ready' || model.info.source !== 'default') &&
      <div>{t('sub.uaDefault')}{' '}<bdi className="mono">{model.info.defaultUserAgent}</bdi></div>}
  </> : null;
  return <div id={id} className="m-form-hint" role="status" style={{ overflowWrap: 'anywhere', whiteSpace: 'normal' }}>
    {model.status === 'loading' ? t('sub.uaLoading') : model.status === 'failed' ? <>
      <span>{t('sub.uaReadFailed')}</span>{' '}
      <button type="button" className="m-form-btn" onClick={model.refresh}>{t('common.retry')}</button>
    </> : <>
      {model.info.effectiveStatus === 'ready' ?
        <div>{model.info.source === 'subscription' ? t('sub.uaEffectiveCustom')
          : model.info.source === 'global' ? t('sub.uaEffectiveGlobal') : t('sub.uaEffectiveDefault')}{' '}
          <bdi className="mono">{model.info.effectiveUserAgent}</bdi>{' '}
          <MobileInfo title={t('sub.uaDetails')} details={metadata} /></div> : <div>
          <span>{t('sub.uaEffectiveReadFailed')}</span>{' '}
          <MobileInfo title={t('sub.uaDetails')} details={metadata} />{' '}
          <button type="button" className="m-form-btn" onClick={model.refresh}>{t('common.retry')}</button>
        </div>}
    </>}
  </div>;
}
