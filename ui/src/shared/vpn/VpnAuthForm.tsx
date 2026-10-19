import { useEffect, useLayoutEffect, useId, useMemo, useRef, useState, type ReactNode, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { api } from '@/ipc';
import { toast } from '@/lib/error-handler';
import { useAppStore } from '@/store/app-store';
import { useVpnStatusStore } from '@/store/use-vpn-status-store';

export interface VpnAuthFrame {
  title: string; requestClose: () => void; submitting: boolean; canSubmit: boolean;
  submit: () => void; openBrowser?: () => void; children: ReactNode;
}
export interface VpnAuthFormProps {
  protocol: 'openconnect' | 'openvpn'; serverId: string; close: () => void;
  renderFrame: (frame: VpnAuthFrame) => ReactElement; mobile?: boolean;
}
// The shell stays mounted across challenges: do not reorder Back layers or steal focus.
export function VpnAuthForm({ protocol, serverId, close, renderFrame, mobile = false }: VpnAuthFormProps) {
  const { t } = useTranslation();
  const inputPrefix = useId();
  const serverName = useAppStore(
    (state) => state.servers.find((server) => server.id === serverId)?.name
  );
  const openConnect = useVpnStatusStore((state) => state.openConnect[serverId]);
  const openVpn = useVpnStatusStore((state) => state.openVpn[serverId]);
  const challenge = protocol === 'openconnect' ? openConnect?.authChallenge : openVpn?.challenge;
  const challengeId = challenge?.id;
  const [submitting, setSubmitting] = useState(false);
  const active = useRef(true);
  const busy = useRef(false);
  const receipt = useRef(0);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const [formValues, setFormValues] = useState<Record<string, string>>({});
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [secret, setSecret] = useState('');
  const [finalURL, setFinalURL] = useState('');
  const [cookieValues, setCookieValues] = useState<Record<string, string>>({});
  const [headerValues, setHeaderValues] = useState<Record<string, string>>({});

  useLayoutEffect(() => {
    receipt.current++;
    busy.current = false;
    setSubmitting(false);
    setPassword(''); setSecret(''); setUsername('');
    setFormValues({}); setFinalURL(''); setCookieValues({}); setHeaderValues({});
    if (!challengeId) {
      close();
      return;
    }
    const latest = useVpnStatusStore.getState();
    const latestOpenConnect = latest.openConnect[serverId]?.authChallenge;
    const latestOpenVpn = latest.openVpn[serverId]?.challenge;
    if (protocol === 'openconnect' && latestOpenConnect) {
      const current = latestOpenConnect;
      setFormValues(Object.fromEntries(current.fields.map((field) => [field.submissionKey, field.value])));
      setFinalURL(current.browser?.finalURL ?? '');
      setCookieValues(Object.fromEntries((current.browser?.cookieNames ?? []).map((name) => [name, ''])));
      setHeaderValues(Object.fromEntries((current.browser?.headerNames ?? []).map((name) => [name, ''])));
    } else if (protocol === 'openvpn' && latestOpenVpn) {
      setUsername(latestOpenVpn.username ?? '');
      setPassword('');
      setSecret('');
    }
  }, [challengeId, close, protocol, serverId]);

  const title = `${protocol === 'openconnect' ? 'OpenConnect' : 'OpenVPN'} · ${
    serverName ?? t('vpnAuth.unknownNode')
  }`;
  const canSubmit = useMemo(() => {
    if (!challenge) return false;
    if (protocol === 'openconnect') return challenge.kind === 'form' || challenge.kind === 'browser';
    return !['message', 'open-url'].includes(challenge.kind);
  }, [challenge, protocol]);

  const requestClose = () => {
    active.current = false;
    receipt.current++;
    const id = challengeId;
    close();
    if (!id) return;
    const cancel =
      protocol === 'openconnect'
        ? api.vpn.cancelOpenConnect(serverId, id)
        : api.vpn.cancelOpenVpn(serverId, id);
    void cancel.catch(() => {});
  };

  const submit = async () => {
    if (!challengeId || !challenge || busy.current || !active.current) return;
    const submission = ++receipt.current;
    const stillCurrent = () => active.current && submission === receipt.current && (protocol === 'openconnect'
      ? useVpnStatusStore.getState().openConnect[serverId]?.authChallenge?.id
      : useVpnStatusStore.getState().openVpn[serverId]?.challenge?.id) === challengeId;
    busy.current = true;
    setSubmitting(true);
    try {
      if (protocol === 'openconnect' && openConnect?.authChallenge) {
        if (openConnect.authChallenge.kind === 'form') {
          await api.vpn.submitOpenConnectForm(serverId, challengeId, formValues);
        } else if (openConnect.authChallenge.kind === 'browser') {
          await api.vpn.submitOpenConnectBrowser(
            serverId,
            challengeId,
            finalURL,
            Object.entries(cookieValues).map(([name, value]) => ({ name, value })),
            Object.entries(headerValues).map(([name, value]) => ({
              name,
              values: value.split('\n').map((line) => line.trim()).filter(Boolean),
            }))
          );
        }
      } else if (protocol === 'openvpn' && openVpn?.challenge) {
        await api.vpn.submitOpenVpn(serverId, challengeId, username, password, secret);
      }
      const latest = useVpnStatusStore.getState();
      const currentId = protocol === 'openconnect'
        ? latest.openConnect[serverId]?.authChallenge?.id
        : latest.openVpn[serverId]?.challenge?.id;
      // A fresh challenge on the same mounted endpoint is not this submission's dialog outcome.
      if (active.current && submission === receipt.current && (!currentId || currentId === challengeId)) close();
    } catch {
      // Native errors can contain credentials; never log or render their payload.
      if (stillCurrent()) toast.error(t('vpnAuth.submitFailed'));
    } finally {
      if (stillCurrent()) { busy.current = false; setSubmitting(false); }
    }
  };

  const openChallengeURL = () => {
    const url =
      protocol === 'openconnect' ? openConnect?.authChallenge?.browser?.url : openVpn?.challenge?.url;
    if (url) void api.system.openExternal(url);
  };

  const openConnectChallenge = protocol === 'openconnect' ? openConnect?.authChallenge : undefined;
  const openVpnChallenge = protocol === 'openvpn' ? openVpn?.challenge : undefined;

  const openBrowser = (openConnectChallenge?.kind === 'browser' || openVpnChallenge?.kind === 'open-url') ? openChallengeURL : undefined;
  return renderFrame({ title, requestClose, submitting, canSubmit, submit: () => void submit(), openBrowser, children: <>
      {(openConnectChallenge?.banner || openConnectChallenge?.message || openVpnChallenge?.message) && (
        <div className={mobile ? "m-form-hint" : "note-box"} style={{ whiteSpace: 'pre-wrap' }}>
          {openConnectChallenge?.banner && <div>{openConnectChallenge.banner}</div>}
          <div>{openConnectChallenge?.message ?? openVpnChallenge?.message}</div>
        </div>
      )}
      {(openConnectChallenge?.error || openVpnChallenge?.previousError) && (
        <div className={mobile ? "m-form-notice err" : "note-box danger"} style={{ whiteSpace: 'pre-wrap' }}>
          {t('vpnAuth.submitFailed')}
        </div>
      )}

      {openConnectChallenge?.kind === 'form' &&
        openConnectChallenge.fields.map((field) => (
          <div className={mobile ? "m-form-row" : "fld"} key={field.submissionKey}>
            <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-${field.submissionKey}`}>
              {field.label || field.name}
            </label>
            {field.options.length > 0 ? (
              <select
                id={`${inputPrefix}-vpn-${field.submissionKey}`}
                className={mobile ? "m-form-input" : "input"}
                value={formValues[field.submissionKey] ?? ''}
                onChange={(event) =>
                  setFormValues((values) => ({ ...values, [field.submissionKey]: event.target.value }))
                }
              >
                {field.options.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label || option.value}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={`${inputPrefix}-vpn-${field.submissionKey}`}
                className={mobile ? "m-form-input" : "input"}
                type={field.kind === 'password' ? 'password' : 'text'}
                value={formValues[field.submissionKey] ?? ''}
                onChange={(event) =>
                  setFormValues((values) => ({ ...values, [field.submissionKey]: event.target.value }))
                }
              />
            )}
          </div>
        ))}

      {openConnectChallenge?.kind === 'browser' && (
        <>
          <div className={mobile ? "m-form-hint" : "card-sub"} style={{ marginBottom: 10 }}>
            {t('vpnAuth.browserResultHelp')}
          </div>
          <div className={mobile ? "m-form-row" : "fld"}>
            <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-final-url`}>{t('vpnAuth.finalURL')}</label>
            <input id={`${inputPrefix}-vpn-final-url`} className={mobile ? "m-form-input" : "input mono"} value={finalURL} onChange={(e) => setFinalURL(e.target.value)} />
          </div>
          {Object.keys(cookieValues).map((name) => (
            <div className={mobile ? "m-form-row" : "fld"} key={`cookie-${name}`}>
              <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-cookie-${name}`}>{t('vpnAuth.cookie')} · {name}</label>
              <input id={`${inputPrefix}-vpn-cookie-${name}`} className={mobile ? "m-form-input" : "input mono"} type="password" value={cookieValues[name]} onChange={(e) => setCookieValues((values) => ({ ...values, [name]: e.target.value }))} />
            </div>
          ))}
          {Object.keys(headerValues).map((name) => (
            <div className={mobile ? "m-form-row" : "fld"} key={`header-${name}`}>
              <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-header-${name}`}>{t('vpnAuth.header')} · {name}</label>
              <textarea id={`${inputPrefix}-vpn-header-${name}`} className={mobile ? "m-form-input" : "input mono"} rows={2} value={headerValues[name]} onChange={(e) => setHeaderValues((values) => ({ ...values, [name]: e.target.value }))} />
            </div>
          ))}
        </>
      )}

      {protocol === 'openvpn' && openVpnChallenge && !['message', 'open-url'].includes(openVpnChallenge.kind) && (
        <>
          {openVpnChallenge.kind === 'credentials' && (
            <div className={mobile ? "m-form-row" : "fld"}>
              <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-username`}>{t('vpnAuth.username')}</label>
              <input id={`${inputPrefix}-vpn-username`} className={mobile ? "m-form-input" : "input"} value={username} onChange={(e) => setUsername(e.target.value)} />
            </div>
          )}
          {openVpnChallenge.kind === 'credentials' && (
            <div className={mobile ? "m-form-row" : "fld"}>
              <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-password`}>{t('vpnAuth.password')}</label>
              <input id={`${inputPrefix}-vpn-password`} className={mobile ? "m-form-input" : "input"} type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
            </div>
          )}
          {(openVpnChallenge.kind === 'secret' || openVpnChallenge.secretMessage) && (
            <div className={mobile ? "m-form-row" : "fld"}>
              <label className={mobile ? "m-form-label" : "fld-l"} htmlFor={`${inputPrefix}-vpn-secret`}>
                {openVpnChallenge.secretMessage || t('vpnAuth.secret')}
              </label>
              <input id={`${inputPrefix}-vpn-secret`} className={mobile ? "m-form-input" : "input"} type={openVpnChallenge.echo ? 'text' : 'password'} value={secret} onChange={(e) => setSecret(e.target.value)} />
            </div>
          )}
        </>
      )}
    </> });
}
