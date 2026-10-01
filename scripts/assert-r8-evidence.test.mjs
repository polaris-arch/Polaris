// assert-r8-evidence.test.mjs —— 把 release 冒烟腿的产物级判据钉住。
//
// 这些用例的承重面是**回放 2026-09-05 验收实测出来的三条真缺陷**：
//   · B07：`io.nekohasekai` 那根针命中的是本仓 proguard-rules.pro 注释里的同一句话，
//          于是 libbox 的 consumer 规则一条没进 R8 时判据照样绿；
//   · B05/M04：keep 规则指向一个不存在的类时，`configuration.txt` 里那行文本照样在，
//          而 `CertificateVerifier` 已被 R8 整包剪掉 —— 判据必须问 `seeds.txt`；
//   · A11：第 5 根针钉的是类名 `com.polaris2.app.RustWebView`，而 Tauri 生成的
//          proguard-wry.pro 里本来就有一条同名 keep ⇒ 本批那个 keep 块整块删掉针照样命中。
// 新判据写完先回放老缺陷：不红就是没写完。

import test from "node:test";
import assert from "node:assert/strict";

import {
  CONFIGURATION_NEEDLES,
  EXTERNAL_SOURCE,
  SEED_ABSENT_CONTROL,
  assertR8Evidence,
  commentOnlyLines,
  parseConfigurationSections,
  sourceMatches,
  stripHashComments,
} from "./assert-r8-evidence.mjs";

/** 一份「什么都对」的 configuration.txt：五根针各以**规则行**形态出现。 */
function goodConfiguration() {
  return [
    "# The proguard configuration file for the following section is app/proguard-rules.pro",
    "# libbox.aar 的 proguard.txt（go.** / io.nekohasekai.**）、androidx.webkit 1.14.0 的 proguard.txt",
    "-keepattributes SourceFile,LineNumberTable",
    "-keep class org.rustls.platformverifier.** { *; }",
    "-keep class com.polaris2.app.PolarisTls { *; }",
    "-keep class com.polaris2.app.RustWebView {",
    "  void clearAllBrowsingData();",
    "  java.lang.String getCookies(java.lang.String);",
    "}",
    "-dontwarn java.beans.ConstructorProperties",
    "-dontwarn java.beans.Transient",
    "# End of content from app/proguard-rules.pro",
    "# The proguard configuration file for the following section is libbox.aar!proguard.txt",
    "-keep class io.nekohasekai.** { *; }",
    "-keep class go.** { *; }",
    "# The proguard configuration file for the following section is tauri-android consumer rules",
    "-keep @app.tauri.annotation.TauriPlugin public class * {",
    "  @app.tauri.annotation.Command public <methods>;",
    "}",
  ].join("\n");
}

/** 一份「什么都保住了」的 seeds.txt（行数要过下限自检）。 */
function goodSeeds() {
  const filler = Array.from({ length: 20 }, (_, i) => `androidx.filler.Class${i}`);
  return [
    "org.rustls.platformverifier.CertificateVerifier",
    "org.rustls.platformverifier.CertificateVerifier: void <init>()",
    "org.rustls.platformverifier.VerificationResult",
    "com.polaris2.app.PolarisTls",
    "com.polaris2.app.PolarisTls: void initPlatformVerifier(android.content.Context)",
    "com.polaris2.app.RustWebView: void clearAllBrowsingData()",
    "com.polaris2.app.RustWebView: java.lang.String getCookies(java.lang.String)",
    ...filler,
  ].join("\n");
}

/** 本仓规则文件的注释形态（真的会被 R8 抄进 configuration.txt 的那种）。 */
const OWN_RULES = [
  "# libbox.aar 的 proguard.txt（go.** / io.nekohasekai.**）、androidx.webkit 1.14.0 的 proguard.txt",
  "-keep class org.rustls.platformverifier.** { *; }",
].join("\n");

function run(overrides = {}) {
  return assertR8Evidence({
    configuration: goodConfiguration(),
    seeds: goodSeeds(),
    ownRules: OWN_RULES,
    ...overrides,
  });
}

test("完好的一对产物：零 problem", () => {
  const { problems } = run();
  assert.deepEqual(problems, [], problems.join("\n"));
});

