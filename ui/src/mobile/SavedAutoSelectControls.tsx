import type { TFunction } from 'i18next';
import { useAppStore } from '@/store/app-store';
import { AutoSelectControls } from '@/mobile/AutoSelectControls';

export function SavedAutoSelectControls({ t, query, showStatus }: { t: TFunction; query?: string; showStatus?: boolean }) {
  const config = useAppStore(state => state.config);
  const accept = useAppStore(state => state.acceptSelectionConfig);
  return <AutoSelectControls config={config} accept={accept} t={t} query={query} showStatus={showStatus} getCurrentConfig={() => useAppStore.getState().config} />;
}
