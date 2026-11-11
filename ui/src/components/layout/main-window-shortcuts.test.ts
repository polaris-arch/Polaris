import { describe, expect, it } from 'vitest';
import {
  isMainWindowQuitShortcut,
  type MainWindowShortcutEvent,
  type MainWindowShortcutPlatform,
} from './main-window-shortcuts';

const CTRL_Q: MainWindowShortcutEvent = {
  key: 'q',
  ctrlKey: true,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  repeat: false,
  isComposing: false,
};

describe('主窗退出快捷键', () => {
  it.each<MainWindowShortcutPlatform>(['win', 'lin'])(
    '%s 由自绘主窗承接 Ctrl+Q',
    (platform) => {
      expect(isMainWindowQuitShortcut(CTRL_Q, platform)).toBe(true);
    },
  );

  it('macOS 的退出快捷键只归系统应用菜单', () => {
    expect(isMainWindowQuitShortcut(CTRL_Q, 'mac')).toBe(false);
    expect(
      isMainWindowQuitShortcut({ ...CTRL_Q, ctrlKey: false, metaKey: true }, 'mac'),
    ).toBe(false);
  });

  it.each([
    { ...CTRL_Q, key: 'w' },
    { ...CTRL_Q, ctrlKey: false },
    { ...CTRL_Q, metaKey: true },
    { ...CTRL_Q, altKey: true },
    { ...CTRL_Q, shiftKey: true },
    { ...CTRL_Q, repeat: true },
    { ...CTRL_Q, isComposing: true },
  ])('拒绝非精确 Ctrl+Q 组合 %#', (event) => {
    expect(isMainWindowQuitShortcut(event, 'lin')).toBe(false);
  });
});
