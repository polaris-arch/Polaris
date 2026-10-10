//! 版本比较的单一权威：[`compare_semver`]（完整 semver 优先级，含 prerelease）。

use thiserror::Error;

/// 版本比较失败（仅「空版本」属于硬错误；不可解析的段在 [`compare_semver`] 中按 0 计而非报错）。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseVersionError {
    /// 版本字符串为空。
    #[error("empty version string")]
    Empty,
}

/// 解析出主版本段数组与 prerelease 段数组（build `+...` 直接丢弃，不参与比较）。
///
/// 移植自 `version.ts:compareSemver` 内部 `parse`。缺失或非数字主版本段按 0 计
/// （上游 `parseInt(p, 10) || 0`）。
fn parse_semver(v: &str) -> (Vec<u64>, Vec<String>) {
    let stripped = v.trim().trim_start_matches(['v', 'V']);
    // build `+...` 丢弃（semver 规范：build metadata 不参与优先级）。
    let no_build = stripped.split('+').next().unwrap_or_default();
    let (main_str, pre_str) = no_build
        .split_once('-')
        .map_or((no_build, ""), |(m, p)| (m, p));

    let main: Vec<u64> = main_str
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    // 空 pre_str → 空 Vec（=「无 prerelease」）；非空 → 按 '.' 切分。
    let pre: Vec<String> = if pre_str.is_empty() {
        Vec::new()
    } else {
        pre_str.split('.').map(ToString::to_string).collect()
    };
    (main, pre)
}

/// 比较两 prerelease 段标识符（semver 规范）。
///
/// 移植自 `version.ts` prerelease 段比较规则：
///   - 纯数字段按数值比（`1` < `2`）
///   - 含字母段按 ASCII 字典序（`alpha` < `beta`）
///   - 数字段 < 字母段（`1` < `alpha`）
///
/// 返回 [`core::cmp::Ordering`]：[`Equal`](core::cmp::Ordering::Equal) 仅当两标识符同型且同值。
fn cmp_pre_ident(a: &str, b: &str) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    match (a.parse::<u64>(), b.parse::<u64>()) {
        // 两段都是纯数字 → 数值比较。
        (Ok(na), Ok(nb)) => na.cmp(&nb),
        // a 数字、b 字母 → a < b。
        (Ok(_), Err(_)) => Ordering::Less,
        // a 字母、b 数字 → a > b。
        (Err(_), Ok(_)) => Ordering::Greater,
        // 两段都是字母 → ASCII 字典序。
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// 比较两 prerelease 段数组（semver 规范）。
///
/// 移植自 `version.ts` prerelease 比较：逐段比，首个差异决定优先级；段数不同时**较长者更大**
/// （`alpha` < `alpha.1`）。两数组都为空（即都无 prerelease）→ [`Equal`](core::cmp::Ordering::Equal)。
fn cmp_pre(a: &[String], b: &[String]) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    for (pa, pb) in a.iter().zip(b.iter()) {
        let o = cmp_pre_ident(pa, pb);
        if o != Ordering::Equal {
            return o;
        }
    }
    // 公共前缀全等 → 段数多者更大（`alpha` < `alpha.1`）。
    a.len().cmp(&b.len())
}

/// 健壮三段（及以上）语义版本比较的单一权威（移植自 `version.ts:compareSemver`）。
///
/// 返回值：`>0` → `a` 更新（= Polaris 返回 `1`）；`<0` → `b` 更新（= Polaris 返回 `-1`）；`0` → 相等。
///
/// 行为（逐字对齐 上游 `compareSemver` 注释）：
///   - 容忍前导 `v`（`"v1.13.13"`）与 build 后缀（`+naive`，比较时忽略）
///   - 支持 semver prerelease 优先级：
///     - 主版本号（major.minor.patch…）数字段优先逐段比较
///     - 主版本相同时「有 prerelease」< 「无 prerelease」（`1.14.0-alpha.32 < 1.14.0`）
///     - 两者都有 prerelease 时逐段比 prerelease 标识符（数字段按数字、字母段按 ASCII、数字段 < 字母段）
///     - 段数不同时较长者更大（`alpha < alpha.1`）
///   - 由此 `alpha.32 < alpha.33 < beta.1 < rc.1 < 1.14.0 < 1.14.1` 全链成立
///   - 每段缺失或非数字主版本段按 0 计；build（`+`）后缀不参与比较（semver 规范）
///
/// # Errors
///
/// 仅当 `a` 或 `b` 为空串时返回 [`ParseVersionError::Empty`]（Polaris 原实现容忍空串但产出无意义比较结果；
/// 显式报错让调用方不至于在「空 vs 空」上误判「相等」进而漏更新）。
pub fn compare_semver(a: &str, b: &str) -> Result<i32, ParseVersionError> {
    if a.trim().is_empty() {
        return Err(ParseVersionError::Empty);
    }
    if b.trim().is_empty() {
        return Err(ParseVersionError::Empty);
    }

    let (main_a, pre_a) = parse_semver(a);
    let (main_b, pre_b) = parse_semver(b);

    // 1) 主版本段逐段比较（缺失段按 0）。
    let max_len = main_a.len().max(main_b.len());
    for i in 0..max_len {
        let na = main_a.get(i).copied().unwrap_or(0);
        let nb = main_b.get(i).copied().unwrap_or(0);
        match na.cmp(&nb) {
            core::cmp::Ordering::Equal => continue,
            core::cmp::Ordering::Greater => return Ok(1),
            core::cmp::Ordering::Less => return Ok(-1),
        }
    }

    // 2) 主版本全等 → 比较 prerelease：
    //    - 都无 pre（空）→ Equal（0）
    //    - a 有 pre、b 无 pre → a < b（ prerelease 优先级低于正式版）
    //    - a 无 pre、b 有 pre → a > b
    //    - 都有 pre → 逐段比 prerelease
    use core::cmp::Ordering;
    let ord = match (pre_a.is_empty(), pre_b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater, // a 无 pre > b 有 pre
        (false, true) => Ordering::Less,    // a 有 pre < b 无 pre
        (false, false) => cmp_pre(&pre_a, &pre_b),
    };
    Ok(match ord {
        Ordering::Equal => 0,
        Ordering::Greater => 1,
        Ordering::Less => -1,
    })
}

#[cfg(test)]
mod tests;
