import type { AutoSelectT } from '@/lib/auto-select-actions';
import { useAppStore } from '@/store/app-store';
import { AutoSelectControls, type AutoSelectControlsProps } from '@/mobile/AutoSelectControls';

export function SavedAutoSelectControls({ t, ...props }: { t: AutoSelectT } & Pick<AutoSelectControlsProps, 'query' | 'showStatus' | 'target' | 'variant'>) {
  const config = useAppStore(state => state.config);
  const accept = useAppStore(state => state.acceptSelectionConfig);
  return <AutoSelectControls config={config} accept={accept} t={t} {...props} getCurrentConfig={() => useAppStore.getState().config} />;
}
