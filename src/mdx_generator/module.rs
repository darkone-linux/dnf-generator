//! Modules reference pages.
//!
//! The reference is split across one index page
//! (`doc/src/content/docs/en/ref/modules.mdx`) listing the categories, plus one
//! page per category (`doc/src/content/docs/en/ref/modules/<slug>.mdx`) holding
//! the actual module documentation. Generic Nix/MDX primitives live in
//! `nix_parser` and `mdx_writer`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::mdx_generator::mdx_writer::escape;
use crate::mdx_generator::nix_parser::{
    extract_first_comment, extract_module_path, extract_nix_files, parse_module_options, NixOption,
};

/// Directory (relative to the project root) holding the generated English
/// reference pages. The index lives at `<DOC_DIR>/modules.mdx` and each category
/// at `<DOC_DIR>/modules/<slug>.mdx`.
const DOC_DIR: &str = "doc/src/content/docs/en/ref";

const INDEX_FRONTMATTER: &str =
    "---\ntitle: Nixos DNF Modules\nsidebar:\n  order: 1\n  label: Overview\n---\n";

/// Language of the generated pages. Starlight serves every page under
/// `/<lang>/`, so root-absolute internal links must carry this prefix. The
/// translation step (`scripts/translate.mjs`) later rewrites `/en/` → `/fr/`
/// for the localized pages, so emitting the source language here is enough.
const LANG: &str = "en";

static INTERNAL_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(/([^)]*)\)").unwrap());

