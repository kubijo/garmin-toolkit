//! Content-addressed, complete bindgen outputs, before Trunk's asset processing.

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

pub struct Cache {
    entry: PathBuf,
    work: Option<TempDir>,
}

impl Cache {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        target: &Path,
        wasm: &Path,
        executable: &Path,
        version: &str,
        args: &[&str],
        cwd: &Path,
        packages: impl Iterator<Item = PathBuf>,
    ) -> Result<Self> {
        let mut hash = Sha256::new();
        hash.update(b"trunk-bindgen-cache-v1");
        hash.update(serde_json::to_vec(&(version, args, cwd, executable))?);
        hash.update(file_digest(wasm)?);
        hash.update(file_digest(executable)?);
        let mut environment = std::env::vars_os()
            .filter(|(key, _)| key.to_string_lossy().starts_with("WASM_BINDGEN_"))
            .collect::<Vec<_>>();
        environment.sort();
        // Preserve non-UTF8 environment values without lossy conversion.
        for (key, value) in environment {
            for bytes in [key.as_encoded_bytes(), value.as_encoded_bytes()] {
                hash.update(bytes.len().to_le_bytes());
                hash.update(bytes);
            }
        }
        let mut packages = packages.collect::<Vec<_>>();
        packages.sort();
        packages.dedup();
        for package in packages {
            hash.update(serde_json::to_vec(&package)?);
            match file_digest(&package) {
                Ok(digest) => {
                    hash.update([1]);
                    hash.update(digest);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => hash.update([0]),
                Err(error) => return Err(error.into()),
            }
        }
        Self::open(target, &format!("{:x}", hash.finalize()))
    }

    fn open(target: &Path, key: &str) -> Result<Self> {
        let root = target.join("trunk/bindgen-cache/v1");
        fs::create_dir_all(&root)?;
        let entry = root.join(key);
        let valid = fs::read(entry.join("complete.sha256"))
            .ok()
            .zip(tree_digest(&entry.join("out")).ok())
            .is_some_and(|(expected, actual)| expected == actual);
        let work = if valid {
            None
        } else {
            let work = tempfile::Builder::new()
                .prefix("pending-")
                .tempdir_in(root)?;
            fs::create_dir(work.path().join("out"))?;
            Some(work)
        };
        Ok(Self { entry, work })
    }

    pub fn hit(&self) -> bool {
        self.work.is_none()
    }

    pub fn output(&self) -> PathBuf {
        self.work
            .as_ref()
            .map_or(self.entry.as_path(), |work| work.path())
            .join("out")
    }

    pub fn publish(&self) -> Result<PathBuf> {
        let Some(work) = &self.work else {
            return Ok(self.output());
        };
        let digest = tree_digest(&self.output())?;
        fs::write(work.path().join("complete.sha256"), digest)?;
        // Only a completed directory is published. A concurrent writer wins
        // without exposing our partial output; this invocation can use its own.
        match fs::rename(work.path(), &self.entry) {
            Ok(()) => Ok(self.entry.join("out")),
            Err(_) if self.entry.exists() => Ok(self.output()),
            Err(error) => Err(error).context("publishing bindgen cache"),
        }
    }
}

fn file_digest(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(hash.finalize().into());
        }
        hash.update(&buffer[..count]);
    }
}

fn tree_digest(root: &Path) -> Result<Vec<u8>> {
    fn visit(path: &Path, root: &Path, hash: &mut Sha256) -> Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type()?;
            ensure!(
                kind.is_dir() || kind.is_file(),
                "non-regular bindgen cache entry"
            );
            hash.update(serde_json::to_vec(path.strip_prefix(root)?)?);
            hash.update([u8::from(kind.is_dir())]);
            if kind.is_dir() {
                visit(&path, root, hash)?;
            } else {
                hash.update(file_digest(&path)?);
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    visit(root, root, &mut hash)?;
    Ok(hash.finalize().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_complete_tree_and_rejects_corruption() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cache = Cache::open(root.path(), "key")?;
        assert!(!cache.hit());
        fs::create_dir_all(cache.output().join("snippets/crate"))?;
        for file in [
            "app.js",
            "app_bg.wasm",
            "app.d.ts",
            "package.json",
            "snippets/crate/local.js",
        ] {
            fs::write(cache.output().join(file), file)?;
        }
        let output = cache.publish()?;
        assert!(Cache::open(root.path(), "key")?.hit());
        assert_eq!(
            fs::read_to_string(output.join("snippets/crate/local.js"))?,
            "snippets/crate/local.js"
        );
        fs::write(output.join("app.js"), "corrupt")?;
        assert!(!Cache::open(root.path(), "key")?.hit());
        Ok(())
    }

    #[test]
    fn failed_and_concurrent_generators_never_publish_partial_output() -> Result<()> {
        let root = tempfile::tempdir()?;
        let first = Cache::open(root.path(), "key")?;
        fs::write(first.output().join("partial"), "incomplete")?;
        let second = Cache::open(root.path(), "key")?;
        assert!(!second.hit());
        fs::write(second.output().join("complete"), "finished")?;
        let published = second.publish()?;
        let private = first.publish()?;
        assert_ne!(published, private);
        assert!(
            Cache::open(root.path(), "key")?
                .output()
                .join("complete")
                .exists()
        );
        let abandoned = private.parent().unwrap().to_owned();
        drop(first);
        assert!(!abandoned.exists());
        Ok(())
    }

    #[test]
    fn invalidates_on_contents_options_tool_and_package_metadata() -> Result<()> {
        let root = tempfile::tempdir()?;
        let wasm = root.path().join("app.wasm");
        let tool = root.path().join("bindgen");
        let package = root.path().join("package.json");
        fs::write(&wasm, "wasm including local JS")?;
        fs::write(&tool, "executable")?;
        let prepare = |version, args| {
            Cache::prepare(
                root.path(),
                &wasm,
                &tool,
                version,
                args,
                root.path(),
                [package.clone()].into_iter(),
            )
        };
        let original = prepare("0.2.126", &["--target=web"])?;
        fs::write(original.output().join("app.js"), "generated")?;
        original.publish()?;
        assert!(prepare("0.2.126", &["--target=web"])?.hit());
        assert!(!prepare("0.2.127", &["--target=web"])?.hit());
        assert!(!prepare("0.2.126", &["--target=web", "--no-demangle"])?.hit());
        fs::write(&package, "{}")?;
        assert!(!prepare("0.2.126", &["--target=web"])?.hit());
        fs::remove_file(&package)?;
        fs::write(&tool, "changed executable")?;
        assert!(!prepare("0.2.126", &["--target=web"])?.hit());
        fs::write(&tool, "executable")?;
        fs::write(&wasm, "wasm including changed local JS")?;
        assert!(!prepare("0.2.126", &["--target=web"])?.hit());
        Ok(())
    }
}
