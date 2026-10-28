# Polaris

<div align="center">

[简体中文](README.md) · [English](README.en.md) · [繁體中文](README.zh-TW.md) · [Русский](README.ru.md) · **فارسی**

[![release](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fapi.github.com%2Frepos%2Fpolaris-arch%2FPolaris%2Freleases%2Flatest&query=%24.tag_name&style=flat-square&color=0E98A4&label=release)](https://github.com/polaris-arch/Polaris/releases/latest)
[![sing-box](https://img.shields.io/badge/sing--box-1.15-0E98A4?style=flat-square)](https://github.com/polaris-arch/sing-box-polaris)
[![platform](https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux%20%C2%B7%20Android-0E98A4?style=flat-square)](#نصب)
[![license](https://img.shields.io/badge/license-MIT-0E98A4?style=flat-square)](LICENSE)
[![stars](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fapi.github.com%2Frepos%2Fpolaris-arch%2FPolaris&query=%24.stargazers_count&style=flat-square&color=0E98A4&label=stars)](https://github.com/polaris-arch/Polaris/stargazers)

</div>

<div dir="rtl">

**Polaris** — یک کلاینت پروکسی شبکه چندسکویی بر پایه [sing-box-polaris](https://github.com/polaris-arch/sing-box-polaris). ساخته‌شده با Tauri 2 (ترکیب Rust و React).

![خانه](docs/screenshots/home.png)

## قابلیت‌ها

| حوزه | قابلیت‌ها |
|---|---|
| شیوه گرفتن ترافیک | TUN · پروکسی سیستمی / پورت محلی (دسکتاپ) |
| مسیریابی | هوشمند / سراسری / مستقیم · قوانین سفارشی · مسیریابی بر پایه برنامه · مسیریابی منطقه‌ای (حالت هوشمند، شامل بازگشت به چین) |
| گره‌های پروکسی | VLESS · VMess · Trojan · Hysteria 2 / 1 · TUIC · Shadowsocks · AnyTLS · Naive · Snell · SOCKS · HTTP · SSH · Tor[1](#tor-note) |
| اتصال‌های شبکه[2](#mesh-note) | WireGuard · Tailscale · WARP (WireGuard) · OpenConnect · OpenVPN · MASQUE[3](#masque-note) |
| DNS | FakeIP · DoH / DoT · رقابت حل‌کننده‌های دامنهٔ گره‌ها |
| کنترل شبکه | کلید IPv6 · مسدودسازی DoH مرورگر · سیاست WebRTC (فقط TUN) |
| عیب‌یابی | توپولوژی اتصال‌ها · گزارش زنده · سنجش سرعت گره‌ها همراه با پیشرفت · نمایش قوانین تطبیق‌یافته · تشخیص باز بودن سرویس‌های پخش و هوش مصنوعی |
| بهره‌برداری | مدیریت اشتراک · پشتیبان‌گیری و بازیابی پیکربندی با تأیید · قفل حریم خصوصی · ماندگاری در سینی سیستمِ دسکتاپ |
| به‌روزرسانی برنامه | کانال پایدار / آزمایشی · دانلود دوبارهٔ نسخهٔ فعلی · بررسی چکیدهٔ نصب‌کننده و هشدار نبود آن |
| بهینه‌سازی حافظهٔ دسکتاپ | با فعال بودن حالت سبک خودکار، WebView اصلی پس از ۱۰ دقیقه پنهان یا کمینه ماندن پیوستهٔ پنجرهٔ اصلی آزاد می‌شود؛ آمار، اتصال‌ها و گزارش‌ها تنها هنگام نیاز مشترک می‌شوند |

</div>

<a id="tor-note"></a>

1. Tor به فایل اجرایی در دسترس Tor و شرایط لازم برای اجرا روی سکو نیاز دارد.

<a id="mesh-note"></a>

2. OpenConnect، ‏OpenVPN و MASQUE از بخش شبکه خصوصی افزوده می‌شوند. مسیریابی شبکهٔ داخلی به زیرشبکه‌های داخلی پیکربندی‌شده (meshRoutes) نیز وابسته است؛ صرف دسته‌بندی ورودی این قابلیت را اثبات نمی‌کند.

<a id="masque-note"></a>

3. MASQUE از پیاده‌سازی خود sing-box استفاده می‌کند و از اتصال رسمی MASQUE کلادفلر، از جمله WARP از طریق MASQUE، پشتیبانی نمی‌کند. این محدودیت فقط مربوط به همین روش اتصال است.

<table>
<tr>
<td width="50%"><img src="docs/screenshots/nodes.png" alt="گره‌ها"><br><sub>مدیریت گره‌ها و سنجش سرعت</sub></td>
<td width="50%"><img src="docs/screenshots/rules.png" alt="قوانین"><br><sub>قوانین مسیریابی سفارشی</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/connections.png" alt="اتصال‌ها"><br><sub>اتصال‌های زنده</sub></td>
<td><img src="docs/screenshots/settings.png" alt="تنظیمات"><br><sub>تنظیمات</sub></td>
</tr>
</table>

<div dir="rtl">

## نصب

پس از انتشار رسمی، بستهٔ سکوی خود را از [Releases](https://github.com/polaris-arch/Polaris/releases) دریافت کنید. موفقیت بررسی کد یا CI به معنی موجود بودن بستهٔ عمومی برای دانلود نیست.

فرایند انتشار فعلی از نام فایل‌های تعریف‌شده در [release-assets.mjs](scripts/release-assets.mjs) استفاده می‌کند؛ `<version>` نسخه برنامه بدون پیشوند `v` است. برای macOS بسته‌های جداگانه Apple Silicon / Intel، برای Windows بسته x64 و برای Linux بسته amd64 در نظر گرفته شده است. بسته‌های Android به‌ترتیب ARM64، ‏ARMv7 و universal آمده‌اند؛ universal فقط همین دو معماری ARM را شامل می‌شود و x86 ندارد. امکان دانلود به فایل‌های واقعی انتشار انتخاب‌شده بستگی دارد. فایل‌ها را با `SHA256SUMS` در همان صفحه بررسی کنید.

| سکو | فایل |
|---|---|
| macOS | `Polaris_<version>_aarch64-mac.dmg` / `Polaris_<version>_x64-mac.dmg` |
| Windows | `Polaris_<version>_x64-win-setup.exe` / `Polaris_<version>_x64-win-Portable.zip` |
| Linux | `Polaris_<version>_amd64-linux.deb` / `Polaris_<version>_amd64-linux.AppImage` |
| Android | `Polaris_<version>_arm64-v8a-android.apk` / `Polaris_<version>_armeabi-v7a-android.apk` / `Polaris_<version>_universal-android.apk` |

بسته‌های دسکتاپ با گواهی پولی امضای کد امضا نمی‌شوند؛ اجرای نخست ممکن است به تأیید دستی نیاز داشته باشد.

نصب‌کننده ویندوز، WebView2 Runtime را در خود جای نمی‌دهد؛ اگر سیستم فاقد آن باشد، به‌صورت برخط دریافت می‌شود. کاربران نسخه‌های سبک‌شده / LTSC یا نسخه قابل حمل که Runtime را ندارند، ابتدا [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) را از سایت مایکروسافت نصب کنند. Polaris نصب‌کننده آفلاین WebView2 ارائه نمی‌دهد.

### نخستین نصب روی macOS

macOS ممکن است بگوید برنامه «آسیب دیده» یا توسعه‌دهنده قابل تأیید نیست.
نسخه‌های بدون امضا یا تأیید notarization ممکن است مسدود شوند، اما این
پیام‌ها ایمن بودن فایل را ثابت نمی‌کنند؛ خرابی یا تغییرات مخرب نیز ممکن است.

۱. پیش از نصب، مطمئن شوید DMG از Release رسمی پروژه دریافت شده و با
پلتفرم و نسخه موردنظر مطابقت دارد. اگر ناشر مقدار checksum ارائه کرده،
آن را تطبیق دهید؛ تطابق به معنای نبود بدافزار نیست. برای ساخت شخصی،
ابتدا منبع کد و فرایند ساخت را بررسی کنید. اگر منشأ یا یکپارچگی فایل
مشخص نیست یا checksum مطابقت ندارد، متوقف شوید و هشدار را دور نزنید.

۲. پس از بررسی، Polaris را به پوشه «Applications» در سمت راست بکشید
و باز کردن عادی آن را امتحان کنید.

۳. اگر تنها مانع، تأیید توسعه‌دهنده است و به نسخه بررسی‌شده اعتماد دارید،
در صورت وجود از گزینه «Open Anyway» در بخش «Privacy & Security»
در «System Settings» سیستم macOS استفاده کنید. پیام‌های سیستم را
بخوانید و تأیید کنید. رابط ممکن است در نسخه‌های مختلف macOS متفاوت باشد.

اگر macOS از خرابی یا بدافزار خبر داد یا برنامه همچنان باز نشد، متوقف
شوید. نسخه تازه‌ای از منبع رسمی دریافت و بررسی کنید یا از نگه‌دارندگان
پروژه کمک بگیرید. برای اجرای اجباری، ویژگی‌های قرنطینه را پاک نکنید و
حفاظت امنیتی سیستم را غیرفعال نکنید. پس از هر جایگزینی دستی، فایل را
دوباره بررسی کنید.

[README First.txt](packaging/macos-dmg-open-guide.txt) · [Apple Support](https://support.apple.com/en-us/102445)

### نخستین نصب روی Windows

هنگام نمایش SmartScreen گزینه «More info» ← «Run anyway» را انتخاب کنید.

## ساخت

این بخش برای نگه‌دارندگانی است که کد منبع فعلی را می‌سازند. ساخت موفق کد منبع به معنی تأیید عملکرد بسته‌های منتشرشده نیست. وابستگی‌های سکوها، ابزارهای موبایل و قراردادهای بسته‌بندی در راهنماهای زیر آمده است.

راهنمای ساخت از کد: [دسکتاپ و Android](docs/build-and-package.en.md) · [iOS / iPadOS](docs/ios-source-build.md) · [هسته موبایل و ابزارهای با نسخه ثابت](scripts/mobile-core/README.md).

برای iOS در حال حاضر IPA یا بسته جیلبریک منتشر نمی‌شود و سازگاری با جیلبریک تأیید نشده است. امضای شخصی برای دستگاه بدون جیلبریک به حساب Apple Developer Program با قابلیت Network Extension و [پروفایل‌های provisioning](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles) منطبق با مجوزهای App، ‏PacketTunnel و App Group نیاز دارد. حساب رایگان [Personal Team از NE provider پشتیبانی نمی‌کند](https://developer.apple.com/forums/thread/128767) و نصب خارج از فروشگاه به‌تنهایی این مجوزها را اعطا نمی‌کند. کامپایل یا نصب موفق جایگزین آزمون عملکرد نیست.

نیازمند Rust stable، Node.js 24+ (در CI اکنون Node 26)، pnpm 11.24.0، Go و [Tauri CLI 2](https://v2.tauri.app/).

[مراحل کامل ساخت دسکتاپ](docs/build-and-package.en.md#producing-installers) را دنبال کنید: ابتدا ابزارها و protoc/PATH، هستهٔ ثابت، helper (به‌همراه cleaner در Windows) و مانیفست هویت نصب را آماده کنید و سپس برنامه را بسازید. دریافت هسته و اجرای `tauri build` به‌تنهایی برای ساخت بستهٔ نصب کامل و قابل استفاده کافی نیست.

هسته در مخزن نگهداری نمی‌شود و باید پیش از بسته‌بندی دریافت شود. سوئیچ `--config` مخصوص هر سکو اختیاری نیست: نبود آن **بسته‌ای بدون هسته** می‌سازد، آن هم بدون هیچ خطایی در زمان ساخت — خرابی تنها در زمان اجرا آشکار می‌شود. شرح کامل، تقسیم کار در CI و قرارداد انتخاب بسته برای نصب‌کننده ویندوز و به‌روزرسان در [ساخت و بسته‌بندی](docs/build-and-package.en.md) آمده است.

دروازه‌های توسعه:

</div>

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd ui && npx pnpm@11.24.0 test
```

<div dir="rtl">

## معماری

</div>

```
ui/          React + Zustand + Vite + Tailwind
src-tauri/   فرایند اصلی Tauri 2
crates/      crateهای دامنه‌ای (config-engine / core-supervisor / helper / updater / …)
resources/   هسته sing-box + libcronet (در زمان ساخت دریافت می‌شوند، در مخزن نیستند)
```

<div dir="rtl">

در دسکتاپ (Windows / macOS / Linux)، هسته به‌صورت یک فرایند فرزند sidecar اجرا می‌شود و از طریق صفحه مدیریت gRPC کنترل می‌گردد. کارهای نیازمند دسترسی ویژه — راه‌اندازی هسته برای TUN و تصاحب سیستم — در هر سه سکو (macOS / Windows / Linux) به helperهای نوشته‌شده با Rust سپرده می‌شود؛ مسیر پروکسی سیستمی که به helper نیاز ندارد، مستقیماً توسط crate ‏`system-integration` در سمت برنامه مدیریت می‌شود و بازگردانی تنظیمات نیز بر عهده همان است.

در Android، ‏libbox در فرایند برنامه بارگذاری می‌شود و از یکپارچه‌سازی VPN موبایل استفاده می‌کند، نه مسیر دسکتاپ sidecar/helper که در بالا توضیح داده شد.

## مستندات

| فایل | محتوا |
|---|---|
| [docs/architecture.md](docs/architecture.md) | لایه‌بندی فعلی، تراکنش‌های اجرا، DNS / مسیریابی، مرزهای دسترسی ویژه و معیار تفکیک فایل‌های بزرگ (فقط به زبان چینی) |
| [docs/build-and-package.en.md](docs/build-and-package.en.md) | ساخت، CI، ثابت‌های بسته‌بندی، قرارداد انتخاب بسته توسط به‌روزرسان |
| [docs/troubleshooting.fa.md](docs/troubleshooting.fa.md) | محدودیت سازگاری MASQUE، نسخه‌های بدون امضا، صفحه سفید / خرابی تصویر / کرش GPU |

تصاویر با دستور `node scripts/capture-screenshots.mjs` ساخته می‌شوند: کروم بدون رابط گرافیکی، خروجی ساخت رابط کاربری را با داده‌های نمونه تزریق‌شده رندر می‌کند — نه نصب برنامه لازم است و نه اجرای هسته.

## وابستگی‌های بالادست

| پروژه | نقش |
|---|---|
| [sing-box-polaris (فورک مورد استفاده)](https://github.com/polaris-arch/sing-box-polaris) | فورک هسته مورد استفاده Polaris؛ کد و نسخه ثابت و فایل‌های sing-box-polaris بر اساس مانیفست ساخت انتخاب‌شده تعیین می‌شوند |
| [sing-box (بالادست اصلی)](https://github.com/SagerNet/sing-box) | پروژه اصلی بالادست و پایه فنی؛ با قدردانی از توسعه‌دهندگان |
| [Tauri 2](https://github.com/tauri-apps/tauri) | بستر اجرای دسکتاپ |
| [cronet-go](https://github.com/SagerNet/cronet-go) | libcronet برای NaiveProxy |
| [sing-box-dashboard](https://github.com/SagerNet/sing-box-dashboard) | پنل داخلی |
| [meta-rules-dat](https://github.com/MetaCubeX/meta-rules-dat) | مجموعه قوانین و داده‌های جغرافیایی (`.srs`) |

حق نشر هر مؤلفه نزد پدیدآورندگان آن باقی است. مؤلفه‌هایی که به شکل زیرفرایند یا فایل باینری یکپارچه شده‌اند در `NOTICE` فهرست شده‌اند؛ وابستگی‌های سطح کد که به خروجی لینک می‌شوند (Tauri / React / چند صد crate زبان Rust) بسته‌به‌بسته در `THIRD-PARTY-LICENSES.md` ثبت شده‌اند.

## دامنه کاربرد و سلب مسئولیت

Polaris یک کلاینت پروکسی شبکه و ابزار عیب‌یابی همه‌منظوره است. این نرم‌افزار گره پروکسی، اشتراک یا سرویس شبکه ارائه، فروش یا نگهداری نمی‌کند. تنها در شرایطی از آن استفاده کنید که قوانین و مقررات محل خود، شرایط خدمات مربوط و مقررات شبکه‌ای را که در آن هستید رعایت می‌کنید و مجوزهای لازم را دارید؛ استفاده از آن برای دسترسی غیرمجاز، نقض حقوق دیگران یا هر سوءاستفاده غیرقانونی دیگر ممنوع است. ارزیابی قابل اعتماد بودن پیکربندی‌ها، گره‌ها و منابع شخص ثالث بر عهده کاربر است و کاربر مسئول رفتار خود و پیامدهای آن است.

این نرم‌افزار «همان‌گونه که هست» ارائه می‌شود و هیچ تضمینی درباره در دسترس بودن شبکه، ناشناس ماندن، امنیت، دسترسی به سرویسی خاص یا یکپارچگی داده‌ها نمی‌دهد. تغییرات TUN، پروکسی سیستمی، DNS و مسیریابی ممکن است اتصال شبکه را موقتاً مختل کند؛ پیش از عملیات مهم از پیکربندی خود پشتیبان بگیرید. جز در مواردی که قانون حاکم به‌صراحت مقرر کرده باشد، نگهدارندگان و مشارکت‌کنندگان مسئول هیچ زیان مستقیم یا غیرمستقیمی که از استفاده یا ناتوانی در استفاده از این نرم‌افزار ناشی شود نیستند. این متن مشاوره حقوقی یا حرفه‌ای نیست.

## پروانه

کد منبع خودِ Polaris با پروانه MIT است (به `LICENSE` مراجعه کنید). **اما شرایط انتشار محصول ساخته‌شده بسته به پلتفرم متفاوت است:**

- **دسکتاپ (Windows / macOS / Linux): MIT.** sing-box (با پروانه GPLv3) به‌صورت فرایند فرزند sidecar یکپارچه شده و بر پایه‌ی موضع mere aggregation بر پروانه این پروژه اثری ندارد. **این موضع در دادگاه آزموده نشده است** — به `NOTICE` مراجعه کنید.
- **اندروید: در کل GPLv3.** در این پلتفرم sing-box تنها می‌تواند به شکل libbox درون فرایند برنامه بارگذاری شود (توصیفگر tun فقط از راه یک واسط درون‌فرایندی در دسترس است)، بنابراین mere aggregation کاربرد ندارد؛ لایه‌ی Kotlin نیز از sing-box-for-android با پروانه GPLv3 پورت شده است. هر کس APK را بگیرد GPLv3 دریافت می‌کند؛ هر کس تنها فایل‌های منبع MIT این مخزن را بردارد همچنان MIT دریافت می‌کند.

مؤلفه‌های شخص ثالث و استدلال کامل در `NOTICE` آمده است.

## روند ستاره‌ها

</div>

<a href="https://www.star-history.com/?repos=polaris-arch%2FPolaris&type=date&legend=top-left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=polaris-arch/Polaris&type=date&theme=dark&legend=top-left&sealed_token=TJg9RA5l3wyd1IgSMMq05QxhNvxS_OcrWbDJxZuwdUwgs-zVIBeoZz2j6swI3y5BxlztkoJMSkkxL6ZbZtw6oyqaRHZSAv0ZS60aVPPuBMdvm8tkxUjyKN1ttiVtPUwJEKObGpBH7BsPhjr6JwFfl_20UYjxgRVOq_V_Q6gKleib6K8LqP3K3nSwPvIJ" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=polaris-arch/Polaris&type=date&legend=top-left&sealed_token=TJg9RA5l3wyd1IgSMMq05QxhNvxS_OcrWbDJxZuwdUwgs-zVIBeoZz2j6swI3y5BxlztkoJMSkkxL6ZbZtw6oyqaRHZSAv0ZS60aVPPuBMdvm8tkxUjyKN1ttiVtPUwJEKObGpBH7BsPhjr6JwFfl_20UYjxgRVOq_V_Q6gKleib6K8LqP3K3nSwPvIJ" />
    <img alt="Polaris Star History Chart" src="https://api.star-history.com/chart?repos=polaris-arch/Polaris&type=date&legend=top-left&sealed_token=TJg9RA5l3wyd1IgSMMq05QxhNvxS_OcrWbDJxZuwdUwgs-zVIBeoZz2j6swI3y5BxlztkoJMSkkxL6ZbZtw6oyqaRHZSAv0ZS60aVPPuBMdvm8tkxUjyKN1ttiVtPUwJEKObGpBH7BsPhjr6JwFfl_20UYjxgRVOq_V_Q6gKleib6K8LqP3K3nSwPvIJ" />
  </picture>
</a>
