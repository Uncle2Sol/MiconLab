//! 原生多语言（资源文件驱动，英文为源语言，简体中文为第二语言）。
//!
//! ## 方案
//! 自建轻量字典层：`locales/*.toml` 由 `build.rs` 在编译期扫描并
//! `include_str!` 嵌入二进制，运行时解析一次。没有额外运行时依赖，也没有
//! 语言包文件的分发/路径问题；新增语种只需新增一个 `<BCP-47>.toml`，
//! 不用改任何 Rust 代码。
//!
//! - **源语言**：英文。代码里的 key 就是英文原文（gettext 风格），
//!   找不到翻译时原样返回，因此永远不会出现空白界面。
//! - **占位符**：`{name}` 形式，用 [`t_args`] 替换；不会与 `format!` 冲突
//!   （替换发生在我们自己的函数里）。
//! - **复数**：`key.one` / `key.other` 两个条目，用 [`tn`] 选择。
//!   中文没有复数变化，只需在中文语言包里同时提供两个条目（或省略，
//!   省略时回退到 `key`）。
//! - **编译期检查**：key 是运行时查表，编译器不校验 key 是否存在；
//!   作为补偿，`tests` 里有一条测试扫描 `src/**/*.rs` 中所有字面量 key，
//!   校验每个非英文语言包都提供了翻译（缺 key 会在 `cargo test` 阶段暴露）。
//!
//! ## 字体
//! egui 默认字体不含 CJK 字形。本模块把 **Noto Sans SC（SIL OFL 1.1）**
//! 子集嵌入二进制（见 `assets/fonts/`），并作为 Proportional / Monospace
//! 两个家族的**最后一级 fallback**：拉丁字符仍用 egui 默认字体，中文自动
//! 落到内置字体。字体走 `include_bytes!`，三平台（Windows / macOS / Linux）
//! 加载路径完全一致，不依赖系统字体目录。
//!
//! 子集化命令（新增中文文案后重新生成，覆盖当前源码里出现的全部字符）：
//! ```text
//! pyftsubset NotoSansCJKsc-Regular.otf \
//!   --text-file=<源码中全部字符> \
//!   --output-file=assets/fonts/NotoSansSC-Subset.otf \
//!   --layout-features='' --no-hinting --desubroutinize --name-IDs='*'
//! ```
//! 当前子集 240 KB（原始 OTF 约 16 MB，增量约 0.24 MB）。

include!(concat!(env!("OUT_DIR"), "/locales_generated.rs"));

use std::collections::HashMap;
#[cfg(not(test))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// 一个语言标识（BCP-47 子集，如 `en`、`zh-CN`）。
///
/// 内部保存 `&'static str`，来源只有两处：生成的语言包索引（`'static`）
/// 或 [`Language::resolve`] 的兜底常量，因此可以安全地按值复制、比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language(&'static str);

#[allow(non_upper_case_globals)]
impl Language {
    /// 英文（源语言 / 兜底语言）
    pub const En: Language = Language("en");
    /// 简体中文
    pub const Zh: Language = Language("zh-CN");

    pub fn code(self) -> &'static str {
        self.0
    }

    /// 按配置里的语言代码精确匹配（大小写不敏感）。
    pub fn from_code(code: &str) -> Option<Self> {
        let code = code.trim();
        if code.is_empty() {
            return None;
        }
        LOCALE_FILES
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(code))
            .map(|(c, _)| Language(c))
    }

    /// 把系统返回的 locale 解析成已支持的语言。
    ///
    /// 依次尝试：精确匹配 → 语言前缀（`zh-Hans-CN` → `zh`）→ 英文兜底。
    /// 中文变体（`zh`、`zh-Hans*`、`zh-SG` 等）统一归到 `zh-CN`。
    pub fn resolve(code: &str) -> Self {
        if let Some(lang) = Self::from_code(code) {
            return lang;
        }
        let lower = code.trim().to_ascii_lowercase();
        if lower.starts_with("zh") {
            if let Some(lang) = Self::from_code("zh-CN") {
                return lang;
            }
        }
        let primary = lower.split(['-', '_']).next().unwrap_or("");
        if let Some((c, _)) = LOCALE_FILES
            .iter()
            .find(|(c, _)| c.split('-').next() == Some(primary))
        {
            return Language(c);
        }
        Language::En
    }

    /// 跟随系统语言（三平台由 `sys-locale` 统一读取）。
    pub fn system() -> Self {
        let raw = sys_locale::get_locale().unwrap_or_else(|| "en".to_string());
        Self::resolve(&raw)
    }

    /// 所有可用语言（语言包来自 `locales/*.toml`，新增文件即出现）。
    pub fn all() -> Vec<LanguageInfo> {
        locales()
            .iter()
            .map(|l| LanguageInfo {
                language: l.language,
                native_name: l.native_name,
            })
            .collect()
    }
}

