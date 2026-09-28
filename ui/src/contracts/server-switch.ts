/** Saving an exit choice does not prove that the running core applied it. */
export interface ServerSwitchReceipt {
  status: 'applied' | 'pending' | 'notRunning' | 'deferred' | 'superseded';
  reason?: 'nodeRequiresApply';
}
