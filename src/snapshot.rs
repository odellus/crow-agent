//! Git-based snapshot system for tracking and reverting file changes.
//!
//! This is internal infrastructure, NOT an LLM tool. The agent never calls "snapshot" -
//! the system tracks changes automatically behind the scenes.
//!
//! Snapshots are stored in XDG data directory for persistence across sessions:
//! ~/.crow_agent/snapshots/{project_id}/

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Manages git-based snapshots for a project
pub struct SnapshotManager {
    /// Path to the shadow git directory (~/.crow_agent/snapshots/{project_id})
    git_dir: PathBuf,
    /// Path to the working tree (project root)
    work_tree: PathBuf,
    /// Project ID (derived from working directory hash)
    project_id: String,
}

/// A patch recording what files changed since a snapshot
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Patch {
    /// Git tree hash before the change
    pub hash: String,
    /// Files that were modified
    pub files: Vec<PathBuf>,
}

/// File diff information for display
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileDiff {
    pub file: String,
    pub before: String,
    pub after: String,
    pub additions: usize,
    pub deletions: usize,
}

impl SnapshotManager {
    /// Create a new snapshot manager for a project directory
    /// Uses ~/.crow_agent/snapshots/{project_id} for persistence
    pub fn for_directory(work_tree: PathBuf) -> Self {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("crow_agent");
        let project_id = Self::compute_project_id(&work_tree);
        let git_dir = data_dir.join("snapshots").join(&project_id);
        Self {
            git_dir,
            work_tree,
            project_id,
        }
    }

    /// Create a new snapshot manager with explicit paths (for testing)
    pub fn new(data_dir: &Path, project_id: &str, work_tree: PathBuf) -> Self {
        let git_dir = data_dir.join("snapshots").join(project_id);
        Self {
            git_dir,
            work_tree,
            project_id: project_id.to_string(),
        }
    }

    /// Compute a project ID from the working directory
    /// Uses git root commit hash if in a git repo, otherwise hashes the path
    fn compute_project_id(work_tree: &Path) -> String {
        // Try to get git root commit hash
        if let Ok(output) = std::process::Command::new("git")
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .current_dir(work_tree)
            .output()
        {
            if output.status.success() {
                let hash = String::from_utf8_lossy(&output.stdout);
                let hash = hash.trim();
                if !hash.is_empty() {
                    return hash[..12.min(hash.len())].to_string();
                }
            }
        }

        // Fall back to hashing the path
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        work_tree.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Get the project ID
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Get the snapshot directory path
    pub fn snapshot_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Initialize the shadow git repository if needed
    async fn ensure_init(&self) -> Result<(), String> {
        if !self.git_dir.exists() {
            tokio::fs::create_dir_all(&self.git_dir)
                .await
                .map_err(|e| format!("Failed to create snapshot dir: {}", e))?;

            // Initialize git repo
            let output = Command::new("git")
                .arg("init")
                .env("GIT_DIR", &self.git_dir)
                .env("GIT_WORK_TREE", &self.work_tree)
                .output()
                .await
                .map_err(|e| format!("Failed to init git: {}", e))?;

            if !output.status.success() {
                return Err(format!(
                    "git init failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }

            // Configure git to not convert line endings
            Command::new("git")
                .args(["--git-dir", self.git_dir.to_str().unwrap()])
                .args(["config", "core.autocrlf", "false"])
                .output()
                .await
                .ok();
        }
        Ok(())
    }

    /// Create a snapshot (git tree hash) of current state
    /// Call this before agent runs or before file-modifying tools
    pub async fn track(&self) -> Result<Option<String>, String> {
        self.ensure_init().await?;

        // Stage all files
        let output = Command::new("git")
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["add", "."])
            .current_dir(&self.work_tree)
            .output()
            .await
            .map_err(|e| format!("Failed to git add: {}", e))?;

        if !output.status.success() {
            // Not fatal - might be empty repo
            return Ok(None);
        }

        // Write tree and get hash
        let output = Command::new("git")
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .arg("write-tree")
            .current_dir(&self.work_tree)
            .output()
            .await
            .map_err(|e| format!("Failed to write-tree: {}", e))?;

        if !output.status.success() {
            return Ok(None);
        }

        let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if hash.is_empty() {
            return Ok(None);
        }

        Ok(Some(hash))
    }

    /// Get list of changed files since a snapshot
    pub async fn patch(&self, hash: &str) -> Result<Patch, String> {
        // Stage current state
        Command::new("git")
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["add", "."])
            .current_dir(&self.work_tree)
            .output()
            .await
            .ok();

        // Get changed files
        let output = Command::new("git")
            .args(["-c", "core.autocrlf=false"])
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["diff", "--name-only", hash, "--", "."])
            .current_dir(&self.work_tree)
            .output()
            .await
            .map_err(|e| format!("Failed to get diff: {}", e))?;

        if !output.status.success() {
            return Ok(Patch {
                hash: hash.to_string(),
                files: vec![],
            });
        }

        let files: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| self.work_tree.join(line.trim()))
            .collect();

        Ok(Patch {
            hash: hash.to_string(),
            files,
        })
    }

