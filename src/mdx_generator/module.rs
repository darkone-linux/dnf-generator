//! Modules reference pages.
//!
//! Layout under `doc/src/content/docs/en/ref/modules/`:
//! - `index.mdx`: the categories;
//! - `<category>/index.mdx`: the modules of a category, one line each;
//! - `<category>/<module>.mdx`: one module, its header comment then its options.
//!
//! A sub-module (`<name>/<topic>.nix` next to `<name>.nix`) gets no page: its
//! parent's header documents it, its options land on its parent's page.
//!
//! Generic Nix/MDX primitives live in `nix_parser` and `mdx_writer`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::error::{NixError, Result};
use crate::mdx_generator::mdx_writer::escape;
use crate::mdx_generator::nix_parser::{
    extract_first_comment, extract_module_path, extract_nix_files, parse_module, ModuleOptions,
    NixOption,
};

/// Directory (relative to the project root) of the generated English pages.
/// The generator owns it: a page it no longer produces is removed.
pub const DOC_DIR: &str = "doc/src/content/docs/en/ref/modules";

/// Checkout of the framework in the project, stripped to get a path of its
/// repository.
const FRAMEWORK_DIR: &str = "dnf/";

/// Where a module page links its source file.
const SOURCE_URL: &str = "https://github.com/darkone-linux/darkone-nixos-framework/blob/main";

const INDEX_FRONTMATTER: &str =
    "---\ntitle: NixOS DNF modules\nsidebar:\n  order: 1\n  label: Overview\n---\n";

/// Language of the generated pages. Starlight serves every page under
/// `/<lang>/`, so root-absolute internal links must carry this prefix. The
/// translation step (`scripts/translate.mjs`) later rewrites `/en/` → `/fr/`
/// for the localized pages, so emitting the source language here is enough.
const LANG: &str = "en";

static INTERNAL_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(/([^)]*)\)").unwrap());

/// Links into the reference, once localized: Markdown targets and `href`s.
static REFERENCE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"(?:\]\(|href=")/{LANG}/ref/modules/([^)"\#]*)"#)).unwrap()
});

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
    description: &'static str,
    /// Source directory (relative to project root) scanned for `.nix` modules.
    relative_dir: &'static str,
    /// Dotted option prefix shared by the category's modules.
    prefix: &'static str,
    /// Directory of the category pages, hence the `/ref/modules/<slug>/` URL.
    slug: &'static str,
    icon: &'static str,
}

const CATEGORIES: &[Category] = &[
    Category {
        title: "Mixin modules",
        description: "**A mixin module** defines a collection of standard modules with a consistent common configuration.",
        relative_dir: "dnf/modules/mixin",
        prefix: "darkone.",
        slug: "mixin",
        icon: "&#x1F4E6;",
    },
    Category {
        title: "Service modules",
        description: "**A service module** provides a ready-to-use service (daemon).",
        relative_dir: "dnf/modules/service",
        prefix: "darkone.service.",
        slug: "service",
        icon: "&#x2728;",
    },
    Category {
        title: "System modules",
        description: "**A system module** manages common, important, and general configurations.",
        relative_dir: "dnf/modules/system",
        prefix: "darkone.system.",
        slug: "system",
        icon: "&#x2699;",
    },
    Category {
        title: "Security modules",
        description: "**A security module** hardens the system, its programs and functionalities.",
        relative_dir: "dnf/modules/security",
        prefix: "darkone.security.",
        slug: "security",
        icon: "&#x1F511;",
    },
    Category {
        title: "CLI applications",
        description: "**A CLI application module** installs terminal applications and their configuration optimized for DNF.",
        relative_dir: "dnf/modules/console",
        prefix: "darkone.console.",
        slug: "console",
        icon: "&#x1F4BB;",
    },
    Category {
        title: "GUI applications",
        description: "**A GUI application module** installs graphical applications and their configuration optimized for DNF.",
        relative_dir: "dnf/modules/graphic",
        prefix: "darkone.graphic.",
        slug: "graphic",
        icon: "&#x1F5BC;",
    },
    Category {
        title: "Administration modules",
        description: "**An administration module** deals with system-specific functionalities for administrators.",
        relative_dir: "dnf/modules/admin",
        prefix: "darkone.admin.",
        slug: "admin",
        icon: "&#x1F451;",
    },
    Category {
        title: "Users management modules",
        description: "**A users management module** manages user-related processing or special user settings.",
        relative_dir: "dnf/modules/user",
        prefix: "darkone.user.",
        slug: "user",
        icon: "&#x1F464;",
    },
    Category {
        title: "Home Manager modules",
        description: "**A home manager module** works with [home manager](https://github.com/nix-community/home-manager) profiles.",
        relative_dir: "dnf/home/modules",
        prefix: "darkone.home.",
        slug: "home",
        icon: "&#x1F3E0;",
    },
];

