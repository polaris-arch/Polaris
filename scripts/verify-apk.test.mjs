import assert from 'node:assert/strict';
import test from 'node:test';

import {
  ABIS,
  APK_DECLARED_PERMISSIONS,
  APK_PERMISSION_REGISTRY,
  ASSET_UP,
  DEFAULT_ABI,
  LICENSE_FILES,
  NATIVE_MARKERS,
  REQUIRED_ELF_SECTIONS,
  SRS_DIR,
  ZIP,
  apkViolations,
  countOccurrences,
  elfSectionNames,
  elfPageAlignment,
  isDebugSection,
  parseAapt2Permissions,
  parseArgs,
  permissionViolations,
  strippedReport,
  zipOverheadBound,
} from './verify-apk.mjs';

/** 夹具默认造缺省 ABI（arm64-v8a）的包；`ABI` 这个名字保留下来只是为了少改一堆现有用例。 */
const ABI = DEFAULT_ABI;

/**
 * `verify-apk.mjs` 的判据本体测试。
 *
 * 这里喂的是**合成输入**，不是真 APK：真 APK 只能证明「这一次的产物是对的」，
 * 证明不了「判据在产物出错时会红」。两者都要 —— 真 APK 的收据在 CI 腿与设计文档里，
 * 判据有没有牙由本文件负责。
 *
 * 每条否定用例都配一条**反向对照**（第一条 `未变异的合成 APK 全绿`）：没有它，
 * 「判据恒红」与「判据抓到了这次变异」在测试结果上完全一样。
 */

const ELF = Buffer.from([0x7f, 0x45, 0x4c, 0x46]);

/**
 * 造一份**结构上真的能被解析**的 ELF：头 + 载荷 + 节名字符串表 + 节表。
 *
 * 判据 ⑥ 读的是节表；拿「4 字节魔数 + 一坨字节」当夹具的话，每条用例都会红在
 * 「节表读不出来」上 —— 那样测到的是解析器的报错路径，不是判据本身。
 *
 * 两种 ELFCLASS 都能造：本仓 armv7/i686 是 ELF32、arm64/x86_64 是 ELF64，
 * 解析器要在两侧都对，夹具就得能喂出两侧。
 *
 * @param {object} o
 * @param {Array<string|[string, number]>} o.sections 节名（或 `[节名, sh_size]`）；
 *        索引 0 的 NULL 节与末尾的 `.shstrtab` 由本函数自己补
 * @param {Buffer} o.payload 夹在头与节表之间的字节（用来塞 cronet/naive 针）
 * @param {32|64}  o.cls
 * @param {number} o.machine `e_machine`（缺省 EM_AARCH64；判据 ② 拿它与 ABI 对拍）
 */
function fakeElf({
  sections = [...REQUIRED_ELF_SECTIONS],
  payload = Buffer.alloc(0),
  cls = 64,
  machine = ABIS[DEFAULT_ABI].machine,
} = {}) {
  const is64 = cls === 64;
  const EHDR = is64 ? 64 : 52;
  const SHENT = is64 ? 64 : 40;
  const PHENT = is64 ? 56 : 32;
  const payloadOffset = EHDR + 2 * PHENT;

  const entries = [['', 0], ...sections.map((x) => (Array.isArray(x) ? x : [x, 16])), ['.shstrtab', 0]];
  const strParts = [];
  const nameOffs = [];
  let cur = 0;
  for (const [name] of entries) {
    nameOffs.push(cur);
    strParts.push(Buffer.from(`${name}\0`, 'latin1'));
    cur += Buffer.byteLength(name, 'latin1') + 1;
  }
  const strtab = Buffer.concat(strParts);

  const strtabOff = payloadOffset + payload.length;
  const shoff = strtabOff + strtab.length;
  const shnum = entries.length;
  const buf = Buffer.alloc(shoff + shnum * SHENT);

  ELF.copy(buf, 0);
  buf[4] = is64 ? 2 : 1; // EI_CLASS
  buf[5] = 1; // EI_DATA = 小端
  buf[6] = 1; // EI_VERSION
  buf.writeUInt16LE(machine, 18); // e_machine（e_ident 16 + e_type 2）
  payload.copy(buf, payloadOffset);
  if (is64) {
    buf.writeBigUInt64LE(BigInt(EHDR), 32);
    buf.writeUInt16LE(PHENT, 54);
    buf.writeUInt16LE(2, 56);
    for (let i = 0; i < 2; i++) {
      buf.writeUInt32LE(1, EHDR + i * PHENT);
      buf.writeBigUInt64LE(16384n, EHDR + i * PHENT + 48);
    }
  }
  strtab.copy(buf, strtabOff);

  if (is64) {
    buf.writeBigUInt64LE(BigInt(shoff), 40);
    buf.writeUInt16LE(SHENT, 58);
    buf.writeUInt16LE(shnum, 60);
    buf.writeUInt16LE(shnum - 1, 62); // `.shstrtab` 是最后一条
  } else {
    buf.writeUInt32LE(shoff, 32);
    buf.writeUInt16LE(SHENT, 46);
    buf.writeUInt16LE(shnum, 48);
    buf.writeUInt16LE(shnum - 1, 50);
  }

  entries.forEach(([name, size], i) => {
    const base = shoff + i * SHENT;
    buf.writeUInt32LE(nameOffs[i], base);
    buf.writeUInt32LE(i === 0 ? 0 : 1, base + 4); // sh_type：非 NULL 一律 PROGBITS
    if (name === '.shstrtab') {
      if (is64) {
        buf.writeBigUInt64LE(BigInt(strtabOff), base + 24);
        buf.writeBigUInt64LE(BigInt(strtab.length), base + 32);
      } else {
        buf.writeUInt32LE(strtabOff, base + 16);
        buf.writeUInt32LE(strtab.length, base + 20);
      }
    } else if (is64) {
      buf.writeBigUInt64LE(BigInt(size), base + 32);
    } else {
      buf.writeUInt32LE(size, base + 20);
    }
  });
  return buf;
}