/// Internal links targeting an anchor on the (former) single modules page:
/// `](/ref/modules/#some-anchor)`. These are rewritten to the right category
/// page now that modules are split (see [`rewrite_cross_links`]).
static MODULES_ANCHOR_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\(/ref/modules/#([^)]+)\)").unwrap());

/// Prepend the page locale to root-absolute internal Markdown links written
/// without one (e.g. `](/ref/modules/mixin/)` → `](/en/ref/modules/mixin/)`).
/// Links already carrying a 2-letter locale prefix, anchors (`](#...)`) and
/// external URLs (`](http...)`) are left untouched.
fn localize_links(content: &str) -> String {
    INTERNAL_LINK
        .replace_all(content, |caps: &regex::Captures| {
            let path = &caps[1];
            let first = path.split('/').next().unwrap_or("");
            let is_locale = first.len() == 2 && first.chars().all(|c| c.is_ascii_lowercase());
            if is_locale {
                format!("](/{path})")
            } else {
                format!("](/{LANG}/{path})")
            }
        })
        .into_owned()
}

struct Category {
    title: &'static str,
    short: &'static str,
    description: &'static str,
    /// Source directory (relative to project root) scanned for `.nix` modules.
    relative_dir: &'static str,
    /// Dotted option prefix shared by the category's modules.
    prefix: &'static str,
    /// Page slug — the `<slug>.mdx` filename and `/ref/modules/<slug>/` URL.
    slug: &'static str,
    icon: &'static str,
}

const CATEGORIES: &[Category] = &[
    Category {
        title: "Mixin modules",
        short: "Mixins",
        description: "**A mixin module** defines a collection of standard modules with a consistent common configuration.",
        relative_dir: "dnf/modules/mixin",
        prefix: "darkone.",
        slug: "mixin",
        icon: "&#x1F4E6;",
    },
    Category {
        title: "Service modules",
        short: "Services",
        description: "**A service module** provides a ready-to-use service (daemon).",
        relative_dir: "dnf/modules/service",
        prefix: "darkone.service.",
        slug: "service",
        icon: "&#x2728;",
    },
    Category {
        title: "System modules",
        short: "System",
        description: "**A system module** manages common, important, and general configurations.",
        relative_dir: "dnf/modules/system",
        prefix: "darkone.system.",
        slug: "system",
        icon: "&#x2699;",
    },
    Category {
        title: "Security modules",
        short: "Security",
        description: "**A security module** hardens the system, its programs and functionalities.",
        relative_dir: "dnf/modules/security",
        prefix: "darkone.security.",
        slug: "security",
        icon: "&#x1F511;",
    },
    Category {
        title: "CLI applications",
        short: "CLI Apps",
        description: "**A CLI application module** installs terminal applications and their configuration optimized for DNF.",
        relative_dir: "dnf/modules/console",
        prefix: "darkone.console.",
        slug: "console",
        icon: "&#x1F4BB;",
    },
    Category {
        title: "GUI applications",
        short: "GUI Apps",
        description: "**A GUI application module** installs graphical applications and their configuration optimized for DNF.",
        relative_dir: "dnf/modules/graphic",
        prefix: "darkone.graphic.",
        slug: "graphic",
        icon: "&#x1F5BC;",
    },
    Category {
        title: "Administration modules",
        short: "Administration",
        description: "**A administration module** deals with system-specific functionalities for administrators.",
        relative_dir: "dnf/modules/admin",
        prefix: "darkone.admin.",
        slug: "admin",
        icon: "&#x1F451;",
    },
    Category {
        title: "Users management modules",
        short: "Users Management",
        description: "**A users management module** manage user-related processing or special user settings.",
        relative_dir: "dnf/modules/user",
        prefix: "darkone.user.",
        slug: "user",
        icon: "&#x1F464;",
    },
    Category {
        title: "Home Manager modules",
        short: "Home Manager",
        description: "**A home manager module** works with [home manager](https://github.com/nix-community/home-manager) profiles.",
        relative_dir: "dnf/home/modules",
        prefix: "darkone.home.",
        slug: "home",
        icon: "&#x1F3E0;",
    },
];

struct ModuleEntry {
    path: String,
    comment: Option<String>,
    options: Vec<NixOption>,
}

/// A category together with the modules collected from its source directory.
struct CollectedCategory {
    category: &'static Category,
    entries: Vec<ModuleEntry>,
}

/// A generated page: its path relative to the project root and its content.
pub struct Page {
    pub rel_path: String,
    pub content: String,
}

/// Build every reference page for `project_root`: the index plus one page per
/// non-empty category. Categories with no module are skipped entirely (no page,
/// no card) to avoid dead links.
pub fn generate_pages(project_root: &Path) -> Vec<Page> {
    let collected: Vec<CollectedCategory> = CATEGORIES
        .iter()
        .map(|category| CollectedCategory {
            category,
            entries: collect_entries(&project_root.join(category.relative_dir), category.prefix),
        })
        .filter(|c| !c.entries.is_empty())
        .collect();

    let anchors = build_anchor_map(&collected);

    let mut pages = vec![Page {
        rel_path: format!("{DOC_DIR}/modules/index.mdx"),
        content: finalize(&render_index(&collected), &anchors),
    }];

    for (i, c) in collected.iter().enumerate() {
        pages.push(Page {
            rel_path: format!("{DOC_DIR}/modules/{}.mdx", c.category.slug),
            content: finalize(&render_category_page(c, i + 2), &anchors),
        });
    }
    pages
}

/// Apply the document-wide link transforms in order: rewrite the old
/// single-page anchors to their new category page, then localize root-absolute
/// links by prefixing the page locale.
fn finalize(content: &str, anchors: &HashMap<String, String>) -> String {
    localize_links(&rewrite_cross_links(content, anchors))
}

// ─── index page ─────────────────────────────────────────────────────────────

fn render_index(collected: &[CollectedCategory]) -> String {
    let mut out = String::new();
    out.push_str(INDEX_FRONTMATTER);
    out.push_str("\nimport { CardGrid, LinkCard } from '@astrojs/starlight/components';\n\n");
    out.push_str(
        "Darkone NixOS Framework modules are grouped by category. Pick a category below to \
         browse its modules and their options.\n\n",
    );
    out.push_str("<CardGrid>\n");
    for c in collected {
        out.push_str(&format!(
            "  <LinkCard title=\"{} {}\" description=\"{}\" href=\"/{LANG}/ref/modules/{}/\" />\n",
            entity_to_char(c.category.icon),
            c.category.title,
            attr_text(c.category.description),
            c.category.slug,
        ));
    }
    out.push_str("</CardGrid>\n");
    out
}

// ─── category page ───────────────────────────────────────────────────────────

fn render_category_page(c: &CollectedCategory, order: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "---\ntitle: {}\nsidebar:\n  order: {order}\n  label: {}\n---\n\n",
        c.category.title, c.category.short
    ));
    out.push_str(&format!(":::note\n{}\n:::\n\n", c.category.description));
    for entry in &c.entries {
        out.push_str(&render_module(entry, c.category.icon));
    }
    out
}

