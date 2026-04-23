use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct Workspace {
    _temp: TempDir,
    pub dir: PathBuf,
}

impl Workspace {
    /// Create a new temporary workspace.
    pub fn new() -> Result<Self> {
        let temp = TempDir::new().context("create temp dir")?;
        let dir = temp.path().to_path_buf();
        Ok(Workspace { _temp: temp, dir })
    }

    /// Copy input files into the workspace, preserving filenames.
    pub fn stage_inputs(&self, inputs: &[String]) -> Result<()> {
        for src in inputs {
            let src_path = Path::new(src);
            let filename = src_path
                .file_name()
                .with_context(|| format!("no filename in {src}"))?;
            let dst = self.dir.join(filename);
            fs::copy(src_path, &dst)
                .with_context(|| format!("stage {src}"))?;
        }
        Ok(())
    }

    /// Copy all files from the workspace to the output directory.
    pub fn collect_outputs(&self, output_dir: &str) -> Result<()> {
        let out = Path::new(output_dir);
        fs::create_dir_all(out).context("create output dir")?;

        for entry in fs::read_dir(&self.dir)?.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let dst = out.join(entry.file_name());
                fs::copy(entry.path(), &dst)
                    .with_context(|| format!("collect {}", entry.file_name().to_string_lossy()))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn new_creates_dir() {
        let ws = Workspace::new().unwrap();
        assert!(ws.dir.exists());
        assert!(ws.dir.is_dir());
    }

    #[test]
    fn stage_single_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("hello.txt");
        fs::write(&src, "hello").unwrap();

        let ws = Workspace::new().unwrap();
        ws.stage_inputs(&[src.to_str().unwrap().to_string()]).unwrap();

        let staged = ws.dir.join("hello.txt");
        assert!(staged.exists());
        assert_eq!(fs::read_to_string(staged).unwrap(), "hello");
    }

    #[test]
    fn stage_multiple_files() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, "aaa").unwrap();
        fs::write(&b, "bbb").unwrap();

        let ws = Workspace::new().unwrap();
        ws.stage_inputs(&[
            a.to_str().unwrap().to_string(),
            b.to_str().unwrap().to_string(),
        ])
        .unwrap();

        assert_eq!(fs::read_to_string(ws.dir.join("a.txt")).unwrap(), "aaa");
        assert_eq!(fs::read_to_string(ws.dir.join("b.txt")).unwrap(), "bbb");
    }

    #[test]
    fn stage_missing_file_errors() {
        let ws = Workspace::new().unwrap();
        let err = ws.stage_inputs(&["/nonexistent/file.txt".to_string()]);
        assert!(err.is_err());
    }

    #[test]
    fn stage_no_filename_errors() {
        let ws = Workspace::new().unwrap();
        let err = ws.stage_inputs(&["/".to_string()]);
        assert!(err.is_err());
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("no filename"), "got: {msg}");
    }

    #[test]
    fn collect_outputs_copies_files() {
        let ws = Workspace::new().unwrap();
        fs::write(ws.dir.join("out.txt"), "data").unwrap();

        let out_tmp = TempDir::new().unwrap();
        let out_dir = out_tmp.path().join("results");
        ws.collect_outputs(out_dir.to_str().unwrap()).unwrap();

        assert_eq!(fs::read_to_string(out_dir.join("out.txt")).unwrap(), "data");
    }

    #[test]
    fn collect_outputs_creates_output_dir() {
        let ws = Workspace::new().unwrap();
        let out_tmp = TempDir::new().unwrap();
        let out_dir = out_tmp.path().join("deep").join("nested");
        ws.collect_outputs(out_dir.to_str().unwrap()).unwrap();
        assert!(out_dir.exists());
    }

    #[test]
    fn collect_outputs_skips_subdirs() {
        let ws = Workspace::new().unwrap();
        fs::write(ws.dir.join("file.txt"), "yes").unwrap();
        fs::create_dir(ws.dir.join("subdir")).unwrap();

        let out_tmp = TempDir::new().unwrap();
        let out_dir = out_tmp.path().join("out");
        ws.collect_outputs(out_dir.to_str().unwrap()).unwrap();

        assert!(out_dir.join("file.txt").exists());
        assert!(!out_dir.join("subdir").exists());
    }

    #[test]
    fn collect_outputs_empty_workspace() {
        let ws = Workspace::new().unwrap();
        let out_tmp = TempDir::new().unwrap();
        let out_dir = out_tmp.path().join("empty_out");
        ws.collect_outputs(out_dir.to_str().unwrap()).unwrap();

        assert!(out_dir.exists());
        let entries: Vec<_> = fs::read_dir(&out_dir).unwrap().collect();
        assert!(entries.is_empty());
    }
}