/**
 * 造一个含 `cronet` × cronet 次、`naive` × naive 次的假 `.so`。
 * 默认节表是「已剥过」的形态（无任何 `.debug_*`），与真库一致。
 */
function fakeSo({ cronet = 500, naive = 100, elf = true, sections, cls, machine } = {}) {
  const so = fakeElf({
    cls,
    machine,
    sections: sections ?? [...REQUIRED_ELF_SECTIONS, '.rodata', '.dynstr', '.bss'],
    payload: Buffer.concat([
      Buffer.from('cronet'.repeat(cronet)),
      Buffer.from('naive'.repeat(naive)),
    ]),
  });
  if (!elf) Buffer.from('NOPE').copy(so, 0);
  return so;
}

/** 应用自己那份 `.so`（不带 cronet/naive 针）。 */
function fakeAppSo({ sections, cls, machine } = {}) {
  return fakeElf({
    cls,
    machine,
    sections: sections ?? [...REQUIRED_ELF_SECTIONS, '.rodata', '.symtab', '.strtab'],
    payload: Buffer.from('app'),
  });
}

const SRS_COUNT = 28;

/** 「什么都对」的合成 APK。用 `patch` 做定点变异；`abi` 决定原生库铺在哪、造成什么架构。 */
function goodApk(patch = {}, abi = DEFAULT_ABI) {
  const licenses = new Map(
    LICENSE_FILES.map((name) => [name, Buffer.from(`${name} 的真实内容\n`)]),
  );
  const members = new Map();
  for (const [name, bytes] of licenses) members.set(`${ASSET_UP}${name}`, bytes);
  const { elfClass, machine } = ABIS[abi];
  const cls = elfClass === 2 ? 64 : 32;
  members.set(`lib/${abi}/libbox.so`, fakeSo({ cls, machine }));
  members.set(`lib/${abi}/libpolaris_lib.so`, fakeAppSo({ cls, machine }));
  for (let i = 0; i < SRS_COUNT; i += 1) {
    members.set(`${ASSET_UP}${SRS_DIR}/geosite-${i}.srs`, Buffer.from('srs'));
  }
  // 填充到真实 APK 的量级（classes.dex / res / META-INF …），让扫描面自检那条不误红。
  for (let i = 0; i < 200; i += 1) members.set(`res/filler/${i}.xml`, Buffer.from('x'));

  // 死字节判据的两个新输入。合成 APK 里成员按「压缩后 = 原字节」算（真实 APK 的
  // `.so` / `resources.arsc` 也确实是 `stor` 不压缩），文件大小 = 条目和 + 一份**真实形状**
  // 的结构开销（本地头 30 + 中央头 46 + 文件名两遍 + EOCD 22）。
  // 刻意**不**用 zipOverheadBound() 来造 fileSize：那会让夹具与判据共用同一个算式，
  // 算式写错时两边一起错、测试照绿（判据被自己污染）。这里手写一遍最小真实开销。
  //
  // 这两个值在 `patch` **之前**算定，之后不再随 `_members` 的增删重算 —— 它们是两个独立旋钮，
  // 由死字节那组用例直接赋值。成员级变异（删一份 .srs / 一份许可文本）因此会让这两个数
  // 偏大几十字节，落在每条目 4096 字节的对齐余量里，够不到带的上沿，不会误红。
  const entriesCompressedTotal = [...members.values()].reduce((a, b) => a + b.length, 0);
  const realisticOverhead =
    members.size * (ZIP.LOCAL_HEADER + ZIP.CENTRAL_HEADER) +
    2 * [...members.keys()].reduce((a, n) => a + Buffer.byteLength(n), 0) +
    ZIP.EOCD;

  const input = {
    names: [...members.keys()],
    fileSize: entriesCompressedTotal + realisticOverhead,
    entriesCompressedTotal,
    readMember: (name) => {
      const hit = members.get(name);
      if (hit === undefined) throw new Error(`测试夹具里没有成员 ${name}`);
      return hit;
    },
    repoLicense: (name) => licenses.get(name) ?? null,
    repoSrsCount: SRS_COUNT,
    abi,
    // 判据 ⑦ 的取材面：默认造一份与登记表恰等的权限集（`patch` 可以就地改它）。
    permissions: {
      uses: APK_PERMISSION_REGISTRY.map((e) => e.name),
      declared: APK_DECLARED_PERMISSIONS.map((e) => e.name),
    },
    // 变异钩子（不是 apkViolations 的参数，仅供本文件内改夹具用）
    _members: members,
    _licenses: licenses,
  };
  patch(input);
  input.names = [...members.keys()];
  return input;
}

function run(patch = () => {}, { abi = DEFAULT_ABI, judgeAs = abi } = {}) {
  const { names, readMember, repoLicense, repoSrsCount, fileSize, entriesCompressedTotal, permissions } =
    goodApk(patch, abi);
  // `judgeAs` 与 `abi` 分开：造包用一个 ABI、判它用另一个，正是「参数没真的进判据」这条变异要的形状。
  return apkViolations({
    names,
    readMember,
    repoLicense,
    repoSrsCount,
    fileSize,
    entriesCompressedTotal,
    abi: judgeAs,
    permissions,
  });
}

