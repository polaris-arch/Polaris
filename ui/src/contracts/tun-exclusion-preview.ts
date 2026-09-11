/**
 * TUN 排除面预览 —— Rust `builder::tun_exclusion_preview` 的线格式对位。
 *
 * 它给的是**生效值**，不是规则说明：`bypassLANList` 与 `tunConfig.inboundExcludeCidrs`
 * 哪张表在本平台真的进 TUN 是平台相关的（win32 两张都进、darwin 只进后者、Linux 一张都不进），
 * 而两张表在界面上长得一样。与其补几条会与代码漂移的平台文案，不如把内核实际吃到的那一份读回来。
 */

/** 生成期诊断（静默剔除的四类原因、Linux 恒忽略那条都在这里）。 */
export interface TunExclusionNote {
  /** `warn` / `info` / … —— 前端按它选样式，不解析文案。 */
  level: string;
  message: string;
}

export interface TunExclusionPreview {
  /**
   * 真正下发给内核的 `route_exclude_address`。
   *
   * **空数组是有意义的结果**（"本平台这份配置下一条都不排除"），不是"没算出来"。
   * 渲染时必须如实显示空态 —— 用任何兜底顶上就退回了本功能要终结的那类谎。
   */
  effective: string[];
  notes: TunExclusionNote[];
  /** 当前不是 TUN 模式时为 false，此时 `effective` 恒空且无意义。 */
  tunActive: boolean;
}
