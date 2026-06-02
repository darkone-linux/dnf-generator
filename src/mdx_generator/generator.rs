use std::path::Path;

use crate::error::Result;
use crate::mdx_generator::module::generate_pages;

pub fn generate(project_root: &Path, display: bool) -> Result<String> {
    let pages = generate_pages(project_root);
    if display {
        return Ok(pages
            .iter()
            .map(|p| format!("// {}\n{}", p.rel_path, p.content))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    for page in &pages {
        let target = project_root.join(&page.rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &page.content)?;
    }
    Ok(String::new())
}