// ─── cross-page link rewriting ────────────────────────────────────────────────

/// Map every anchor that used to live on the single `modules.mdx` page to its
/// new location. Module headings (`### <icon> <path>`) keep their anchor but
/// move to the owning category page; the former category section headings
/// (`## <Category title>`) become category pages, so their anchor maps to the
/// page root (no fragment).
fn build_anchor_map(collected: &[CollectedCategory]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for c in collected {
        let slug = c.category.slug;
        map.insert(gh_slug(c.category.title), format!("/ref/modules/{slug}/"));
        for entry in &c.entries {
            let anchor = module_anchor(&entry.path);
            map.insert(anchor.clone(), format!("/ref/modules/{slug}/#{anchor}"));
        }
    }
    map
}

fn rewrite_cross_links(content: &str, anchors: &HashMap<String, String>) -> String {
    MODULES_ANCHOR_LINK
        .replace_all(content, |caps: &regex::Captures| {
            match anchors.get(&caps[1]) {
                Some(target) => format!("]({target})"),
                None => caps[0].to_string(),
            }
        })
        .into_owned()
}

/// Anchor of a `### <icon> <path>` heading. Astro slugs the rendered text with
/// github-slugger; the icon entity decodes to an emoji that github-slugger
/// drops, leaving the separating space as a leading `-` (see `anchors.mjs`).
fn module_anchor(path: &str) -> String {
    format!("-{}", gh_slug(path))
}

/// Reproduce github-slugger over plain heading text (no emoji): lowercase, drop
/// the punctuation github-slugger strips, turn spaces into hyphens. Matches the
/// JS implementation reused by `doc/scripts/lib/anchors.mjs`.
fn gh_slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        if c == ' ' {
            out.push('-');
        } else if !is_slug_stripped(c) {
            out.push(c);
        }
    }
    out
}

fn is_slug_stripped(c: char) -> bool {
    matches!(
        c,
        '\\' | '\''
            | '!'
            | '"'
            | '#'
            | '$'
            | '%'
            | '&'
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | ']'
            | '^'
            | '`'
            | '{'
            | '|'
            | '}'
            | '~'
    ) || ('\u{2000}'..='\u{206F}').contains(&c)
        || ('\u{2E00}'..='\u{2E7F}').contains(&c)
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Turn a numeric HTML entity (`&#x1F4E6;`) into its character so it renders in
/// a JSX attribute (`title="📦 …"`). Non-entity input is returned unchanged.
fn entity_to_char(entity: &str) -> String {
    entity
        .strip_prefix("&#x")
        .and_then(|s| s.strip_suffix(';'))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .and_then(char::from_u32)
        .map(|c| c.to_string())
        .unwrap_or_else(|| entity.to_string())
}

static MD_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static MD_EMPHASIS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_~`]").unwrap());

/// Reduce inline Markdown to plain text for use inside a JSX attribute string
/// (`[text](url)` → `text`, drop emphasis/code markers).
fn attr_text(s: &str) -> String {
    let s = MD_LINK.replace_all(s, "$1");
    MD_EMPHASIS.replace_all(&s, "").into_owned()
}

// Collect nix modules contents
fn collect_entries(dir: &Path, prefix: &str) -> Vec<ModuleEntry> {
    let files: Vec<PathBuf> = extract_nix_files(dir);
    let mut entries = vec![];
    for file in files {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        entries.push(ModuleEntry {
            path: extract_module_path(&file, dir, prefix),
            comment: extract_first_comment(&source),
            options: parse_module_options(&source),
        });
    }
    entries
}

/// Escape `<` and `>` outside backtick-delimited code spans so that bare
/// angle sequences like `<->` don't get interpreted as JSX tags in MDX.
fn escape_angle_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for ch in text.chars() {
        match ch {
            '`' => { in_code = !in_code; out.push(ch); }
            '<' if !in_code => out.push_str("&lt;"),
            '>' if !in_code => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

fn render_module(entry: &ModuleEntry, icon: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("### {icon} {}\n\n", entry.path));
    if let Some(c) = &entry.comment {
        out.push_str(&escape_angle_brackets(c));
        out.push_str("\n\n");
    }
    out.push_str(&render_options(entry));
    out.push_str("<hr/>\n\n");
    out
}