/// Options declared by one file of a module.
struct OptionBlock {
    /// Sub-module file, relative to the module's directory
    /// (`gateway/backup-links.nix`); `None` for the module file itself.
    file: Option<String>,
    declared: ModuleOptions,
}

struct ModuleEntry {
    /// Dotted option path derived from the file (`darkone.service.foo`).
    path: String,
    /// `path` without the category prefix (`foo`, `host.gateway`): menu label.
    label: String,
    /// Page name (`foo`, `host-gateway`), hence `/ref/modules/<category>/<slug>/`.
    slug: String,
    /// Module file, relative to the framework repository.
    source: String,
    comment: Option<String>,
    /// The module's own options first, then those of its sub-modules.
    blocks: Vec<OptionBlock>,
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

/// Build every reference page for `project_root`: the index, then per
/// non-empty category its index and one page per module. A category with no
/// module is skipped entirely (no page, no card) to avoid dead links.
///
/// Fails on a header linking a reference page that does not exist.
pub fn generate_pages(project_root: &Path) -> Result<Vec<Page>> {
    let collected: Vec<CollectedCategory> = CATEGORIES
        .iter()
        .map(|category| CollectedCategory {
            category,
            entries: collect_entries(project_root, category),
        })
        .filter(|c| !c.entries.is_empty())
        .collect();

    let mut pages = vec![Page {
        rel_path: format!("{DOC_DIR}/index.mdx"),
        content: localize_links(&render_index(&collected)),
    }];

    for (i, c) in collected.iter().enumerate() {
        let slug = c.category.slug;
        pages.push(Page {
            rel_path: format!("{DOC_DIR}/{slug}/index.mdx"),
            content: localize_links(&render_category_page(c, i + 2)),
        });
        for entry in &c.entries {
            pages.push(Page {
                rel_path: format!("{DOC_DIR}/{slug}/{}.mdx", entry.slug),
                content: localize_links(&render_module_page(entry, c.category.icon)),
            });
        }
    }

    check_reference_links(&pages)?;
    Ok(pages)
}

/// Every link into the reference must target a generated page: a module header
/// naming a renamed or removed module fails here, not at the site build.
fn check_reference_links(pages: &[Page]) -> Result<()> {
    let routes: BTreeSet<String> = pages
        .iter()
        .filter_map(|page| page.rel_path.strip_prefix(DOC_DIR))
        .map(|file| {
            let route = file.trim_start_matches('/').trim_end_matches(".mdx");
            let route = route.strip_suffix("index").unwrap_or(route);
            route.trim_end_matches('/').to_string()
        })
        .collect();

    let mut dead = vec![];
    for page in pages {
        for caps in REFERENCE_LINK.captures_iter(&page.content) {
            let target = caps[1].trim_end_matches('/');
            if !routes.contains(target) {
                dead.push(format!("  {}: /ref/modules/{}", page.rel_path, &caps[1]));
            }
        }
    }
    if dead.is_empty() {
        return Ok(());
    }
    Err(NixError::generate(format!(
        "links to a missing reference page (expected /ref/modules/<category>/<module>/):\n{}",
        dead.join("\n")
    )))
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

/// The modules of a category: one table row each, linking the module page.
fn render_category_page(c: &CollectedCategory, order: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "---\ntitle: {}\nsidebar:\n  order: {order}\n  label: Overview\n---\n\n",
        c.category.title
    ));
    out.push_str(&format!("{}\n\n", c.category.description));
    out.push_str("| Module | Role |\n|---|---|\n");
    for entry in &c.entries {
        let role = entry.comment.as_deref().map(summary).unwrap_or_default();
        out.push_str(&format!(
            "| [`{}`](/ref/modules/{}/{}/) | {} |\n",
            entry.label,
            c.category.slug,
            entry.slug,
            escape(&role).replace('|', "\\|"),
        ));
    }
    out
}

