# Application fonts

Noto Sans Regular and SemiBold are version 2.015 from `notofonts/latin-greek-cyrillic`.

Arabic and Tifinagh fallbacks preserve multilingual map names. The unmodified hinted regular faces come from
[`notofonts/noto-fonts`](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf),
revision `ffebf8c1ee449e544955a7e813c54f9b73848eac`:

| File                           | Version | SHA-256                                                            |
| ------------------------------ | ------- | ------------------------------------------------------------------ |
| `NotoSansArabic-Regular.ttf`   | 2.009   | `ceea25b464a656dc3b26849bab9356740401af62aedf1bfa8b7f0d9b75925b1b` |
| `NotoSansTifinagh-Regular.ttf` | 2.002   | `72066fbbf300dc08644b1f1a30fa25b205c9c15c82e1566d1f4aae2ebf00659c` |

All use OFL-1.1; `infra/nix/licenses.nix` includes their notices in both application bundles.
These fallbacks cover the reported North African labels, not every Unicode script.
