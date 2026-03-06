// SPDX-License-Identifier: Apache-2.0
//! Docker utilities for RBFT testnet

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const DEFAULT_REGISTRY: &str = "ghcr.io/raylsnetwork";

fn registry() -> String {
    std::env::var("RBFT_REGISTRY").unwrap_or_else(|_| DEFAULT_REGISTRY.to_string())
}
const IMAGE_NAME: &str = "rbft-node";
const IMAGE_TAG_PREFIX: &str = "testnet";

const SOURCE_DIGEST_FILE: &str = ".last_docker_image_digest";

pub struct PreparedImage {
    pub registry_image: String,
    pub tag: String,
    pub digest: String,
}

/// Stop and remove any running testnet Docker containers
pub fn cleanup_containers() {
    eprintln!("Cleaning up any running testnet containers...");
    let output = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", "name=rbft-node-testnet"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let container_ids = String::from_utf8_lossy(&output.stdout);
            let ids: Vec<&str> = container_ids.lines().collect();

            if !ids.is_empty() {
                eprintln!("Found {} testnet container(s) to remove", ids.len());
                let rm_output = std::process::Command::new("docker")
                    .arg("rm")
                    .arg("-f")
                    .args(&ids)
                    .output();

                if let Ok(rm_output) = rm_output {
                    if rm_output.status.success() {
                        eprintln!("✓ Removed {} testnet container(s)", ids.len());
                    } else {
                        eprintln!("⚠️  Warning: Failed to remove some containers");
                    }
                }
            }
        }
    }
}