/// First paragraph of a header comment, on one line.
fn summary(comment: &str) -> String {
    comment
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ─── module page ─────────────────────────────────────────────────────────────

/// One module: header comment, options (its own, then per sub-module file),
/// link to the source. The frontmatter holds identifiers only, so that the
/// translation copies it as is; the category icon prefixes the title only, the
/// menu label stays bare.
fn render_module_page(entry: &ModuleEntry, icon: &str) -> String {
    let mut out = format!(
        "---\ntitle: \"{} {}\"\nsidebar:\n  label: \"{}\"\n---\n\n",
        entity_to_char(icon),
        entry.path,
        entry.label
    );
    if let Some(comment) = &entry.comment {
        out.push_str(&escape(&promote_headings(comment)));
        out.push_str("\n\n");
    }
    if !entry.blocks.is_empty() {
        out.push_str("## Options\n\n");
    }
    for block in &entry.blocks {
        if let Some(file) = &block.file {
            out.push_str(&format!("### `{file}`\n\n"));
        }

        // Declared path first: a sub-module extends its parent's namespace,
        // whatever its file name.
        let namespace = match block.declared.namespace.as_str() {
            "" => entry.path.as_str(),
            declared => declared,
        };
        out.push_str(&render_options(namespace, &block.declared.options));
    }
    out.push_str(&format!(
        "Source: [`{0}`]({SOURCE_URL}/{0})\n",
        entry.source
    ));
    out
}

/// Shift the headings of a header comment so that its top level is `##`: a
/// module header nests under a title, whichever level its author picked. Lines
/// of a fenced code block are left alone (`# shell comment`).
fn promote_headings(comment: &str) -> String {
    let level = |line: &str| -> Option<usize> {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes)
    };

    let mut in_fence = false;
    let mut headings = vec![];
    for (i, line) in comment.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence {
            if let Some(hashes) = level(line) {
                headings.push((i, hashes));
            }
        }
    }
    let Some(top) = headings.iter().map(|(_, hashes)| *hashes).min() else {
        return comment.to_string();
    };

    let mut out = vec![];
    let mut next = headings.iter().peekable();
    for (i, line) in comment.lines().enumerate() {
        match next.peek() {
            Some((at, hashes)) if *at == i => {
                let shifted = (hashes - top + 2).min(6);
                out.push(format!("{}{}", "#".repeat(shifted), &line[*hashes..]));
                next.next();
            }
            _ => out.push(line.to_string()),
        }
    }
    out.join("\n")
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Turn a numeric HTML entity (`&#x1F4E6;`) into its character so it renders in
/// a JSX attribute or the frontmatter (`title="📦 …"`). Non-entity input is returned unchanged.
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

/// Module file a file belongs to: itself, or for a sub-module
/// (`<name>/<topic>.nix`) the `<name>.nix` next to its directory.
fn owner_of(file: &Path) -> PathBuf {
    let mut owner = file.to_path_buf();
    while let Some(parent) = owner.parent().map(|dir| dir.with_extension("nix")) {
        if !parent.is_file() {
            break;
        }
        owner = parent;
    }
    owner
}

