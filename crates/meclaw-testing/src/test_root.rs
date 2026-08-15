//! RAII wrapper around a temporary `{root}` directory.
//!
//! Drop cleans the directory. This is the only sanctioned exception to
//! meclaw's no-delete policy — tmp roots are not part of any live tree.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct TestRoot {
    dir: TempDir,
}

impl TestRoot {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            dir: tempfile::tempdir()?,
        })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn join<P: AsRef<Path>>(&self, p: P) -> PathBuf {
        self.dir.path().join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_exists_until_drop() {
        let saved_path: PathBuf;
        {
            let tr = TestRoot::new().unwrap();
            saved_path = tr.path().to_path_buf();
            assert!(saved_path.exists());
            std::fs::write(tr.join("marker"), b"x").unwrap();
        }
        assert!(!saved_path.exists(), "tmp dir must be cleaned up on drop");
    }
}
