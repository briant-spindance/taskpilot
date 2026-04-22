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
