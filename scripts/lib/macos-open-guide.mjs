// One reviewed guide feeds README translations and the complete mounted DMG.
// These anchors preserve provenance, conditional OS approval, and stop guidance.
const contracts = [
  ['简体中文', 'README.md', ['来源或完整性无法确认', '若仅因开发者无法验证', '不要清除隔离属性或关闭系统']],
  ['English', 'README.en.md', ['Stop if provenance or integrity is uncertain', 'If only developer verification blocks it', 'Do not clear quarantine attributes or disable system security protections']],
  ['繁體中文', 'README.zh-TW.md', ['來源或完整性無法確認', '若僅因無法驗證開發者', '不要清除隔離屬性或關閉系統']],
  ['Русский', 'README.ru.md', ['Если происхождение или целостность не подтверждены', 'Если блокировка связана только с проверкой разработчика', 'Не снимайте атрибуты карантина и не отключайте защиту системы']],
  ['فارسی', 'README.fa.md', ['اگر منشأ یا یکپارچگی فایل مشخص نیست', 'اگر تنها مانع، تأیید توسعه‌دهنده است', 'ویژگی‌های قرنطینه را پاک نکنید']],
];
const normalize = (text) => text.replace(/\s+/gu, ' ').trim();
export function macOpenGuideViolations(guide, readmes) {
  const errors = [];
  const sections = [...guide.matchAll(/═+\n  ([^\n]+)\n═+\n\n([\s\S]*?)(?=\n\n═+|$)/gu)];
  if (sections.length !== contracts.length) errors.push('macOS guide must contain all five complete translations');
  for (const [language, file, anchors] of contracts) {
    const matching = sections.filter((section) => section[1].startsWith(`${language} — `));
    if (matching.length !== 1) { errors.push(`macOS guide missing or duplicate ${language} section`); continue; }
    const body = normalize(matching[0][2]);
    for (const anchor of anchors) if (!body.includes(anchor)) errors.push(`${language} guide lost safety instruction: ${anchor}`);
    if (!normalize(readmes[file] ?? '').includes(body)) errors.push(`${file} differs from the reviewed ${language} guide`);
  }
  for (const [file, text] of [['guide', guide], ...Object.entries(readmes)]) {
    if (/\bxattr\s+-|\bspctl\s+--(?:master-disable|global-disable)/u.test(text)) errors.push(`${file} contains a security-bypass command`);
  }
  return errors;
}