/** 违反项里恰有 n 条提到 `needle`。 */
function hits(violations, needle) {
  return violations.filter((v) => v.includes(needle)).length;
}

test('反向对照：未变异的合成 APK 全绿', () => {
  assert.deepEqual(run(), []);
});

test('M1 形态：包里缺 NOTICE ⇒ 红，且恰红在 NOTICE 那一条', () => {
  const violations = run((input) => input._members.delete(`${ASSET_UP}NOTICE`));
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.equal(hits(violations, 'NOTICE'), 1);
  assert.equal(hits(violations, 'LICENSE'), 0);
  assert.equal(hits(violations, 'THIRD-PARTY'), 0);
});

test('M2 形态：三份许可文本全不在包里（`_up_` 整条逃逸）⇒ 三条都红', () => {
  const violations = run((input) => {
    for (const name of LICENSE_FILES) input._members.delete(`${ASSET_UP}${name}`);
  });
  assert.equal(violations.length, 3, violations.join(' | '));
  for (const name of LICENSE_FILES) assert.equal(hits(violations, `${ASSET_UP}${name} 不在`), 1);
});

test('许可文本被截断（在包里但与工作树不一致）⇒ 红', () => {
  const violations = run((input) =>
    input._members.set(`${ASSET_UP}LICENSE`, Buffer.from('MIT')),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /与工作树 LICENSE 不一致/);
});

test('工作树里的真值缺失或为空 ⇒ 红在真值那一侧，不冒充「APK 有问题」', () => {
  const missing = run((input) => input._licenses.delete('NOTICE'));
  assert.equal(missing.length, 1);
  assert.match(missing[0], /工作树里没有 NOTICE/);

  const empty = run((input) => input._licenses.set('NOTICE', Buffer.alloc(0)));
  assert.equal(empty.length, 1);
  assert.match(empty[0], /0 字节/);
});

test('M3 形态：libbox.so 里没有 naive ⇒ 红（判据本体有牙）', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ cronet: 500, naive: 0 })),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /'naive' 只出现 0 次/);
  assert.match(violations[0], /naive\/H3|naive outbound/);
});

test('cronet 被摘掉（with_naive_outbound 掉了）⇒ 两条都红', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ cronet: 0, naive: 0 })),
  );
  assert.equal(violations.length, 2, violations.join(' | '));
  assert.equal(hits(violations, "'cronet'"), 1);
  assert.equal(hits(violations, "'naive'"), 1);
});

test('阈值是严格大于：恰等于下限判红，下限 +1 判绿', () => {
  for (const { needle, min } of NATIVE_MARKERS) {
    const atLimit = run((input) =>
      input._members.set(
        `lib/${ABI}/libbox.so`,
        fakeSo(needle === 'cronet' ? { cronet: min, naive: 999 } : { cronet: 999, naive: min }),
      ),
    );
    assert.equal(hits(atLimit, `'${needle}' 只出现 ${min} 次`), 1, `${needle} 恰等于下限应判红`);

    const overLimit = run((input) =>
      input._members.set(
        `lib/${ABI}/libbox.so`,
        fakeSo(needle === 'cronet' ? { cronet: min + 1, naive: 999 } : { cronet: 999, naive: min + 1 }),
      ),
    );
    assert.deepEqual(overLimit, [], `${needle} 超出下限 1 应判绿`);
  }
});

test('libbox.so 整个缺席 ⇒ 红，且构建 tag 判据不在缺席的输入上假绿', () => {
  const violations = run((input) => input._members.delete(`lib/${ABI}/libbox.so`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libbox\.so 不在 APK 里/);
});

test('应用自己的 libpolaris_lib.so 缺席 ⇒ 红（libbox 在场不构成放行）', () => {
  const violations = run((input) => input._members.delete(`lib/${ABI}/libpolaris_lib.so`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libpolaris_lib\.so 不在 APK 里/);
});

test('读到的 .so 不是 ELF ⇒ 红（不拿一坨字节冒充共享库），且判据 ② 与 ⑥ 都说话', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ elf: false })),
  );
  // ② 魔数那条
  assert.equal(hits(violations, '不是 ELF（前四字节'), 1, violations.join(' | '));
  // ⑥ 节表那条：**这一条才是关键** —— 剥符号判据绝不能在「读到的根本不是 ELF」的输入上
  // 因为「没扫到 .debug_*」而判绿。两条各红一次，恰好 2 条。
  assert.equal(hits(violations, '节表读不出来'), 1, violations.join(' | '));
  assert.equal(violations.length, 2, violations.join(' | '));
});

test('.srs 份数与工作树不等 ⇒ 红', () => {
  const violations = run((input) => input._members.delete(`${ASSET_UP}${SRS_DIR}/geosite-0.srs`));
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], new RegExp(`有 ${SRS_COUNT - 1} 份 \\.srs`));
});