/// 语言选择器里展示的一项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageInfo {
    pub language: Language,
    /// 该语言自己的名字（English / 简体中文），不随界面语言变化
    pub native_name: &'static str,
}

/// 运行时解析后的语言包。
struct Locale {
    language: Language,
    native_name: &'static str,
    /// key（英文字面量或点分 key）→ 译文，值已 leak 成 `'static`
    strings: HashMap<&'static str, &'static str>,
}

static LOCALES: OnceLock<Vec<Locale>> = OnceLock::new();

#[cfg(not(test))]
static CURRENT: AtomicUsize = AtomicUsize::new(0);

// 测试里语言索引按线程隔离：`cargo test` 各用例并发跑，
// 全局切换会让「断言某句译文」的用例互相踩踏。
#[cfg(test)]
thread_local! {
    static TEST_LANG_INDEX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn current_index() -> usize {
    #[cfg(not(test))]
    {
        CURRENT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_LANG_INDEX.with(|c| c.get())
    }
}

fn store_index(index: usize) {
    #[cfg(not(test))]
    {
        CURRENT.store(index, Ordering::Relaxed);
    }
    #[cfg(test)]
    {
        TEST_LANG_INDEX.with(|c| c.set(index));
    }
}

fn locales() -> &'static [Locale] {
    LOCALES.get_or_init(|| {
        LOCALE_FILES
            .iter()
            .map(|(code, text)| {
                let mut native_name: &'static str = code;
                let mut strings: HashMap<&'static str, &'static str> = HashMap::new();

                // 语言包格式：顶层字符串表。`__name` 是语言自称，其余都是翻译。
                let table: toml::Table = toml::from_str(text).unwrap_or_default();
                for (key, value) in table {
                    let Some(value) = value.as_str() else {
                        continue;
                    };
                    let value: &'static str = Box::leak(value.to_string().into_boxed_str());
                    if key == "__name" {
                        native_name = value;
                    } else {
                        let key: &'static str = Box::leak(key.into_boxed_str());
                        strings.insert(key, value);
                    }
                }

                Locale {
                    language: Language(code),
                    native_name,
                    strings,
                }
            })
            .collect()
    })
}

/// 跟随系统语言（三平台由 `sys-locale` 统一读取）。
pub fn system_language() -> Language {
    Language::system()
}

/// 当前界面语言（界面代码直接调 `t()`，此函数主要供测试与诊断使用）。
#[cfg_attr(not(test), allow(dead_code))]
pub fn current_language() -> Language {
    let list = locales();
    let index = current_index().min(list.len().saturating_sub(1));
    list.get(index).map(|l| l.language).unwrap_or(Language::En)
}

/// 切换界面语言（立即生效：所有 `t()` 下一次调用就返回新语言）。
pub fn set_language(lang: Language) {
    let list = locales();
    let index = list
        .iter()
        .position(|l| l.language == lang)
        .unwrap_or_else(|| list.iter().position(|l| l.language == Language::En).unwrap_or(0));
    store_index(index);
}

