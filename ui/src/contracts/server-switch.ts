/** Backend receipt for an explicit exit selection. Saving the choice is not proof that the live core applied it. */
export interface ServerSwitchReceipt {
  status: 'applied' | 'pending' | 'restarting' | 'notRunning' | 'deferred' | 'superseded';
  reason?: 'nodeRequiresApply';
}