test('工作树 .srs 份数为零 ⇒ 红（`0 === 0` 不许判绿）', () => {
  const g = goodApk((input) => {
    for (const name of [...input._members.keys()]) {
      if (name.includes(`${SRS_DIR}/`)) input._members.delete(name);
    }
  });
  const { names, readMember, repoLicense } = g;
  const violations = apkViolations({
    names,
    readMember,
    repoLicense,
    permissions: g.permissions,
    repoSrsCount: 0,
    fileSize: g.fileSize,
    entriesCompressedTotal: g.entriesCompressedTotal,
    abi: DEFAULT_ABI,
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /真值为零/);
});

test('成员清单塌了（空 / 过短）⇒ 红在扫描面，不报成「许可文本不在包里」', () => {
  const violations = apkViolations({
    names: [],
    readMember: () => Buffer.alloc(0),
    repoLicense: () => Buffer.from('x'),
    repoSrsCount: SRS_COUNT,
    fileSize: 1,
    entriesCompressedTotal: 1,
    abi: DEFAULT_ABI,
  });
  assert.equal(violations.length, 1);
  assert.match(violations[0], /成员清单只有 0 条/);
});

test('countOccurrences 与 Python bytes.count 同语义（重叠不计）', () => {
  assert.equal(countOccurrences(Buffer.from('aaaa'), 'aa'), 2);
  assert.equal(countOccurrences(Buffer.from('abcabc'), 'abc'), 2);
  assert.equal(countOccurrences(Buffer.from('abc'), 'x'), 0);
});

// ───────────────────────── 死字节判据（判据 ⑤）─────────────────────────
//
// 反向对照沿用文件开头那条「未变异的合成 APK 全绿」：它现在也覆盖死字节这一条，
// 没有它，下面每一条「变异后判红」都与「判据恒红」不可分。

test('M4 形态：文件里多出一大段没有条目指向的字节 ⇒ 红，且恰红在死字节那一条', () => {
  // 越界幅度取真实脏包的量级：实测非条目字节 388058998 是上界 4295570 的 90 倍。
  // **不**用「比值 ×2.77」来驱动：合成夹具的条目总量只有几 KB，而上界含每条目一页
  // 对齐余量（231 条目 ≈ 0.9 MB），比值的带在这个尺度上宽得没有分辨力 —— 那是判据
  // 在小包上的真实性质（见 verify-apk.mjs 里 zipOverheadBound 的「射程与灵敏度下限」段），
  // 不是夹具的毛病。真实尺度上的红/绿对照由本文件最后一条用实测数字覆盖。
  const violations = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names) * 90;
  });
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /没有任何条目指向/);
  assert.match(violations[0], /超出带 \[1\.000000, /);
  // 报文必须点名：读的人要一眼看出差多少、往哪个方向修。
  assert.match(violations[0], /删掉这个 APK 再打一次/);
});

test('死字节变异**不**牵连其它四条判据（变异是定点的，不是把夹具打坏了）', () => {
  const violations = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names) * 90;
  });
  assert.equal(hits(violations, '不在 APK 里'), 0, violations.join(' | '));
  assert.equal(hits(violations, '只出现'), 0, violations.join(' | '));
  assert.equal(hits(violations, '.srs'), 0, violations.join(' | '));
});

test('带的上沿是严格大于：恰落在上沿判绿，上沿 +1 字节判红', () => {
  // hi = 1 + 上界/条目和  ⇔  fileSize = 条目和 + 上界。判据是 `ratio > hi` 才红。
  const atEdge = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names);
  });
  assert.deepEqual(atEdge, [], `恰在上沿应判绿，实为：${atEdge.join(' | ')}`);

  const overEdge = run((input) => {
    input.fileSize = input.entriesCompressedTotal + zipOverheadBound(input.names) + 1;
  });
  assert.equal(overEdge.length, 1, overEdge.join(' | '));
  assert.match(overEdge[0], /没有任何条目指向/);
});

