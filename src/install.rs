use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Install a skill from a local directory into ~/.agents/skills/<name>.
pub fn from_local(src_dir: &str) -> Result<()> {
    let src = Path::new(src_dir);
    if !src.join("SKILL.md").exists() {
        anyhow::bail!("source is not a valid skill (no SKILL.md)");
    }

    let name = src
        .file_name()
        .context("invalid source path")?
        .to_string_lossy();

    let home = std::env::var("HOME").context("resolve home")?;
    let dest = Path::new(&home)
        .join(".agents")
        .join("skills")
        .join(name.as_ref());

    // Remove existing
    let _ = fs::remove_dir_all(&dest);

    copy_dir(src, &dest).context("copy skill")?;

    eprintln!("Installed skill {name:?} to {}", dest.display());
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
