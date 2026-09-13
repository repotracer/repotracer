//! Bundled benchmark scripts for installed binaries, independent of a checkout.
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

const FILES: &[(&str, &str)] = &[
    (
        "workflow.py",
        include_str!("../../../tools/benchmarks/workflow.py"),
    ),
    (
        "native.py",
        include_str!("../../../tools/benchmarks/native.py"),
    ),
    (
        "usage_tap.py",
        include_str!("../../../tools/benchmarks/usage_tap.py"),
    ),
    (
        "bench.py",
        include_str!("../../../tools/benchmarks/bench.py"),
    ),
    (
        "dataset.py",
        include_str!("../../../tools/benchmarks/dataset.py"),
    ),
    ("routing.txt", crate::agents::ROUTING_INSTRUCTIONS),
];

pub fn install() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .context("no user data directory for benchmark scripts")?
        .join("repotracer")
        .join("benchmark-engine");
    install_at(&base)
}

fn install_at(base: &Path) -> Result<PathBuf> {
    let mut hash = Sha256::new();
    for (name, body) in FILES {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(body.as_bytes());
        hash.update([0]);
    }
    // Existing jobs retain the exact scripts they started with after an update.
    let digest: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let directory = base.join(digest);
    std::fs::create_dir_all(&directory)?;
    for (name, body) in FILES {
        let target = directory.join(name);
        if target.is_file() {
            anyhow::ensure!(
                std::fs::read(&target)? == body.as_bytes(),
                "bundled benchmark script was modified: {}",
                target.display()
            );
            continue;
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
        temporary.write_all(body.as_bytes())?;
        temporary.flush()?;
        if let Err(error) = temporary.persist_noclobber(&target) {
            if !target.is_file() || std::fs::read(&target)? != body.as_bytes() {
                return Err(error.error).context("write bundled benchmark script");
            }
        }
    }
    Ok(directory.join("workflow.py"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_engine_is_complete_and_reusable_without_a_checkout() {
        let root = tempfile::tempdir().unwrap();
        let script = install_at(root.path()).unwrap();
        assert_eq!(install_at(root.path()).unwrap(), script);
        for (name, content) in FILES {
            assert_eq!(
                std::fs::read_to_string(script.parent().unwrap().join(name)).unwrap(),
                *content
            );
        }
    }
}