test('带的下沿：文件比自己条目的压缩和还小 ⇒ 红在取材面，不冒充「包里有死字节」', () => {
  // 纯上限判据在这种输入上是**恒绿**的——下沿存在的全部理由就是这个。
  const violations = run((input) => {
    input.fileSize = input.entriesCompressedTotal - 1;
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /物理上不可能/);
  assert.match(violations[0], /取材面错了，不是包有问题/);
  assert.equal(hits(violations, '没有任何条目指向'), 0);
});

test('条目压缩和读不出来（null / 0）⇒ 红在取材面，不许空跑绿', () => {
  for (const bad of [null, 0, undefined, NaN]) {
    const violations = run((input) => {
      input.entriesCompressedTotal = bad;
    });
    assert.equal(violations.length, 1, `${bad} 应恰 1 条违反，实为：${violations.join(' | ')}`);
    assert.match(violations[0], /没能从 zip 读出条目尺寸/);
    assert.match(violations[0], /恒真，必须红在取材面/);
  }
});

test('文件大小读不出来（0 / NaN）⇒ 红在取材面', () => {
  for (const bad of [0, NaN, -1]) {
    const violations = run((input) => {
      input.fileSize = bad;
    });
    assert.equal(violations.length, 1, `${bad} 应恰 1 条违反，实为：${violations.join(' | ')}`);
    assert.match(violations[0], /取材面塌了/);
  }
});

test('zipOverheadBound 与逐项手算逐字对拍（算式本身没有被自己的实现证明）', () => {
  const names = ['a.txt', 'lib/arm64-v8a/libbox.so', '中文名.srs'];
  const nameBytes = names.reduce((a, n) => a + Buffer.byteLength(n, 'utf8'), 0);
  // 手写一遍：本地头 30 + 中央头 46 + data descriptor 24 + 本地 extra 4096，每条目一份；
  // 文件名存两遍；整包一个 EOCD 22 + zip64 的 56 + 20。
  const byHand = names.length * (30 + 46 + 24 + 4096) + 2 * nameBytes + 22 + 56 + 20;
  assert.equal(zipOverheadBound(names), byHand);
  // 名字按**字节**算，不是按字符：'中文名.srs' 是 7 个字符、13 个字节。
  assert.equal(Buffer.byteLength('中文名.srs', 'utf8'), 13); // 3 个 CJK ×3 + '.srs' 4
  assert.ok(nameBytes > names.join('').length, '含非 ASCII 名字时字节数必须大于字符数');
});

test('真实尺度的红/绿对照：本机实测的两份产物喂进判据，判定与实测一致', () => {
  // 干净包与脏包**只差 fileSize** —— 同一次剥符号改动、同一份条目内容（条目压缩和逐字节相同），
  // 一个删了旧 APK 重打，一个没删。这是这条判据要抓的形态在真实尺度上的样子；
  // 上面那些合成夹具的用例证明它有牙，这一条证明带宽在真实数字上也是对的（不会被悄悄放松）。
  const base = goodApk(() => {}); // goodApk 的默认形参是 {} 不是函数，须显式给一个空 patch
  const names = [
    ...base.names,
    ...Array.from({ length: 1001 - base.names.length }, (_, i) => `res/pad/${i}.xml`),
  ];
  const common = {
    names,
    readMember: base.readMember,
    repoLicense: base.repoLicense,
    repoSrsCount: SRS_COUNT,
    abi: DEFAULT_ABI,
    permissions: base.permissions,
  };
  const entriesCompressedTotal = 218812042; // 两份包相同

  const clean = apkViolations({ ...common, fileSize: 219011984, entriesCompressedTotal });
  assert.deepEqual(clean, [], `干净包（删旧包重打）应全绿，实为：${clean.join(' | ')}`);

  const dirty = apkViolations({ ...common, fileSize: 606871040, entriesCompressedTotal });
  assert.equal(dirty.length, 1, `脏包应恰 1 条违反，实为：${dirty.join(' | ')}`);
  assert.match(dirty[0], /死字节占整包 63\.\d%/);
  assert.match(dirty[0], /比值 2\.7734\d+ 超出带/);
});

// ───────────────────── 原生库剥符号判据（判据 ⑥）─────────────────────
//
// 这条判据存在的理由：把 `.cargo/config.toml` 的四段 android rustflags 删掉之后，
// 判据 ①②③④⑤ **全绿**（死字节那条也不红——文件大小与条目压缩和是一起变大的），
// 而 APK 从 219011984 弹回 606871040。反向对照仍是文件开头那条「未变异的合成 APK 全绿」。

test('M5 形态：libpolaris_lib.so 留着 DWARF ⇒ 红，点名节区、字节数与改动点', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({
        sections: [
          ...REQUIRED_ELF_SECTIONS,
          ['.debug_info', 165020847],
          ['.debug_str', 159873331],
        ],
      }),
    ),
  );
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /libpolaris_lib\.so 里还留着 2 个调试信息节/);
  assert.match(violations[0], /\.debug_info \.debug_str/);
  assert.match(violations[0], /324894178 字节/); // 两个节的 sh_size 之和，正面报出实测量
  // 报文要把人送到改动点，而不是让他自己去找
  assert.match(violations[0], /\.cargo\/config\.toml/);
  assert.match(violations[0], /strip=debuginfo/);
});

test('M5 变异是定点的：不牵连许可文本 / 符号计数 / .srs / 死字节四条', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: [...REQUIRED_ELF_SECTIONS, '.debug_info'] }),
    ),
  );
  assert.equal(hits(violations, '不在 APK 里'), 0, violations.join(' | '));
  assert.equal(hits(violations, '只出现'), 0, violations.join(' | '));
  assert.equal(hits(violations, '.srs'), 0, violations.join(' | '));
  assert.equal(hits(violations, '没有任何条目指向'), 0, violations.join(' | '));
});

test('libbox.so 也在射程内，且报文指向 build-libbox.sh 而不是 rustflags', () => {
  // 实测：三份真库（libbox 30 节 / libc++_shared 31 节 / libpolaris_lib 28 节）当下
  // 都是 0 个 debug 节，故把它们全纳入不会造出假红。而 libbox 由 Go/gomobile 构建、
  // 不经 rustc —— 出处不同，报文就必须指不同的地方，否则红了也修不到点上。
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ sections: [...REQUIRED_ELF_SECTIONS, '.debug_line'] })),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /libbox\.so 里还留着 1 个调试信息节/);
  assert.match(violations[0], /build-libbox\.sh/);
  assert.doesNotMatch(violations[0], /cargo\/config\.toml/);
});

test('压缩 DWARF 的旧拼写 `.zdebug_*` 同样算 ⇒ 红（读回形态≠写入形态）', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: [...REQUIRED_ELF_SECTIONS, ['.zdebug_info', 4096]] }),
    ),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /\.zdebug_info/);
  assert.ok(isDebugSection('.zdebug_info') && isDebugSection('.debug_info'));
  assert.ok(!isDebugSection('.dynsym') && !isDebugSection('.text'));
});

test('取材面：节表读不出来 ⇒ 红在取材面，绝不因「没扫到 .debug_*」判绿', () => {
  // e_shoff = 0：一份没有节表的 ELF。纯否定式判据在它上面是**恒绿**的。
  const violations = run((input) => {
    const so = fakeAppSo();
    so.writeBigUInt64LE(0n, 40);
    input._members.set(`lib/${ABI}/libpolaris_lib.so`, so);
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /节表读不出来/);
  assert.match(violations[0], /e_shoff = 0/);
});

test('取材面：节表越过文件末尾（读到的字节不完整）⇒ 红在取材面', () => {
  const violations = run((input) => {
    const full = fakeAppSo();
    input._members.set(`lib/${ABI}/libpolaris_lib.so`, full.subarray(0, full.length - 200));
  });
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /节表读不出来/);
  assert.match(violations[0], /超出文件长度/);
});