    /// Revert files to their state at given patches
    pub async fn revert(&self, patches: &[Patch]) -> Result<(), String> {
        let mut reverted = HashSet::new();

        for patch in patches {
            for file in &patch.files {
                if reverted.contains(file) {
                    continue;
                }

                let relative = file
                    .strip_prefix(&self.work_tree)
                    .unwrap_or(file)
                    .to_str()
                    .unwrap_or_default();

                // Try to checkout file from snapshot
                let output = Command::new("git")
                    .args(["--git-dir", self.git_dir.to_str().unwrap()])
                    .args(["--work-tree", self.work_tree.to_str().unwrap()])
                    .args(["checkout", &patch.hash, "--", relative])
                    .current_dir(&self.work_tree)
                    .output()
                    .await;

                match output {
                    Ok(o) if o.status.success() => {
                        reverted.insert(file.clone());
                    }
                    _ => {
                        // File didn't exist in snapshot - check if it should be deleted
                        let check = Command::new("git")
                            .args(["--git-dir", self.git_dir.to_str().unwrap()])
                            .args(["--work-tree", self.work_tree.to_str().unwrap()])
                            .args(["ls-tree", &patch.hash, "--", relative])
                            .current_dir(&self.work_tree)
                            .output()
                            .await;

                        if let Ok(check_output) = check {
                            if check_output.status.success()
                                && String::from_utf8_lossy(&check_output.stdout)
                                    .trim()
                                    .is_empty()
                            {
                                // File didn't exist in snapshot, delete it
                                tokio::fs::remove_file(file).await.ok();
                            }
                        }
                        reverted.insert(file.clone());
                    }
                }
            }
        }

        Ok(())
    }