/// Modules of a category, sorted by page name. Sub-module files only
/// contribute their options, to their owner.
fn collect_entries(project_root: &Path, category: &Category) -> Vec<ModuleEntry> {
    let dir = project_root.join(category.relative_dir);
    let mut sub_modules: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut modules = vec![];
    for file in extract_nix_files(&dir) {
        let owner = owner_of(&file);
        if owner == file {
            modules.push(file);
        } else {
            sub_modules.entry(owner).or_default().push(file);
        }
    }

    let declared_by = |file: &Path| -> Option<ModuleOptions> {
        let declared = parse_module(&std::fs::read_to_string(file).ok()?);
        (!declared.options.is_empty()).then_some(declared)
    };

    let mut entries = vec![];
    for file in modules {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let path = extract_module_path(&file, &dir, category.prefix);
        let label = path
            .strip_prefix(category.prefix)
            .unwrap_or(&path)
            .to_string();
        let relative = file.strip_prefix(&dir).unwrap_or(&file);

        let mut blocks: Vec<OptionBlock> = declared_by(&file)
            .map(|declared| OptionBlock {
                file: None,
                declared,
            })
            .into_iter()
            .collect();
        for sub_module in sub_modules.get(&file).into_iter().flatten() {
            let Some(declared) = declared_by(sub_module) else {
                continue;
            };
            let name = sub_module
                .strip_prefix(file.parent().unwrap_or(&dir))
                .unwrap_or(sub_module);
            blocks.push(OptionBlock {
                file: Some(name.to_string_lossy().into_owned()),
                declared,
            });
        }

        entries.push(ModuleEntry {
            slug: label.replace('.', "-"),
            label,
            path,
            source: format!(
                "{}/{}",
                category.relative_dir.trim_start_matches(FRAMEWORK_DIR),
                relative.to_string_lossy()
            ),
            comment: extract_first_comment(&source),
            blocks,
        });
    }
    entries.sort_by(|a, b| a.slug.cmp(&b.slug));
    entries
}

/// Bullets describing each option, then a Nix block setting them all under
/// `namespace`, the dotted path they are declared at.
fn render_options(namespace: &str, options: &[NixOption]) -> String {
    if options.is_empty() {
        return String::new();
    }
    let mut bullets = String::new();
    let mut code = String::new();
    let opt_count = options.len();
    let multi = opt_count > 1;

    code.push_str("\n```nix\n");
    if multi {
        code.push_str(&format!("{namespace} = {{\n"));
    }

    let prefix = if multi {
        "  ".to_string()
    } else {
        format!("{namespace}.")
    };

    // Track names by level so we can rebuild dotted paths inside opened submodules.
    let mut opened_levels: Vec<String> = vec![];
    let mut last_level = 1usize;
    let mut names_by_level: Vec<String> = vec![String::new()];

    for (i, opt) in options.iter().enumerate() {
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
        let next_is_deeper = options
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

    const REF: &str = "doc/src/content/docs/en/ref/modules";

    /// Write `source` to `<root>/<rel>`, creating the directories.
    fn write(root: &Path, rel: &str, source: &str) {
        let file = root.join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, source).unwrap();
    }

    fn page<'a>(pages: &'a [Page], rel_path: &str) -> &'a str {
        &pages
            .iter()
            .find(|p| p.rel_path == format!("{REF}/{rel_path}"))
            .unwrap_or_else(|| panic!("missing page {rel_path}"))
            .content
    }

    #[test]
    fn renders_module_page() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/security/foo.nix",
            r#"# A foo service.
{ lib, ... }: {
  options = {
    darkone.security.foo.enable = lib.mkEnableOption "Enable foo";
  };
}
"#,
        );

        let pages = generate_pages(dir.path()).unwrap();
        let mdx = page(&pages, "security/foo.mdx");
        assert!(
            mdx.starts_with("---\ntitle: \"🔑 darkone.security.foo\"\nsidebar:\n  label: \"foo\"\n---\n")
        );
        assert!(mdx.contains("A foo service."));
        assert!(mdx.contains("## Options\n\n* **enable** `bool` Enable foo"));
        assert!(mdx.contains("darkone.security.foo.enable = false;"));
        assert!(mdx.contains(
            "Source: [`modules/security/foo.nix`](https://github.com/darkone-linux/darkone-nixos-framework/blob/main/modules/security/foo.nix)"
        ));
    }

    #[test]
    fn renders_one_line_module() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/security/foo.nix",
            r#"# A foo service.
{ lib, ... }: {
  options.darkone.security.foo.enable = lib.mkEnableOption "Enable foo";
}
"#,
        );

        let pages = generate_pages(dir.path()).unwrap();
        let mdx = page(&pages, "security/foo.mdx");
        assert!(mdx.contains("* **enable** `bool` Enable foo"));
        assert!(mdx.contains("darkone.security.foo.enable = false;"));
    }

    #[test]
    fn module_without_options_has_no_options_section() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/system/bar.nix",
            "# Bar.\n{ ... }: { }\n",
        );

        let pages = generate_pages(dir.path()).unwrap();
        let mdx = page(&pages, "system/bar.mdx");
        assert!(!mdx.contains("## Options"));
        assert!(mdx.contains("Bar.\n\nSource: [`modules/system/bar.nix`]"));
    }

    #[test]
    fn index_and_category_pages_link_down() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/service/foo.nix",
            r#"# A foo service, on
