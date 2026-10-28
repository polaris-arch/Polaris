/**
 * 「退出更新弹窗」在各 phase 下发哪个动作 —— 纯函数，单独成文件是为了能在 node 环境直测
 * （`main.ts` 一 import 就会碰 `document` / `./style.css`，测不了）。
 */
import type { PopupAction, PopupPhase } from '@/contracts/types/update';

/** Close is accepted in every backend phase, including when the rendered phase is stale. */
export function exitActionFor(_phase: PopupPhase | undefined): PopupAction {
  return 'close';
}