test('正面那一半有牙：节表解析得出但缺 .text/.dynsym ⇒ 红（不靠「什么都没解析到」判绿）', () => {
  const violations = run((input) =>
    input._members.set(
      `lib/${ABI}/libpolaris_lib.so`,
      fakeAppSo({ sections: ['.rodata', '.bss', '.data', '.comment', '.note'] }),
    ),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /没有 \.text \/ \.dynsym/);
  assert.match(violations[0], /不构成任何证据/);
});

test('取材面：lib/<ABI>/ 下一个 .so 都没扫到 ⇒ 红（空集上恒真不许判绿）', () => {
  const violations = run((input) => {
    for (const name of [...input._members.keys()]) {
      if (name.startsWith(`lib/${ABI}/`)) input._members.delete(name);
    }
  });
  assert.equal(hits(violations, '一个 .so 都没扫到'), 1, violations.join(' | '));
  assert.match(violations.find((v) => v.includes('一个 .so 都没扫到')), /恒真，不许判绿/);
});

test('ELF32 也要解析对（armv7 / i686 两个 ABI 是 32 位）', () => {
  const stripped = fakeElf({ cls: 32, sections: [...REQUIRED_ELF_SECTIONS, '.rodata'] });
  const ok = strippedReport(stripped);
  assert.equal(ok.why, null, `ELF32 应解析成功，实为：${ok.why}`);
  assert.equal(ok.ok, true);
  assert.equal(ok.sections, 5); // NULL + .text + .dynsym + .rodata + .shstrtab

  const dirty = fakeElf({ cls: 32, sections: [...REQUIRED_ELF_SECTIONS, ['.debug_info', 999]] });
  const bad = strippedReport(dirty);
  assert.equal(bad.ok, false);
  assert.deepEqual(bad.debug, ['.debug_info']);
  assert.equal(bad.debugBytes, 999);
  // 反向对照：把同一份 ELF32 当 ELF64 读会读到垃圾——这正是「写死一种 class」的后果。
  const mislabeled = Buffer.from(dirty);
  mislabeled[4] = 2;
  assert.notEqual(strippedReport(mislabeled).why, null, '按错的 class 解析必须失败，而不是静默给个干净结论');
});

test('elfSectionNames 报出的是**实际读到的**节名与字节数（正面断言的取值口）', () => {
  const buf = fakeElf({ sections: [['.text', 100], ['.dynsym', 200], ['.debug_info', 300]] });
  const parsed = elfSectionNames(buf);
  assert.equal(parsed.ok, true);
  assert.deepEqual(parsed.names, ['', '.text', '.dynsym', '.debug_info', '.shstrtab']);
  assert.equal(parsed.bytes[1], 100);
  assert.equal(parsed.bytes[2], 200);
  assert.equal(parsed.bytes[3], 300);
  assert.equal(strippedReport(buf).debugBytes, 300);
});

// ───────────────────── ABI 参数化（2026-09-05）─────────────────────
//
// 起因：`ABI` 曾是写死的 `arm64-v8a`，而收尾序列要出 arm64 与 x86_64 两个 debug 包，
// **x86_64 恰恰是模拟器上跑的那个**——它此前在本仓一条产物级判据都没有。
// 下面这组用例分两半：一半钉参数取值口（缺省不变 / 未知取值必须抛），
// 一半钉「参数真的进了判据」（换 ABI 造的包按另一个 ABI 判必须红，不许静默判绿）。

test('parseArgs：不带 --abi 时缺省仍是 arm64-v8a（既有调用逐字不变）', () => {
  assert.deepEqual(parseArgs(['out/app-arm64-debug.apk']), {
    apk: 'out/app-arm64-debug.apk',
    abi: 'arm64-v8a',
  });
  assert.equal(DEFAULT_ABI, 'arm64-v8a');
});

test('parseArgs：--abi 取得到，且与路径的先后顺序无关', () => {
  assert.deepEqual(parseArgs(['a.apk', '--abi', 'x86_64']), { apk: 'a.apk', abi: 'x86_64' });
  assert.deepEqual(parseArgs(['--abi', 'x86_64', 'a.apk']), { apk: 'a.apk', abi: 'x86_64' });
});

test('parseArgs：未知 ABI ⇒ 抛，且报文列出全部支持的取值（不静默按缺省走）', () => {
  // 静默回落的后果不是「少验一点」：`--abi x86-64`（该写下划线）会让判据去量 arm64 那半个包，
  // 而它在 x86_64 产物里根本不存在 —— 要么红在「取材面塌了」（离真因两跳远），要么恰好判绿。
  assert.throws(() => parseArgs(['a.apk', '--abi', 'x86-64']), (e) => {
    assert.match(e.message, /未知 ABI："x86-64"/);
    for (const known of Object.keys(ABIS)) assert.ok(e.message.includes(known), `报文里应列出 ${known}`);
    return true;
  });
  assert.throws(() => parseArgs(['a.apk', '--abi']), /--abi 后面要跟一个 ABI 名/);
  // 反向对照：四个已知取值一个都不许被这条拦下来。
  for (const known of Object.keys(ABIS)) {
    assert.equal(parseArgs(['a.apk', '--abi', known]).abi, known);
  }
});

test('反向对照：四个 ABI 各造一个包、各按自己判 ⇒ 全绿', () => {
  for (const abi of Object.keys(ABIS)) {
    assert.deepEqual(run(() => {}, { abi }), [], `${abi} 的合成包应全绿`);
  }
});