# two lines.
#
# Details.
{ lib, ... }: {
  options.darkone.service.foo.enable = lib.mkEnableOption "Enable foo";
}
"#,
        );

        let pages = generate_pages(dir.path()).unwrap();

        // Index: one card per non-empty category, no module documentation.
        let index = page(&pages, "index.mdx");
        assert!(index.contains("import { CardGrid, LinkCard }"));
        assert!(index.contains("href=\"/en/ref/modules/service/\""));
        assert!(!index.contains("## "));

        // Category: one row per module, its first paragraph as role.
        let category = page(&pages, "service/index.mdx");
        assert!(category.contains("title: Service modules"));
        assert!(category
            .contains("| [`foo`](/en/ref/modules/service/foo/) | A foo service, on two lines. |"));
        assert!(!category.contains("Details."));

        // Index, category, module: nothing for the empty categories.
        assert_eq!(pages.len(), 3);
    }

    #[test]
    fn mixin_page_is_named_after_its_option_path() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/mixin/host/gateway.nix",
            r#"# The gateway.
{ lib, ... }: {
  options.darkone.host.gateway.enable = lib.mkEnableOption "Enable";
}
"#,
        );

        let pages = generate_pages(dir.path()).unwrap();
        let mdx = page(&pages, "mixin/host-gateway.mdx");
        assert!(mdx.contains("title: \"📦 darkone.host.gateway\"\n"));
        assert!(mdx.contains("label: \"host.gateway\""));
        assert!(mdx.contains("blob/main/modules/mixin/host/gateway.nix"));
        assert!(page(&pages, "mixin/index.mdx")
            .contains("[`host.gateway`](/en/ref/modules/mixin/host-gateway/)"));
    }

    #[test]
    fn sub_module_options_land_on_the_parent_page() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/mixin/host/gateway.nix",
            r#"# The gateway.
{ lib, ... }: {
  options.darkone.host.gateway.enable = lib.mkEnableOption "Enable";
}
"#,
        );
        write(
            dir.path(),
            "dnf/modules/mixin/host/gateway/backup-links.nix",
            r#"# DNF gateway: backup uplinks. Doc: `../gateway.nix` header.
{ lib, ... }: {
  options.darkone.host.gateway.linkCheck = {
    interval = lib.mkOption {
      type = lib.types.ints.positive;
      default = 10;
      description = "Seconds between two probe rounds.";
    };
    failAfter = lib.mkOption {
      type = lib.types.ints.positive;
      default = 3;
      description = "Failed rounds.";
    };
  };
}
"#,
        );
        write(
            dir.path(),
            "dnf/modules/mixin/host/gateway/silent.nix",
            "# DNF gateway: no option.\n{ ... }: { }\n",
        );

        let pages = generate_pages(dir.path()).unwrap();

        // No page of their own, no row in the category.
        assert_eq!(pages.len(), 3);
        assert!(!page(&pages, "mixin/index.mdx").contains("backup-links"));

        let mdx = page(&pages, "mixin/host-gateway.mdx");
        assert!(mdx.contains("### `gateway/backup-links.nix`"));
        assert!(mdx.contains("* **interval** `ints.positive` Seconds between two probe rounds."));

        // Under the namespace the options are declared at, not the file path.
        assert!(mdx.contains("darkone.host.gateway.linkCheck = {\n  interval = 10;"));
        assert!(!mdx.contains("darkone.host.gateway.backup-links"));
        assert!(!mdx.contains("silent"));
    }

    #[test]
    fn promotes_header_headings_to_page_sections() {
        let comment = "Intro.\n\n#### First\n\n```sh\n# not a heading\n```\n\n##### Deeper";
        assert_eq!(
            promote_headings(comment),
            "Intro.\n\n## First\n\n```sh\n# not a heading\n```\n\n### Deeper"
        );
        assert_eq!(promote_headings("No heading."), "No heading.");
    }

    #[test]
    fn localizes_links_to_other_modules() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/service/dnsmasq.nix",
            "# DNS and DHCP.\n{ ... }: { }\n",
        );
        write(
            dir.path(),
            "dnf/modules/service/adguardhome.nix",
            r#"# Uses the [dnsmasq module](/ref/modules/service/dnsmasq/) as upstream, see
# [the fr page](/fr/doc/x/) and [home](https://example.com).
{ ... }: { }
"#,
        );

        let pages = generate_pages(dir.path()).unwrap();
        let mdx = page(&pages, "service/adguardhome.mdx");
        assert!(mdx.contains("](/en/ref/modules/service/dnsmasq/)"));

        // Already-localized and external links are left untouched.
        assert!(mdx.contains("](/fr/doc/x/)"));
        assert!(mdx.contains("](https://example.com)"));
        assert!(!mdx.contains("](/ref/"));
    }

    #[test]
    fn refuses_a_link_to_a_missing_reference_page() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/service/adguardhome.nix",
            "# Uses the [dnsmasq module](/ref/modules/#-darkoneservicednsmasq).\n{ ... }: { }\n",
        );
        write(
            dir.path(),
            "dnf/modules/service/foo.nix",
            "# See [bar](/ref/modules/service/bar/).\n{ ... }: { }\n",
        );

        let error = generate_pages(dir.path()).err().unwrap().to_string();
        assert!(error.contains("service/foo.mdx: /ref/modules/service/bar/"));

        // The reference root exists: an anchor on it is the site build's to check.
        assert!(!error.contains("adguardhome"));
    }

    #[test]
    fn renders_submodule_block() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "dnf/modules/system/services.nix",
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
        );

        let pages = generate_pages(dir.path()).unwrap();
        let mdx = page(&pages, "system/services.mdx");
        assert!(mdx.contains("title: \"⚙ darkone.system.services\"\n"));
        assert!(mdx.contains("* **service** `attrs` Services"));
        assert!(mdx.contains("  * **enable** `bool` Enable proxy"));
        assert!(mdx.contains("  * **persist.dirs** `listOf str` Dirs"));
        // Inside multi-option, nested code uses parent dotted prefix:
        assert!(mdx.contains("service.enable = false;"));
        assert!(mdx.contains("service.persist.dirs = [ ];"));
    }
}
