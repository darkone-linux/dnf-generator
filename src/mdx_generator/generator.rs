use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::mdx_generator::module::{generate_pages, DOC_DIR};

pub fn generate(project_root: &Path, display: bool) -> Result<String> {
    let pages = generate_pages(project_root)?;
    if display {
        return Ok(pages
            .iter()
            .map(|p| format!("// {}\n{}", p.rel_path, p.content))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let mut written = BTreeSet::new();
    for page in &pages {
        let target = project_root.join(&page.rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &page.content)?;
        written.insert(target);
    }
    prune(&project_root.join(DOC_DIR), &written)?;
    Ok(String::new())
}

/// Remove the pages of `dir` this run did not write, then the directories left
/// empty: a renamed or removed module must not keep its page. Returns whether
/// `dir` itself ends up empty.
fn prune(dir: &Path, written: &BTreeSet<PathBuf>) -> Result<bool> {
    let mut empty = true;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if prune(&path, written)? {
                std::fs::remove_dir(&path)?;
            } else {
                empty = false;
            }
        } else if path.extension().is_some_and(|ext| ext == "mdx") && !written.contains(&path) {
            std::fs::remove_file(&path)?;
        } else {
            empty = false;
        }
    }
    Ok(empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn removes_the_pages_it_no_longer_produces() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let module = root.join("dnf/modules/service/foo.nix");
        fs::create_dir_all(module.parent().unwrap()).unwrap();
        fs::write(&module, "# Foo.\n{ ... }: { }\n").unwrap();

        // Leftovers: the former category page, a module since removed, a
        // category since emptied, and a file that is not a page.
        let reference = root.join(DOC_DIR);
        fs::create_dir_all(reference.join("service")).unwrap();
        fs::create_dir_all(reference.join("user")).unwrap();
        fs::write(reference.join("service.mdx"), "old").unwrap();
        fs::write(reference.join("service/gone.mdx"), "old").unwrap();
        fs::write(reference.join("user/index.mdx"), "old").unwrap();
        fs::write(reference.join("notes.txt"), "kept").unwrap();

        generate(root, false).unwrap();

        assert!(reference.join("index.mdx").is_file());
        assert!(reference.join("service/index.mdx").is_file());
        assert!(reference.join("service/foo.mdx").is_file());
        assert!(!reference.join("service.mdx").exists());
        assert!(!reference.join("service/gone.mdx").exists());
        assert!(!reference.join("user").exists());
        assert!(reference.join("notes.txt").is_file());
    }
}