fn render_options(entry: &ModuleEntry) -> String {
    if entry.options.is_empty() {
        return String::new();
    }
    let mut bullets = String::new();
    let mut code = String::new();
    let opt_count = entry.options.len();
    let multi = opt_count > 1;

    code.push_str("\n```nix\n");
    if multi {
        code.push_str(&format!("{} = {{\n", entry.path));
    }

    let prefix = if multi {
        "  ".to_string()
    } else {
        format!("{}.", entry.path)
    };

    // Track names by level so we can rebuild dotted paths inside opened submodules.
    let mut opened_levels: Vec<String> = vec![];
    let mut last_level = 1usize;
    let mut names_by_level: Vec<String> = vec![String::new()];

    for (i, opt) in entry.options.iter().enumerate() {
        // Adjust the stack of "opened" parent names.
        if opt.level > last_level {
            // Diving in: the previous option's name becomes a parent dotted prefix.
            while opened_levels.len() < opt.level - 1 {
                let idx = opened_levels.len() + 1;
                let parent = names_by_level.get(idx).cloned().unwrap_or_default();
                opened_levels.push(parent);
            }
        } else if opt.level <= last_level {
            // Coming back out (or staying flat): drop deeper parents.
            opened_levels.truncate(opt.level - 1);
        }

        // Remember this option's name at its level (parents may use it later).
        while names_by_level.len() <= opt.level {
            names_by_level.push(String::new());
        }
        names_by_level[opt.level] = opt.name.clone();

        let indent = "  ".repeat(opt.level - 1);
        bullets.push_str(&indent);
        bullets.push_str("* **");
        bullets.push_str(&escape(&opt.name));
        bullets.push_str("**");
        if !opt.type_label.is_empty() {
            bullets.push_str(&format!(" `{}`", opt.type_label));
        }
        if let Some(desc) = &opt.description {
            let trimmed = desc.trim().trim_matches('"');
            if !trimmed.is_empty() {
                bullets.push(' ');
                bullets.push_str(&indent_continuation(&escape(trimmed), &indent));
            }
        }
        bullets.push('\n');

        // Skip emitting `parent = { };` when its submodule body follows.
        let opens_submodule = matches!(opt.type_label.as_str(), "submodule" | "attrs");
        let next_is_deeper = entry
            .options
            .get(i + 1)
            .map(|n| n.level > opt.level)
            .unwrap_or(false);
        if opens_submodule && next_is_deeper {
            last_level = opt.level;
            continue;
        }

        let value = code_value(opt);
        let parents = opened_levels
            .iter()
            .map(|p| format!("{p}."))
            .collect::<String>();
        code.push_str(&prefix);
        code.push_str(&parents);
        code.push_str(&opt.name);
        code.push_str(" = ");
        code.push_str(&value);
        code.push_str(";\n");

        last_level = opt.level;
    }

    if multi {
        code.push_str("};\n");
    }
    code.push_str("```\n\n");

    format!("{bullets}{code}")
}