test("B07 回放：io.nekohasekai 只出现在本仓注释里 ⇒ 必须红", () => {
  // libbox 的 consumer 规则整段没进 R8，只剩本仓 proguard-rules.pro 那行注释抄了过来。
  const polluted = goodConfiguration()
    .split("\n")
    .filter((line) => !line.startsWith("-keep class io.nekohasekai"))
    .concat(["# 反向对照：libbox.aar 的 proguard.txt 只有 go.** / io.nekohasekai.**"])
    .join("\n");
  const { problems } = run({ configuration: polluted });
  assert.equal(problems.length, 1, problems.join("\n"));
  // 断言的是「剥注释后的判定面里没有它」这一条，不是「随便哪条 problem 里提到了它」——
  // 后者会被「剥注释没生效」那条自检消息喂绿（它的引文里也带着这串）。
  assert.match(problems[0], /（剥注释后）中没有 `io\.nekohasekai`/);
});

test("B07 的反面：不剥注释的话同一份输入是绿的（证明这条用例有牙）", () => {
  const polluted = goodConfiguration()
    .split("\n")
    .filter((line) => !line.startsWith("-keep class io.nekohasekai"))
    .concat(["# 反向对照：libbox.aar 的 proguard.txt 只有 go.** / io.nekohasekai.**"])
    .join("\n");
  assert.ok(polluted.includes("io.nekohasekai"), "原文里确实还有这串");
  assert.ok(
    !stripHashComments(polluted).includes("io.nekohasekai"),
    "剥掉注释之后就没有了 —— 两个面的差别正是 B07",
  );
});

test("B05 / M04 回放：keep 指向不存在的类 ⇒ configuration 照样有，seeds 说话", () => {
  const bogus = goodConfiguration().replace(
    "-keep class org.rustls.platformverifier.** { *; }",
    "-keep class org.rustls.platformverifier.NoSuchThing { *; }",
  );
  const seeds = goodSeeds()
    .split("\n")
    .filter((line) => !line.startsWith("org.rustls.platformverifier"))
    .join("\n");
  // configuration 这一侧仍然全绿 —— 这正是「文本到过 R8」证明不了的事。
  const configOnly = run({ configuration: bogus });
  assert.ok(
    !configOnly.problems.some((p) => p.includes("NoSuchThing")),
    "文本判据对这条变异是瞎的（这就是本判据要补的洞）",
  );
  const { problems } = run({ configuration: bogus, seeds });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /CertificateVerifier/);
});

test("wry 的两个成员一个没保住也要红", () => {
  const seeds = goodSeeds()
    .split("\n")
    .filter((line) => !line.includes("clearAllBrowsingData"))
    .join("\n");
  const { problems } = run({ seeds });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /clearAllBrowsingData/);
});

test("反向对照有牙：seeds 里出现不存在的类 ⇒ 判定为匹配器恒真", () => {
  const seeds = `${goodSeeds()}\n${SEED_ABSENT_CONTROL}`;
  const { problems } = run({ seeds });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /恒真/);
});

test("剥注释确实发生了：本仓注释原文被抄进来时也不该有 problem，且留下一条记录", () => {
  const { problems, notes } = run();
  assert.deepEqual(problems, []);
  assert.ok(
    notes.some((n) => n.includes("抄进了 configuration.txt")),
    `期望有一条「R8 抄了注释」的记录，实得：${notes.join(" | ")}`,
  );
});

test("空产物必须红，而不是在空面上恒真", () => {
  assert.match(run({ configuration: "" }).problems.join("\n"), /configuration\.txt 是空的/);
  assert.match(run({ seeds: "" }).problems.join("\n"), /seeds\.txt 是空的/);
});

test("configuration 全是注释 ⇒ 判定面塌了，当场红", () => {
  const { problems } = run({ configuration: "# 全是注释\n# 一条规则都没有\n" });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /取材面塌了/);
});

test("seeds 行数过少 ⇒ 当场红，不在这种面上下命中结论", () => {
  const { problems } = run({ seeds: "org.rustls.platformverifier.CertificateVerifier\n" });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /只有 1 行/);
});

test("五根 configuration 针每一根都真的有牙（逐条删除对照）", () => {
  for (const [needle] of CONFIGURATION_NEEDLES) {
    const stripped = goodConfiguration()
      .split("\n")
      .filter((line) => !line.includes(needle))
      .join("\n");
    const { problems } = run({ configuration: stripped });
    assert.ok(
      problems.some((p) => p.includes(needle)),
      `删掉 \`${needle}\` 之后判据没红 —— 这根针没有牙`,
    );
  }
});

test("commentOnlyLines 只取注释行，且短注释不入对照面", () => {
  const long = "这是一行足够长的注释原文，长到能被当成正向对照使用，不会被最小长度过滤掉";
  const lines = commentOnlyLines(`# 短\n-keep class A\n# ${long}\n`);
  assert.deepEqual(lines, [long]);
});