    /// Full restore to a snapshot state
    pub async fn restore(&self, hash: &str) -> Result<(), String> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "git --git-dir {} --work-tree {} read-tree {} && git --git-dir {} --work-tree {} checkout-index -a -f",
                self.git_dir.to_str().unwrap(),
                self.work_tree.to_str().unwrap(),
                hash,
                self.git_dir.to_str().unwrap(),
                self.work_tree.to_str().unwrap()
            ))
            .current_dir(&self.work_tree)
            .output()
            .await
            .map_err(|e| format!("Failed to restore: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "restore failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Get unified diff from a snapshot to current state
    pub async fn diff(&self, hash: &str) -> Result<String, String> {
        // Stage current state
        Command::new("git")
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["add", "."])
            .current_dir(&self.work_tree)
            .output()
            .await
            .ok();

        let output = Command::new("git")
            .args(["-c", "core.autocrlf=false"])
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["diff", hash, "--", "."])
            .current_dir(&self.work_tree)
            .output()
            .await
            .map_err(|e| format!("Failed to get diff: {}", e))?;

        if !output.status.success() {
            return Ok(String::new());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Get detailed per-file diffs between two snapshots
    pub async fn diff_full(&self, from: &str, to: &str) -> Result<Vec<FileDiff>, String> {
        let output = Command::new("git")
            .args(["-c", "core.autocrlf=false"])
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["diff", "--no-renames", "--numstat", from, to, "--", "."])
            .current_dir(&self.work_tree)
            .output()
            .await
            .map_err(|e| format!("Failed to get diff: {}", e))?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let mut results = vec![];

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if line.is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != 3 {
                continue;
            }

            let (additions, deletions, file) = (parts[0], parts[1], parts[2]);
            let is_binary = additions == "-" && deletions == "-";

            let before = if is_binary {
                String::new()
            } else {
                self.show_file(from, file).await.unwrap_or_default()
            };

            let after = if is_binary {
                String::new()
            } else {
                self.show_file(to, file).await.unwrap_or_default()
            };

            results.push(FileDiff {
                file: file.to_string(),
                before,
                after,
                additions: additions.parse().unwrap_or(0),
                deletions: deletions.parse().unwrap_or(0),
            });
        }

        Ok(results)
    }

    /// Get file contents at a specific snapshot
    async fn show_file(&self, hash: &str, file: &str) -> Result<String, String> {
        let output = Command::new("git")
            .args(["-c", "core.autocrlf=false"])
            .args(["--git-dir", self.git_dir.to_str().unwrap()])
            .args(["--work-tree", self.work_tree.to_str().unwrap()])
            .args(["show", &format!("{}:{}", hash, file)])
            .output()
            .await
            .map_err(|e| format!("Failed to show file: {}", e))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_snapshot_track_and_revert() {
        let work_dir = TempDir::new().unwrap();
        let data_temp = TempDir::new().unwrap();
        let work_tree = work_dir.path().to_path_buf();
        let data_dir = data_temp.path().to_path_buf();

        let manager = SnapshotManager::new(&data_dir, "test-project", work_tree.clone());

        // Create initial file
        let test_file = work_tree.join("test.txt");
        tokio::fs::write(&test_file, "initial content")
            .await
            .unwrap();

        // Track initial state
        let hash = manager.track().await.unwrap().unwrap();
        assert!(!hash.is_empty());

        // Modify file
        tokio::fs::write(&test_file, "modified content")
            .await
            .unwrap();

        // Get patch
        let patch = manager.patch(&hash).await.unwrap();
        assert_eq!(patch.files.len(), 1);
        assert!(patch.files[0].ends_with("test.txt"));

        // Revert
        manager.revert(&[patch]).await.unwrap();

        // Verify content restored
        let content = tokio::fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "initial content");
    }

    #[tokio::test]
    async fn test_snapshot_new_file_delete_on_revert() {
        let work_dir = TempDir::new().unwrap();
        let data_temp = TempDir::new().unwrap();
        let work_tree = work_dir.path().to_path_buf();
        let data_dir = data_temp.path().to_path_buf();

        let manager = SnapshotManager::new(&data_dir, "test-project", work_tree.clone());

        // Track empty state
        let hash = manager.track().await.unwrap().unwrap();

        // Create new file
        let new_file = work_tree.join("new.txt");
        tokio::fs::write(&new_file, "new content").await.unwrap();

        // Get patch
        let patch = manager.patch(&hash).await.unwrap();
        assert_eq!(patch.files.len(), 1);

        // Revert should delete the new file
        manager.revert(&[patch]).await.unwrap();

        // File should be deleted
        assert!(!new_file.exists());
    }

    #[tokio::test]
    async fn test_snapshot_diff() {
        let temp_dir = TempDir::new().unwrap();
        let work_tree = temp_dir.path().to_path_buf();
        let data_dir = temp_dir.path().join(".crow");

        let manager = SnapshotManager::new(&data_dir, "test-project", work_tree.clone());

        // Create initial file
        let test_file = work_tree.join("test.txt");
        tokio::fs::write(&test_file, "line1\nline2\n")
            .await
            .unwrap();

        // Track
        let hash = manager.track().await.unwrap().unwrap();

        // Modify
        tokio::fs::write(&test_file, "line1\nline2\nline3\n")
            .await
            .unwrap();

        // Get diff
        let diff = manager.diff(&hash).await.unwrap();
        assert!(diff.contains("+line3"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::TempDir;

    /// Simulates the full ACP session flow with snapshots
    #[tokio::test]
    async fn test_full_acp_session_flow() {
        // Setup: Create work directory and data directory
        let work_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let work_tree = work_dir.path().to_path_buf();

        // Create initial project files
        let main_rs = work_tree.join("main.rs");
        let config_toml = work_tree.join("config.toml");
        tokio::fs::write(&main_rs, "fn main() {\n    println!(\"Hello\");\n}\n").await.unwrap();
        tokio::fs::write(&config_toml, "[settings]\nversion = 1\n").await.unwrap();

        // Initialize snapshot manager (happens at session creation)
        let manager = SnapshotManager::new(data_dir.path(), "test-session", work_tree.clone());

        // === Simulate Prompt 1: Agent modifies main.rs ===
        
        // Before prompt: track current state
        let snapshot_before_prompt1 = manager.track().await.unwrap().unwrap();
        assert!(!snapshot_before_prompt1.is_empty());

        // Simulate: Agent uses edit_file tool to modify main.rs
        tokio::fs::write(&main_rs, "fn main() {\n    println!(\"Hello, World!\");\n}\n").await.unwrap();

        // After tool: get patch
        let patch1 = manager.patch(&snapshot_before_prompt1).await.unwrap();
        assert_eq!(patch1.files.len(), 1);
        assert!(patch1.files[0].ends_with("main.rs"));

        // Store patch (simulating session.patches.push())
        let mut patches = vec![patch1.clone()];

        // === Simulate Prompt 2: Agent creates new file ===
        
        // Before prompt: track current state
        let snapshot_before_prompt2 = manager.track().await.unwrap().unwrap();
        
        // Simulate: Agent uses write tool to create new file
        let new_file = work_tree.join("utils.rs");
        tokio::fs::write(&new_file, "pub fn helper() {}\n").await.unwrap();

        // After tool: get patch
        let patch2 = manager.patch(&snapshot_before_prompt2).await.unwrap();
        assert_eq!(patch2.files.len(), 1);
        assert!(patch2.files[0].ends_with("utils.rs"));
        patches.push(patch2.clone());

        // === Simulate session/patches query ===
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].files.len(), 1);
        assert_eq!(patches[1].files.len(), 1);

        // === Simulate session/revert (last patch only) ===
        let last_patch = patches.pop().unwrap();
        manager.revert(&[last_patch]).await.unwrap();

        // Verify: new file should be deleted
        assert!(!new_file.exists(), "utils.rs should be deleted after revert");
        
        // Verify: main.rs should still have modifications from prompt 1
        let main_content = tokio::fs::read_to_string(&main_rs).await.unwrap();
        assert!(main_content.contains("Hello, World!"), "main.rs changes should persist");

        // === Simulate session/revert (remaining patch) ===
        let first_patch = patches.pop().unwrap();
        manager.revert(&[first_patch]).await.unwrap();

        // Verify: main.rs should be back to original
        let main_content = tokio::fs::read_to_string(&main_rs).await.unwrap();
        assert!(main_content.contains("println!(\"Hello\");"), "main.rs should be reverted");
        assert!(!main_content.contains("World"), "main.rs should not have 'World'");

        // Config should be unchanged throughout
        let config_content = tokio::fs::read_to_string(&config_toml).await.unwrap();
        assert!(config_content.contains("version = 1"));
    }

    /// Test multiple tools in single prompt
    #[tokio::test]
    async fn test_multiple_tools_single_prompt() {
        let work_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let work_tree = work_dir.path().to_path_buf();

        // Initial state
        let file1 = work_tree.join("file1.txt");
        let file2 = work_tree.join("file2.txt");
        tokio::fs::write(&file1, "original1").await.unwrap();
        tokio::fs::write(&file2, "original2").await.unwrap();

        let manager = SnapshotManager::new(data_dir.path(), "multi-tool", work_tree.clone());

        // Before prompt: snapshot
        let snapshot = manager.track().await.unwrap().unwrap();

        // Tool 1: edit file1
        tokio::fs::write(&file1, "modified1").await.unwrap();
        
        // Tool 2: edit file2
        tokio::fs::write(&file2, "modified2").await.unwrap();

        // Tool 3: create file3
        let file3 = work_tree.join("file3.txt");
        tokio::fs::write(&file3, "new file").await.unwrap();

        // Get cumulative patch
        let patch = manager.patch(&snapshot).await.unwrap();
        assert_eq!(patch.files.len(), 3, "Should detect all 3 changed files");

        // Verify current state
        assert_eq!(tokio::fs::read_to_string(&file1).await.unwrap(), "modified1");
        assert_eq!(tokio::fs::read_to_string(&file2).await.unwrap(), "modified2");
        assert!(file3.exists());

        // Revert all changes at once
        manager.revert(&[patch]).await.unwrap();

        // Verify all reverted
        assert_eq!(tokio::fs::read_to_string(&file1).await.unwrap(), "original1");
        assert_eq!(tokio::fs::read_to_string(&file2).await.unwrap(), "original2");
        assert!(!file3.exists(), "file3 should be deleted");
    }

    /// Test revert with nested directories
    #[tokio::test]
    async fn test_nested_directory_revert() {
        let work_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();
        let work_tree = work_dir.path().to_path_buf();

        let manager = SnapshotManager::new(data_dir.path(), "nested", work_tree.clone());

        // Initial snapshot (empty)
        let snapshot = manager.track().await.unwrap().unwrap();

        // Create nested structure
        let nested_dir = work_tree.join("src/components/ui");
        tokio::fs::create_dir_all(&nested_dir).await.unwrap();
        
        let deep_file = nested_dir.join("button.rs");
        tokio::fs::write(&deep_file, "pub struct Button;").await.unwrap();

        // Get patch
        let patch = manager.patch(&snapshot).await.unwrap();
        assert_eq!(patch.files.len(), 1);

        // Revert
        manager.revert(&[patch]).await.unwrap();
        
        // File should be gone
        assert!(!deep_file.exists());
    }
}