/// Indents the lines following the first one so a multi-paragraph description
/// stays inside its bullet: an unindented blank line closes the list item, and
/// what follows (`:::caution` blocks, nested bullets) then lands outside it.
fn indent_continuation(text: &str, indent: &str) -> String {
    let mut lines = text.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        if !line.trim().is_empty() {
            out.push_str(indent);
            out.push_str("  ");
            out.push_str(line);
        }
    }
    out
}

fn code_value(opt: &NixOption) -> String {
    if let Some(ex) = &opt.example {
        if !ex.is_empty() {
            return ex.clone();
        }
    }
    if let Some(d) = &opt.default {
        if !d.is_empty() {
            return d.clone();
        }
    }
    "null".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Concatenate every generated page — handy for assertions that don't care
    /// which file a fragment landed in.
    fn all_content(root: &Path) -> String {
        generate_pages(root)
            .iter()
            .map(|p| format!("{}\n{}", p.rel_path, p.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn page<'a>(pages: &'a [Page], rel_path: &str) -> &'a Page {
        pages
            .iter()
            .find(|p| p.rel_path == rel_path)
            .unwrap_or_else(|| panic!("missing page {rel_path}"))
    }

    #[test]
    fn renders_simple_module() {
        let dir = tempdir().unwrap();
        let dnf = dir.path().join("dnf/modules/security");
        fs::create_dir_all(&dnf).unwrap();
        fs::write(
            dnf.join("foo.nix"),
            r#"# A foo service.
{ lib, ... }: {
  options = {
    darkone.security.foo.enable = lib.mkEnableOption "Enable foo";
  };
}
"#,
        )
        .unwrap();

        let pages = generate_pages(dir.path());
        let mdx = &page(&pages, "doc/src/content/docs/en/ref/modules/security.mdx").content;
        assert!(mdx.contains("### &#x1F511; darkone.security.foo"));
        assert!(mdx.contains("A foo service."));
        assert!(mdx.contains("* **enable** `bool` Enable foo"));
        assert!(mdx.contains("darkone.security.foo.enable = false;"));
    }

    #[test]
    fn renders_one_line_module() {
        let dir = tempdir().unwrap();
        let dnf = dir.path().join("dnf/modules/security");
        fs::create_dir_all(&dnf).unwrap();
        fs::write(
            dnf.join("foo.nix"),
            r#"# A foo service.
{ lib, ... }: {
  options.darkone.security.foo.enable = lib.mkEnableOption "Enable foo";
}
"#,
        )
        .unwrap();

        let mdx = all_content(dir.path());
        assert!(mdx.contains("### &#x1F511; darkone.security.foo"));
        assert!(mdx.contains("A foo service."));
        assert!(mdx.contains("* **enable** `bool` Enable foo"));
        assert!(mdx.contains("darkone.security.foo.enable = false;"));
    }

    #[test]
    fn index_lists_categories_and_links_to_pages() {
        let dir = tempdir().unwrap();
        let dnf = dir.path().join("dnf/modules/service");
        fs::create_dir_all(&dnf).unwrap();
        fs::write(
            dnf.join("foo.nix"),
            r#"# A foo service.
{ lib, ... }: {
  options.darkone.service.foo.enable = lib.mkEnableOption "Enable foo";
}
"#,
        )
        .unwrap();

        let pages = generate_pages(dir.path());
        let index = &page(&pages, "doc/src/content/docs/en/ref/modules/index.mdx").content;
        // Index has the import, a card and a localized link to the category page.
        assert!(index.contains("import { CardGrid, LinkCard }"));
        assert!(index.contains("<LinkCard"));
        assert!(index.contains("href=\"/en/ref/modules/service/\""));
        // Index carries no module documentation; that lives on the category page.
        assert!(!index.contains("### "));
        // Only non-empty categories get a page.
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn rewrites_cross_page_anchor_links() {
        let dir = tempdir().unwrap();
        let svc = dir.path().join("dnf/modules/service");
        fs::create_dir_all(&svc).unwrap();
        fs::write(
            svc.join("dnsmasq.nix"),
            r#"# DNS and DHCP.
{ lib, ... }: {
  options.darkone.service.dnsmasq.enable = lib.mkEnableOption "Enable";
}
"#,
        )
        .unwrap();
        fs::write(
            svc.join("adguardhome.nix"),
            r#"# Uses the [dnsmasq module](/ref/modules/#-darkoneservicednsmasq) as upstream.
{ lib, ... }: {
  options.darkone.service.adguardhome.enable = lib.mkEnableOption "Enable";
}
"#,
        )
        .unwrap();

        let pages = generate_pages(dir.path());
        let mdx = &page(&pages, "doc/src/content/docs/en/ref/modules/service.mdx").content;
        // The cross-link now points at the service page anchor, locale-prefixed.
        assert!(mdx.contains("](/en/ref/modules/service/#-darkoneservicednsmasq)"));
        // The bare single-page anchor must not survive.
        assert!(!mdx.contains("](/ref/modules/#-darkoneservicednsmasq)"));
        assert!(!mdx.contains("](/en/ref/modules/#-darkoneservicednsmasq)"));
    }

    #[test]
    fn rewrites_category_section_links_to_page_root() {
        let dir = tempdir().unwrap();
        let svc = dir.path().join("dnf/modules/service");
        fs::create_dir_all(&svc).unwrap();
        fs::write(
            svc.join("foo.nix"),
            r#"# See the [service modules](/ref/modules/#service-modules) overview.
{ lib, ... }: {
  options.darkone.service.foo.enable = lib.mkEnableOption "Enable";
}
"#,
        )
        .unwrap();

        let pages = generate_pages(dir.path());
        let mdx = &page(&pages, "doc/src/content/docs/en/ref/modules/service.mdx").content;
        assert!(mdx.contains("](/en/ref/modules/service/)"));
        assert!(!mdx.contains("#service-modules"));
    }

    #[test]
    fn localizes_unprefixed_internal_links() {
        let dir = tempdir().unwrap();
        let dnf = dir.path().join("dnf/modules/service");
        fs::create_dir_all(&dnf).unwrap();
        fs::write(
            dnf.join("adguardhome.nix"),
            r#"# See [the fr page](/fr/ref/modules/#x) and [home](https://example.com).
{ lib, ... }: {
  options.darkone.service.adguardhome.enable = lib.mkEnableOption "Enable";
}
"#,
        )
        .unwrap();

        let mdx = all_content(dir.path());
        // Already-localized and external links are left untouched.
        assert!(mdx.contains("](/fr/ref/modules/#x)"));
        assert!(mdx.contains("](https://example.com)"));
        // No root-absolute link without a locale prefix remains.
        assert!(!mdx.contains("](/ref/"));
    }

    #[test]
    fn renders_submodule_block() {
        let dir = tempdir().unwrap();
        let dnf = dir.path().join("dnf/modules/system");
        fs::create_dir_all(&dnf).unwrap();
        fs::write(
            dnf.join("services.nix"),
            r#"{ lib, ... }: {
  options = {
    darkone.system.services.enable = lib.mkEnableOption "Enable manager";
    darkone.system.services.service = lib.mkOption {
      default = { };
      type = lib.types.attrsOf (lib.types.submodule (_: {
        options = {
          enable = lib.mkEnableOption "Enable proxy";
          persist.dirs = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            description = "Dirs";
          };
        };
      }));
      description = "Services";
    };
  };
}
"#,
        )
        .unwrap();

        let mdx = all_content(dir.path());
        assert!(mdx.contains("### &#x2699; darkone.system.services"));
        assert!(mdx.contains("* **service** `attrs` Services"));
        assert!(mdx.contains("  * **enable** `bool` Enable proxy"));
        assert!(mdx.contains("  * **persist.dirs** `listOf str` Dirs"));
        // Inside multi-option, nested code uses parent dotted prefix:
        assert!(mdx.contains("service.enable = false;"));
        assert!(mdx.contains("service.persist.dirs = [ ];"));
    }
}