// ══ A11：针必须分得清规则来自**哪一份**规则文件 ══════════════════════════════════

/** 把本批那个 keep 块从本仓段里摘掉，改成由 Tauri 生成的 proguard-wry.pro 段提供同名类。 */
function wryFedConfiguration() {
  return goodConfiguration()
    .split("\n")
    .filter(
      (line) =>
        !line.includes("-keep class com.polaris2.app.RustWebView {") &&
        !line.includes("void clearAllBrowsingData();") &&
        !line.includes("java.lang.String getCookies(java.lang.String);"),
    )
    .concat([
      "# The proguard configuration file for the following section is " +
        "app/src/main/java/com/polaris2/app/generated/proguard-wry.pro",
      "-keep class com.polaris2.app.RustWebView {",
      "  public <init>(...);",
      "  void loadUrlMainThread(...);",
      "  void evalScript(...);",
      "}",
      "# End of content from app/src/main/java/com/polaris2/app/generated/proguard-wry.pro",
    ])
    .join("\n");
}

test("A11 回放：本批的 keep 块没了、只剩 proguard-wry.pro 里的同名类 ⇒ 必须红", () => {
  const { problems } = run({ configuration: wryFedConfiguration() });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /clearAllBrowsingData/);
  assert.match(problems[0], /proguard-rules\.pro/);
});

test("A11 的反面：只按类名、不按来源判的话同一份输入是绿的（证明这条用例有牙）", () => {
  const polluted = wryFedConfiguration();
  assert.ok(
    stripHashComments(polluted).includes("com.polaris2.app.RustWebView"),
    "上一版那根针（类名）在这份输入上照样命中 —— 这正是 A11",
  );
  assert.ok(
    !stripHashComments(polluted).includes("clearAllBrowsingData"),
    "本批那个块确实已经不在了",
  );
});

test("外部来源的针被抄进本仓自己那一段 ⇒ 不算数，必须红", () => {
  // libbox 的 consumer 规则一条没进 R8，但有人把同一条抄进了 proguard-rules.pro。
  const moved = goodConfiguration()
    .split("\n")
    .filter((line) => !line.startsWith("-keep class io.nekohasekai"))
    .map((line) =>
      line === "-dontwarn java.beans.Transient"
        ? "-dontwarn java.beans.Transient\n-keep class io.nekohasekai.** { *; }"
        : line,
    )
    .join("\n");
  assert.ok(stripHashComments(moved).includes("io.nekohasekai"), "全文里确实还有这串");
  const { problems } = run({ configuration: moved });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /io\.nekohasekai/);
  assert.match(problems[0], /别的段里/);
});

test("configuration.txt 没有段落标记 ⇒ 归不了因，当场红（不退化成全文匹配）", () => {
  const flat = goodConfiguration()
    .split("\n")
    .filter((line) => !line.trim().startsWith("#"))
    .join("\n");
  const { problems } = run({ configuration: flat });
  assert.equal(problems.length, 1, problems.join("\n"));
  assert.match(problems[0], /一个段落标记都没有/);
});

test("段落切分自检：来源取得到，End of content 之后的内容不归任何来源", () => {
  const sections = parseConfigurationSections(
    [
      "# The proguard configuration file for the following section is a/one.pro",
      "-keep class A",
      "# End of content from a/one.pro",
      "-keep class Orphan",
      "# The proguard configuration file for the following section is b/two.pro",
      "# 注释里也写着 -keep class A",
      "-keep class B",
    ].join("\n"),
  );
  assert.deepEqual(
    sections.map((section) => section.source),
    ["a/one.pro", null, "b/two.pro"],
  );
  assert.ok(sections[0].body.includes("-keep class A"));
  assert.ok(sections[1].body.includes("-keep class Orphan"));
  // 段内注释同样被剥掉：注释里那句 `-keep class A` 不许给 b/two.pro 那一段作证。
  assert.ok(!sections[2].body.includes("-keep class A"));
  assert.ok(sections[2].body.includes("-keep class B"));
  // 来源判定：本仓自己那三份不算「外部来源」。
  assert.ok(sourceMatches("app/proguard-rules.pro", "proguard-rules.pro"));
  assert.ok(!sourceMatches("app/proguard-rules.pro", EXTERNAL_SOURCE));
  assert.ok(sourceMatches("libbox.aar!proguard.txt", EXTERNAL_SOURCE));
  assert.ok(!sourceMatches(null, EXTERNAL_SOURCE));
});