test('ABI 真的进了判据：x86_64 的包按缺省 arm64 判 ⇒ 红（不许静默判绿）', () => {
  // 这条是「参数化只改了签名、判据还挂在旧常量上」的变异。若 abi 是装饰品，
  // 下面这次判定会去看 lib/arm64-v8a/（包里没有）却依然判绿。
  const violations = run(() => {}, { abi: 'x86_64', judgeAs: 'arm64-v8a' });
  assert.equal(hits(violations, 'lib/arm64-v8a/libbox.so 不在 APK 里'), 1, violations.join(' | '));
  assert.equal(hits(violations, 'lib/arm64-v8a/libpolaris_lib.so 不在 APK 里'), 1, violations.join(' | '));
  assert.equal(hits(violations, '一个 .so 都没扫到'), 1, violations.join(' | '));
  assert.equal(violations.length, 3, violations.join(' | '));
});

test('ELF 机器类型对不上 ABI ⇒ 红（路径前缀不校验内容，只有这条看得见）', () => {
  const violations = run((input) =>
    input._members.set(`lib/${ABI}/libbox.so`, fakeSo({ machine: ABIS.x86_64.machine })),
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /e_machine=0x3e/);
  assert.match(violations[0], /应为 .*e_machine=0xb7（EM_AARCH64）/);
});

test('ELF 位宽对不上 ABI ⇒ 红（armeabi-v7a 是 ELF32，塞进一份 ELF64）', () => {
  const abi = 'armeabi-v7a';
  const violations = run(
    (input) =>
      input._members.set(
        `lib/${abi}/libpolaris_lib.so`,
        fakeAppSo({ cls: 64, machine: ABIS[abi].machine }),
      ),
    { abi },
  );
  assert.equal(violations.length, 1, violations.join(' | '));
  assert.match(violations[0], /读到 EI_CLASS=2（ELF64）/);
  assert.match(violations[0], /应为 EI_CLASS=1（ELF32）/);
});

test('换了 ABI 之后，构建 tag 与剥符号两条判据照样有牙', () => {
  const abi = 'x86_64';
  const { machine } = ABIS[abi];

  const noNaive = run(
    (input) => input._members.set(`lib/${abi}/libbox.so`, fakeSo({ machine, naive: 0 })),
    { abi },
  );
  assert.equal(noNaive.length, 1, noNaive.join(' | '));
  assert.match(noNaive[0], new RegExp(`lib/${abi}/libbox\\.so 里 'naive' 只出现 0 次`));

  const withDwarf = run(
    (input) =>
      input._members.set(
        `lib/${abi}/libpolaris_lib.so`,
        fakeAppSo({ machine, sections: [...REQUIRED_ELF_SECTIONS, ['.debug_info', 4096]] }),
      ),
    { abi },
  );
  assert.equal(withDwarf.length, 1, withDwarf.join(' | '));
  assert.match(withDwarf[0], /还留着 1 个调试信息节/);
  assert.match(withDwarf[0], /\.cargo\/config\.toml/);
});

// ───────────────────── 出厂权限集判据（判据 ⑦）─────────────────────
//
// 这条判据存在的理由（2026-09-06 实证）：源码侧那道权限门（`src-tauri/tests/
// android_native_surface_wiring.rs`）读的是**本仓自有**的 AndroidManifest.xml，而 APK 带的是
// manifest merger 合并库 manifest 之后的集合。本仓 8 条、合并后 11 条，两者今天就已经不等。
// 于是「升一版 tauri 插件 ⇒ APK 多一条敏感权限」这一形，在本判据之前没有任何一处会说话。
//
// 下面这段是**本机真 aapt2 的真实输出**（build-tools 36.0.0，喂的是由本仓 packaged manifest
// 现场 link 出来的 APK），逐字记下来当夹具。解析器的形状因此不是从文档抄的：
// `permission:`（自己声明的）与 `uses-permission:`（请求的）只差一个词，行首锚定是承重的。
const AAPT2_DUMP = [
  'package: com.polaris2.app',
  'permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION',
  "uses-permission: name='android.permission.INTERNET'",
  "uses-permission: name='android.permission.FOREGROUND_SERVICE'",
  "uses-permission: name='android.permission.FOREGROUND_SERVICE_SYSTEM_EXEMPTED'",
  "uses-permission: name='android.permission.POST_NOTIFICATIONS'",
  "uses-permission: name='android.permission.ACCESS_NETWORK_STATE'",
  "uses-permission: name='android.permission.CHANGE_NETWORK_STATE'",
  "uses-permission: name='android.permission.ACCESS_WIFI_STATE'",
  "uses-permission: name='android.permission.REQUEST_INSTALL_PACKAGES'",
  "uses-permission: name='android.permission.RECEIVE_BOOT_COMPLETED'",
  "uses-permission: name='android.permission.WAKE_LOCK'",
  "uses-permission: name='com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'",
  '',
].join('\n');

test('反向对照：真 aapt2 输出解析出 11 条请求 + 1 条声明，且判据全绿', () => {
  const parsed = parseAapt2Permissions(AAPT2_DUMP);
  assert.equal(parsed.packageName, 'com.polaris2.app');
  assert.equal(parsed.uses.length, 11);
  assert.equal(parsed.declared.length, 1);
  assert.deepEqual(permissionViolations(parsed), []);
});

test('Debug 包只重基 applicationId 权限，Release 身份与其余权限仍严格验收', () => {
  const debug = parseAapt2Permissions(AAPT2_DUMP.replaceAll('com.polaris2.app', 'com.polaris2.app.debug'));
  assert.equal(debug.packageName, 'com.polaris2.app.debug');
  assert.deepEqual(permissionViolations(debug), []);
  const wrongPermission = parseAapt2Permissions(
    AAPT2_DUMP.replace('package: com.polaris2.app', 'package: com.polaris2.app.debug'),
  );
  assert.ok(permissionViolations(wrongPermission).some((v) => v.includes('DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION')));
  const unexpectedId = parseAapt2Permissions(
    AAPT2_DUMP.replace('package: com.polaris2.app', 'package: com.polaris2.app.other'),
  );
  assert.match(permissionViolations(unexpectedId)[0], /applicationId/);
});

