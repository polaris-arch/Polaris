import type { AutoNodeSwitchedPayload } from '@/contracts/auto-select';
import { AUTO_SELECT_CAUSES } from '@/contracts/auto-select';
import type { AutoSelectT } from './auto-select-actions';

/** Switch events are confirmed results. Their cause determines the message, never an assumed fault. */
export function autoNodeSwitchFeedback(event: AutoNodeSwitchedPayload, t: AutoSelectT): string {
  const cause = event.leg === 'failover' && event.cause === 'failover' ? 'failover'
    : event.leg === 'select' && event.cause !== 'failover' && AUTO_SELECT_CAUSES.includes(event.cause) ? event.cause : 'unknown';
  return t(`autoSelect.event.${cause}`, { name: event.newServerName });
}