/// Use Docker to clean up root-owned files in a directory
pub fn cleanup_directory(path: &std::path::Path) -> eyre::Result<()> {
    eprintln!(
        "Using Docker to clean up root-owned files in {}",
        path.display()
    );
    let output = std::process::Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!(
                "{}:/cleanup",
                path.to_str().expect("cleanup path contains invalid UTF-8")
            ),
            "alpine",
            "sh",
            "-c",
            "rm -rf /cleanup/*",
        ])
        .output()?;

    if !output.status.success() {
        return Err(eyre::eyre!(
            "Docker cleanup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Now remove the directory itself (should be empty now)
    fs::remove_dir_all(path)?;
    Ok(())
}

pub fn prepare_kube_image(assets_dir: &Path) -> eyre::Result<PreparedImage> {
    let digest = compute_source_digest()?;
    let short_digest: String = digest.chars().take(12).collect();
    let tag = format!("{IMAGE_TAG_PREFIX}-{short_digest}");
    let local_image = format!("{IMAGE_NAME}:{tag}");
    let registry_image = format!("{}/{IMAGE_NAME}:{tag}", registry());
    let state_path = assets_dir.join(SOURCE_DIGEST_FILE);

    let last_digest = fs::read_to_string(&state_path)
        .ok()
        .map(|contents| contents.trim().to_string());
    let force_rebuild = std::env::var("RBFT_FORCE_DOCKER_REBUILD")
        .ok()
        .is_some_and(|v| v != "0");

    if force_rebuild || last_digest.as_deref() != Some(digest.as_str()) {
        eprintln!(
            "Source changes detected. Building and pushing Docker image {}...",
            tag
        );
        build_image(&local_image)?;
        push_image_to_registry(&local_image, &registry_image)?;
        fs::write(&state_path, &digest)?;
    } else {
        eprintln!(
            "No source changes since last build (digest {}). Reusing {}.",
            short_digest, tag
        );
    }

    Ok(PreparedImage {
        registry_image,
        tag,
        digest,
    })
}

/// Build a Docker image for testnet using the repo Dockerfile.
pub fn build_image(image: &str) -> eyre::Result<()> {
    eprintln!("Building Docker image {} for testnet...", image);
    let root = git_root().unwrap_or(std::env::current_dir()?);
    let dockerfile_path = root.join("Dockerfile");
    let dockerfile = dockerfile_path
        .to_str()
        .ok_or_else(|| eyre::eyre!("Dockerfile path is not valid UTF-8"))?;
    if !dockerfile_path.exists() {
        return Err(eyre::eyre!(
            "Dockerfile not found at {}",
            dockerfile_path.display()
        ));
    }
    let context = root
        .to_str()
        .ok_or_else(|| eyre::eyre!("Repo root path is not valid UTF-8"))?;

    let output = std::process::Command::new("docker")
        .args(["build", "-t", image, "-f", dockerfile, context])
        .output()?;

    if !output.status.success() {
        return Err(eyre::eyre!(
            "Docker build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    eprintln!("✓ Docker image {} built successfully", image);
    Ok(())
}

/// Push Docker image to DigitalOcean registry
pub fn push_image_to_registry(local_image: &str, registry_image: &str) -> eyre::Result<()> {
    eprintln!("Tagging image for registry...");
    let tag_output = std::process::Command::new("docker")
        .args(["tag", local_image, registry_image])
        .output()?;

    if !tag_output.status.success() {
        return Err(eyre::eyre!(
            "Failed to tag image: {}",
            String::from_utf8_lossy(&tag_output.stderr)
        ));
    }

    eprintln!("Pushing image to DigitalOcean registry...");
    let push_output = std::process::Command::new("docker")
        .args(["push", registry_image])
        .output()?;

    if !push_output.status.success() {
        return Err(eyre::eyre!(
            "Failed to push image: {}",
            String::from_utf8_lossy(&push_output.stderr)
        ));
    }

    eprintln!("✓ Image pushed to {}", registry_image);
    Ok(())
}

fn compute_source_digest() -> eyre::Result<String> {
    if let Some(root) = git_root() {
        let mut files = git_list_files(&root, &["ls-files", "-z"])?;
        let mut untracked = git_list_files(&root, &["ls-files", "-o", "--exclude-standard", "-z"])?;
        files.append(&mut untracked);
        files.retain(|path| !should_skip_path(path));
        files.sort();
        files.dedup();
        return hash_files(&root, &files);
    }

    let root = std::env::current_dir()?;
    let mut hasher = Sha256::new();
    hash_dir(&root, &root, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn git_root() -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout);
    let root = root.trim();
    if root.is_empty() {
        return None;
    }
    Some(PathBuf::from(root))
}

fn git_list_files(root: &Path, args: &[&str]) -> eyre::Result<Vec<PathBuf>> {
    let root_str = root.to_string_lossy();
    let output = std::process::Command::new("git")
        .args(["-C", root_str.as_ref()])
        .args(args)
        .output()?;

    if !output.status.success() {
        return Err(eyre::eyre!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let mut files = Vec::new();
    for entry in output.stdout.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(entry).to_string();
        files.push(PathBuf::from(path));
    }

    Ok(files)
}

fn hash_files(root: &Path, files: &[PathBuf]) -> eyre::Result<String> {
    let mut hasher = Sha256::new();
    for rel in files {
        let rel_str = rel.to_string_lossy();
        hasher.update(rel_str.as_bytes());
        hasher.update([0u8]);

        let path = root.join(rel);
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };

        let mut buf = [0u8; 8192];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        hasher.update([0u8]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_dir(root: &Path, dir: &Path, hasher: &mut Sha256) -> eyre::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_string());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if should_skip_component(&name) {
                continue;
            }
            hash_dir(root, &path, hasher)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel_str = rel.to_string_lossy();
        hasher.update(rel_str.as_bytes());
        hasher.update([0u8]);

        let mut file = fs::File::open(&path)?;
        let mut buf = [0u8; 8192];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        hasher.update([0u8]);
    }

    Ok(())
}

fn should_skip_component(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "rbft-monitor-logs"
    )
}

fn should_skip_path(path: &Path) -> bool {
    let mut components = path.components();
    let first = match components.next() {
        Some(component) => component,
        None => return false,
    };
    let first = first.as_os_str().to_string_lossy();
    should_skip_component(&first)
}
