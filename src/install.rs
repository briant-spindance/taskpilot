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

pub(crate) fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: set HOME to a temp dir and return it (keeps it alive).
    fn redirect_home() -> TempDir {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());
        tmp
    }

    // ── from_local ──────────────────────────────────────────────

    #[test]
    #[serial]
    fn from_local_valid_skill() {
        let home = redirect_home();
        let src = TempDir::new().unwrap();
        fs::write(src.path().join("SKILL.md"), "# My Skill").unwrap();
        fs::write(src.path().join("extra.txt"), "data").unwrap();

        from_local(src.path().to_str().unwrap()).unwrap();

        let name = src.path().file_name().unwrap().to_string_lossy();
        let installed = home.path().join(".agents/skills").join(name.as_ref());
        assert!(installed.join("SKILL.md").exists());
        assert!(installed.join("extra.txt").exists());
    }

    #[test]
    #[serial]
    fn from_local_no_skill_md() {
        let _home = redirect_home();
        let src = TempDir::new().unwrap();

        let err = from_local(src.path().to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("no SKILL.md"),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[serial]
    fn from_local_invalid_source_path() {
        let _home = redirect_home();
        // "/" has no file_name(), but the SKILL.md check comes first and
        // "/" won't contain SKILL.md, so we get that error instead.
        let err = from_local("/").unwrap_err();
        assert!(
            err.to_string().contains("no SKILL.md"),
            "unexpected error: {err}"
        );
    }

    // ── copy_dir ────────────────────────────────────────────────

    #[test]
    fn copy_dir_flat_files() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        let dst_target = dst.path().join("out");

        fs::write(src.path().join("a.txt"), "aaa").unwrap();
        fs::write(src.path().join("b.txt"), "bbb").unwrap();

        copy_dir(src.path(), &dst_target).unwrap();

        assert_eq!(fs::read_to_string(dst_target.join("a.txt")).unwrap(), "aaa");
        assert_eq!(fs::read_to_string(dst_target.join("b.txt")).unwrap(), "bbb");
    }

    #[test]
    fn copy_dir_nested() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        let dst_target = dst.path().join("out");

        let sub = src.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.txt"), "deep").unwrap();
        fs::write(src.path().join("top.txt"), "top").unwrap();

        copy_dir(src.path(), &dst_target).unwrap();

        assert_eq!(
            fs::read_to_string(dst_target.join("sub/nested.txt")).unwrap(),
            "deep"
        );
        assert_eq!(
            fs::read_to_string(dst_target.join("top.txt")).unwrap(),
            "top"
        );
    }

    #[test]
    fn copy_dir_empty() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        let dst_target = dst.path().join("out");

        copy_dir(src.path(), &dst_target).unwrap();

        assert!(dst_target.exists());
        assert!(fs::read_dir(&dst_target).unwrap().next().is_none());
    }
}