/// 翻译一个静态 key。找不到时返回 key 本身（英文源语言兜底）。
///
/// `##` 之后是上下文后缀（同一句英文在不同位置有不同译法时使用），
/// 查不到时显示 `##` 之前的英文原文。
pub fn t(key: &'static str) -> &'static str {
    let list = locales();
    let index = current_index().min(list.len().saturating_sub(1));
    match list.get(index) {
        Some(locale) => locale
            .strings
            .get(key)
            .copied()
            .unwrap_or_else(|| key.split("##").next().unwrap_or(key)),
        None => key.split("##").next().unwrap_or(key),
    }
}

/// 翻译一个运行时 key（例如来自配置/插件），返回拥有所有权的字符串。
pub fn t_owned(key: &str) -> String {
    let list = locales();
    let index = current_index().min(list.len().saturating_sub(1));
    list.get(index)
        .and_then(|locale| locale.strings.get(key).copied())
        .unwrap_or_else(|| key.split("##").next().unwrap_or(key))
        .to_string()
}

/// 带占位符的翻译：`{name}` 会被 `args` 里的值替换。
#[cfg_attr(not(test), allow(dead_code))]
pub fn t_args(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut text = t(key).to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// 复数选择：优先 `key.one` / `key.other`，缺失时回退到 `key`。
/// 同时把 `{n}` / `{count}` 替换为实际数量。
#[allow(dead_code)] // 当前无调用点，保留给需要单复数的统计文案
pub fn tn(key: &'static str, n: u64) -> String {
    let suffix = if n == 1 { ".one" } else { ".other" };
    let plural_key = format!("{key}{suffix}");
    let text = t_owned(&plural_key);
    let text = if text == plural_key { t(key) } else { &text };
    text.replace("{n}", &n.to_string())
        .replace("{count}", &n.to_string())
}

/// 运行时格式化模板（Rust 的 `format!` 只接受字符串字面量，无法直接用译文）。
///
/// `parts` 每项是 `(英文占位符, 中文占位符, 已格式化好的值)`。函数按顺序替换：
/// 英文占位符用于英文模板，中文占位符用于中文模板（两者可能不同，例如
/// `{:.1}` 与 `{}`）。同一占位符重复出现时按 `parts` 顺序逐个替换。
pub fn tf(key: &'static str, parts: &[(&str, &str, &str)]) -> String {
    let mut out = t(key).to_string();
    for (en_ph, zh_ph, value) in parts {
        if out.contains(en_ph) {
            out = out.replacen(en_ph, value, 1);
        }
        if zh_ph != en_ph && out.contains(zh_ph) {
            out = out.replacen(zh_ph, value, 1);
        }
    }
    out
}

/// 兼容旧的 `L10n` 调用方式：绑定一个语言并调用 `t()`。
///
/// 新代码可以直接用模块级 [`t`]（跟随全局当前语言，切换后立即生效）。
#[derive(Debug, Clone, Copy)]
pub struct L10n {
    lang: Language,
}

impl L10n {
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }

    /// 用绑定语言翻译（与当前全局语言无关，保留给需要固定语言的场景）。
    pub fn t(&self, key: &'static str) -> &'static str {
        let list = locales();
        list.iter()
            .find(|l| l.language == self.lang)
            .and_then(|locale| locale.strings.get(key).copied())
            .unwrap_or_else(|| key.split("##").next().unwrap_or(key))
    }
}

const CJK_FONT: &[u8] =
    include_bytes!("../assets/fonts/NotoSansSC-Subset.otf");

/// 注册内置 CJK 字体：作为两个字体族的最后一级 fallback。
///
/// 必须在创建 `egui::Context` 后尽早调用一次。字体是编译进二进制的，
/// Windows / macOS / Linux 走同一份字节，不存在平台字体路径差异。
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto-sans-sc".to_owned(),
        egui::FontData::from_static(CJK_FONT),
    );
    for family in [
        egui::FontFamily::Proportional,
        egui::FontFamily::Monospace,
    ] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("noto-sans-sc".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// 内置 CJK 字体字节（测试与体积统计用）。
#[cfg_attr(not(test), allow(dead_code))]
pub fn cjk_font_bytes() -> &'static [u8] {
    CJK_FONT
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 公开版（未开启 device-workspaces）跳过设备专用 key 的完整性检查：
    /// 这些 key 只存在于未导出的设备源码与 `*.device.toml` overlay 中。
    fn is_device_key(key: &str) -> bool {
        if cfg!(feature = "device-workspaces") {
            return false;
        }
        let k = key.to_ascii_lowercase();
        [
            "insole",
            "cushion",
            "fa008",
            "jq care",
            "jqcare",
            "panels/insole",
            "panels/cushion",
            "devices/",
            "device_hub",
            "device workspaces",
        ]
        .iter()
        .any(|p| k.contains(p))
            || key.contains("鞋垫")
            || key.contains("坐垫")
    }

    #[test]
    fn language_packs_are_loaded() {
        let all = Language::all();
        assert!(all.iter().any(|l| l.language == Language::En));
        assert!(all.iter().any(|l| l.language == Language::Zh));
        // 语言自称来自资源文件，不写死在代码里
        assert_eq!(
            all.iter().find(|l| l.language == Language::Zh).unwrap().native_name,
            "简体中文"
        );
    }

    #[test]
    fn switching_language_takes_effect_immediately() {
        let old = current_language();
        set_language(Language::En);
        assert_eq!(t("Connected"), "Connected");
        set_language(Language::Zh);
        assert_eq!(t("Connected"), "已连接");
        set_language(old);
    }

    #[test]
    fn unknown_key_falls_back_to_source_text() {
        set_language(Language::Zh);
        assert_eq!(t("__definitely_not_a_key__"), "__definitely_not_a_key__");
    }

    #[test]
    fn placeholders_and_plural_forms_work() {
        set_language(Language::En);
        assert_eq!(t_args("Hello, {name}", &[("name", "Ada")]), "Hello, Ada");
        assert_eq!(tn("file.one", 1), "file.one"); // 未定义复数条目时回退
    }

    #[test]
    fn system_locale_resolution_is_generic() {
        assert_eq!(Language::resolve("zh-Hans-CN"), Language::Zh);
        assert_eq!(Language::resolve("zh_CN"), Language::Zh);
        assert_eq!(Language::resolve("en-US"), Language::En);
        // 未支持的语言 → 英文兜底，不 panic
        assert_eq!(Language::resolve("fr-FR"), Language::En);
    }

    /// 编译期/测试期检查：源码里所有字面量 `t("...")` key，
    /// 非英文语言包必须提供翻译，否则运行时会露出英文。
    #[test]
    fn every_source_string_has_a_translation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing: Vec<(String, String)> = Vec::new();
        let mut files = Vec::new();
        collect_rs_files(&root, &mut files);

        set_language(Language::Zh);
        let zh = locales()
            .iter()
            .find(|l| l.language == Language::Zh)
            .expect("缺少 zh-CN 语言包");

        for file in files {
            // 只看非测试代码：测试里允许出现故意不存在的 key
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for key in extract_t_keys(code) {
                if is_device_key(&key) {
                    continue;
                }
                if !zh.strings.contains_key(key.as_str()) {
                    missing.push((
                        file.strip_prefix(&root).unwrap_or(&file).display().to_string(),
                        key,
                    ));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "以下 {} 个 key 缺少中文翻译（前 20 条）：{:?}",
            missing.len(),
            missing.iter().take(20).collect::<Vec<_>>()
        );
    }

    /// 点分稳定 ID（如 `report.protocol.section.mcu`、`burn.report.summary.esp32`）
    /// 必须同时存在于**所有**语言包：这类 key 不会回退成有意义的英文，
    /// 缺条目时界面会直接显示 ID，所以用测试在编译期之外兜住拼写错误。
    #[test]
    fn dotted_ids_exist_in_every_language() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&root, &mut files);

        let is_dotted_id = |key: &str| {
            key.split('.').count() >= 2
                && key.split('.').all(|seg| {
                    !seg.is_empty()
                        && seg
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                })
        };

        let mut used: Vec<String> = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            used.extend(
                extract_t_keys(code)
                    .into_iter()
                    .filter(|k| is_dotted_id(k) && !is_device_key(k)),
            );
        }
        used.sort();
        used.dedup();

        let mut missing: Vec<(String, String)> = Vec::new();
        for locale in locales() {
            for key in &used {
                if !locale.strings.contains_key(key.as_str()) {
                    missing.push((locale.language.code().to_string(), key.clone()));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "以下 {} 个点分 key 缺少语言包条目（前 20 条）：{:?}",
            missing.len(),
            missing.iter().take(20).collect::<Vec<_>>()
        );
    }

    fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_rs_files(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
    }

    /// 提取 `t("key")` / `t_args("key"` / `tn("key"` 里的第一个字符串字面量。
    fn extract_t_keys(text: &str) -> Vec<String> {
        let mut keys = Vec::new();
        for func in ["t(", "tf(", "t_args(", "tn("] {
            let mut search = 0;
            while let Some(pos) = text[search..].find(func) {
                let abs = search + pos;
                search = abs + func.len();

                // 排除 `save_state(` / `insert(` 这类以 t( 结尾的普通函数调用：
                // 只接受 `t(`、`.t(`、`::t(`（前一个字符不能是标识符字符）。
                let preceded_by_ident = text[..abs]
                    .chars()
                    .next_back()
                    .map(|c| c.is_alphanumeric() || c == '_')
                    .unwrap_or(false);
                if preceded_by_ident {
                    continue;
                }

                let rest = text[search..].trim_start();
                let Some(after_quote) = rest.strip_prefix('"') else {
                    continue;
                };
                let Some(end) = after_quote.find('"') else {
                    continue;
                };
                let raw = &after_quote[..end];
                if !raw.is_empty() {
                    keys.push(decode_rust_escapes(raw));
                }
            }
        }
        keys
    }

    /// 把 Rust 字符串字面量里的转义解码成运行时值，
    /// 这样 TOML 里的 key（真实字符）才能和源码字面量对上。
    fn decode_rust_escapes(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                match chars[i + 1] {
                    '"' => { out.push('"'); i += 2; continue; }
                    '\'' => { out.push('\''); i += 2; continue; }
                    '\\' => { out.push('\\'); i += 2; continue; }
                    'n' => { out.push('\n'); i += 2; continue; }
                    'r' => { out.push('\r'); i += 2; continue; }
                    't' => { out.push('\t'); i += 2; continue; }
                    'u' if i + 2 < chars.len() && chars[i + 2] == '{' => {
                        if let Some(close) = chars[i + 3..].iter().position(|c| *c == '}') {
                            let hex: String = chars[i + 3..i + 3 + close].iter().collect();
                            if let Ok(code) = u32::from_str_radix(&hex.replace('_', ""), 16) {
                                if let Some(ch) = char::from_u32(code) {
                                    out.push(ch);
                                    i += 3 + close + 1;
                                    continue;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    #[test]
    fn embedded_cjk_font_contains_chinese_glyphs() {
        let face = ttf_parser::Face::parse(cjk_font_bytes(), 0)
            .expect("内置字体无法解析（三平台都应一致）");
        for ch in ['中', '文', '连', '接', '数', '据'] {
            assert!(
                face.glyph_index(ch).is_some(),
                "内置字体缺少字形: {ch}"
            );
        }
        assert!(
            cjk_font_bytes().len() < 1024 * 1024,
            "字体子集应小于 1 MB，当前 {} 字节",
            cjk_font_bytes().len()
        );
    }

    /// 走一遍 egui 真实的字体注册/排版路径（不依赖系统字体），
    /// 相当于三平台上应用启动时的行为。
    #[test]
    fn egui_can_render_chinese_with_embedded_font() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        ctx.begin_pass(egui::RawInput::default());
        let font_id = egui::FontId::proportional(14.0);
        let ok = ctx.fonts(|f| f.has_glyphs(&font_id, "中文连接数据"));
        let _ = ctx.end_pass();
        assert!(ok, "egui 字体系统里找不到中文字形（字体注册失败）");
    }
}