test('解析器按行首锚定：`permission:` 那一行不许被算成一次请求', () => {
  const parsed = parseAapt2Permissions(AAPT2_DUMP);
  // 同一个名字既被声明也被请求（androidx 那条就是这样）——两个集合各算各的，不许串。
  assert.ok(parsed.declared.includes('com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'));
  assert.equal(
    parsed.uses.filter((n) => n === 'com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION').length,
    1,
    '请求集里那条应恰好来自 uses-permission 行，声明行不许再算一次',
  );
});

test('M11 形态：依赖往合并 manifest 注入 QUERY_ALL_PACKAGES ⇒ 红，且点名那一条', () => {
  const parsed = parseAapt2Permissions(
    AAPT2_DUMP.replace(
      "uses-permission: name='android.permission.INTERNET'",
      "uses-permission: name='android.permission.INTERNET'\nuses-permission: name='android.permission.QUERY_ALL_PACKAGES'",
    ),
  );
  const violations = permissionViolations(parsed);
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /没登记/);
  assert.match(violations[0], /QUERY_ALL_PACKAGES/);
});

test('M12 形态：某条腿的权限被误删 ⇒ 红，且点名那一条', () => {
  const parsed = parseAapt2Permissions(
    AAPT2_DUMP.replace("uses-permission: name='android.permission.REQUEST_INSTALL_PACKAGES'\n", ''),
  );
  const violations = permissionViolations(parsed);
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /不存在/);
  assert.match(violations[0], /REQUEST_INSTALL_PACKAGES/);
});

test('M13 形态：库悄悄多声明一条自建 permission ⇒ 红（声明面与请求面各判各的）', () => {
  const parsed = parseAapt2Permissions(
    AAPT2_DUMP.replace(
      'permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION',
      'permission: com.polaris2.app.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION\npermission: com.polaris2.app.SOMETHING_NEW',
    ),
  );
  const violations = permissionViolations(parsed);
  assert.equal(violations.length, 1, `应恰 1 条违反，实为：${violations.join(' | ')}`);
  assert.match(violations[0], /自建 permission/);
  assert.match(violations[0], /SOMETHING_NEW/);
});

test('取材面塌陷：aapt2 缺席 / 输出读不出 ⇒ 红在取材面，不判绿也不报成「少了 11 条」', () => {
  // `null` = 工具找不到或调用失败。
  const missing = permissionViolations(null);
  assert.equal(missing.length, 1);
  assert.match(missing[0], /读不出出厂 APK 的权限集/);
  // 没有 `package:` 行 ⇒ 解析器自己就返回 null（而不是一个空集）。
  assert.equal(parseAapt2Permissions('Error: failed to open apk\n'), null);
  assert.equal(parseAapt2Permissions(''), null);
});

test('正面断言有牙：解析器只读到零星几条 ⇒ 红在下限，不去逐条报差集', () => {
  const violations = permissionViolations({ uses: ['android.permission.INTERNET'], declared: [] });
  assert.equal(violations.length, 1, `应恰 1 条（下限那条），实为：${violations.join(' | ')}`);
  assert.match(violations[0], /解析口径塌了/);
});

test('登记表自身：本仓自有恰 9 条、库注入恰 2 条，且每条都写了 from 与 why', () => {
  // 2026-09-25：RECEIVE_BOOT_COMPLETED 由「库注入」改为本仓自有（开机自动连接真的消费它），8+3 → 9+2。
  // 「今天 9 + 2」这件事本身要被钉住：把一条自有权限改标成「库注入」来消一个红，
  // 或反过来把库注入的那三条抹掉当自有的，都会在这里红。
  const self = APK_PERMISSION_REGISTRY.filter((e) => e.from === 'self');
  const injected = APK_PERMISSION_REGISTRY.filter((e) => e.from !== 'self');
  assert.equal(self.length, 9, '本仓自有 uses-permission 的条数变了 —— 权限画像变了，来这里答一次题');
  assert.equal(injected.length, 2, '库注入的条数变了 —— 依赖带进来的权限变了，逐条登记来源与理由');
  for (const entry of [...APK_PERMISSION_REGISTRY, ...APK_DECLARED_PERMISSIONS]) {
    assert.ok(entry.from && entry.from.length > 0, `${entry.name} 没写 from`);
    assert.ok(
      entry.why && [...entry.why].length >= 20,
      `${entry.name} 的理由太短：${entry.why} —— 要说清「缺了它哪条腿会怎样失败」`,
    );
    assert.ok(!entry.why.includes(entry.name), `${entry.name} 的理由只是把权限名重复了一遍`);
  }
  // 名字不许重复：重复会让「集合恰等」在一条被删掉时仍然绿。
  const names = APK_PERMISSION_REGISTRY.map((e) => e.name);
  assert.equal(new Set(names).size, names.length, '登记表里有重复条目');
});


test('16 KB alignment checks every LOAD, including dependency libraries', () => {
  const so = fakeElf();
  assert.equal(elfPageAlignment(so).ok, true);
  so.writeBigUInt64LE(4096n, 64 + 56 + 48);
  assert.equal(elfPageAlignment(so).ok, false);
  const result = run(input => input._members.set(`lib/${ABI}/libextra.so`, so));
  assert.ok(result.some(message => message.includes('libextra.so') && message.includes('16 KB')));
  assert.equal(elfPageAlignment(so.subarray(0, 70)).ok, false);
});
