use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use huggingface_hub::{CreateRepoParams, HFClient, HFError, RepoType};
use tokio::process::Command;

#[derive(Parser)]
#[command(name = "migrate-to-kernel-repo")]
#[command(about = "Migrate Hugging Face Hub model repos to kernel repos")]
#[command(
    long_about = "Clone a model repo (with all branches and LFS objects), create a kernel-type \
                  repo on the Hub, squash each branch to a single commit to avoid broken LFS \
                  history, and push everything to the new kernel repo."
)]
struct Args {
    /// One or more repo IDs to migrate (e.g. kernels-community/relu)
    repo_ids: Vec<String>,

    /// Path to a file containing repo IDs, one per line
    #[arg(long, short)]
    batch: Option<PathBuf>,

    /// Show what would be done without doing it
    #[arg(long, short = 'n')]
    dry_run: bool,

    /// Keep local clone after migration
    #[arg(long, short)]
    keep: bool,

    /// Directory for cloned repos (default: temp directory)
    #[arg(long, short)]
    work_dir: Option<PathBuf>,

    /// Create the kernel repo as private
    #[arg(long)]
    private: bool,
}

fn log(msg: &str, indent: usize) {
    let pad = "  ".repeat(indent);
    eprintln!("{pad}{msg}");
}

async fn run_git(args: &[&str], cwd: &Path, quiet: bool) -> Result<String, String> {
    if !quiet {
        log(&format!("$ git {}", args.join(" ")), 1);
    }
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| format!("failed to run git: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !quiet {
        for line in stdout.trim().lines() {
            if !line.is_empty() {
                log(line, 2);
            }
        }
        for line in stderr.trim().lines() {
            if !line.is_empty() {
                log(line, 2);
            }
        }
    }

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(format!("git {} failed: {}", args[0], stderr.trim()))
    }
}

fn get_branches(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.contains("origin/HEAD") && l.starts_with("origin/"))
        .map(|l| l.strip_prefix("origin/").unwrap().to_string())
        .collect()
}

async fn migrate_repo(
    repo_id: &str,
    client: &HFClient,
    dry_run: bool,
    keep: bool,
    work_dir: Option<&Path>,
    private: bool,
) -> bool {
    let model_url = format!("git@hf.co:{repo_id}");
    let kernel_url = format!("git@hf.co:kernels/{repo_id}");

    eprintln!();
    log(&format!("repo:       {repo_id}"), 0);
    log(&format!("model url:  {model_url}"), 0);
    log(&format!("kernel url: {kernel_url}"), 0);

    if dry_run {
        log(
            "[DRY RUN] would: create kernel repo -> clone -> fetch LFS -> squash -> push",
            0,
        );
        return true;
    }

    // Determine clone path
    let clone_path = match work_dir {
        Some(dir) => dir.join(repo_id.replace('/', "--")),
        None => {
            let tmp = std::env::temp_dir().join(format!(
                "migrate-kernel-{}",
                repo_id.replace('/', "--")
            ));
            tmp
        }
    };

    let result = do_migrate(repo_id, &model_url, &kernel_url, &clone_path, client, private).await;

    if !keep && clone_path.exists() {
        log("cleaning up local clone", 0);
        let _ = std::fs::remove_dir_all(&clone_path);
    }

    result
}

async fn do_migrate(
    repo_id: &str,
    model_url: &str,
    kernel_url: &str,
    clone_path: &Path,
    client: &HFClient,
    private: bool,
) -> bool {
    // 1. Create kernel repo on Hub (fail fast before expensive clone/LFS)
    log("creating kernel repo on Hub...", 0);
    let params = CreateRepoParams::builder()
        .repo_id(repo_id)
        .repo_type(RepoType::Kernel)
        .private(private)
        .exist_ok(true)
        .build();

    if let Err(e) = client.create_repo(&params).await {
        match &e {
            HFError::Http { status, .. } if status.as_u16() == 409 => {
                log("kernel repo already exists, continuing", 1);
            }
            HFError::Http { status, url, body } => {
                log(&format!("FAILED to create kernel repo: HTTP {status} {url}"), 0);
                if !body.is_empty() {
                    log(&format!("Response: {body}"), 1);
                }
                return false;
            }
            _ => {
                log(&format!("FAILED to create kernel repo: {e}"), 0);
                return false;
            }
        }
    }

    // 2. Clone
    if clone_path.join(".git").exists() {
        log("reusing existing clone", 0);
    } else {
        log("cloning model repo...", 0);
        std::fs::create_dir_all(clone_path).ok();
        let parent = clone_path.parent().unwrap_or(Path::new("."));
        if run_git(
            &["clone", model_url, &clone_path.to_string_lossy()],
            parent,
            false,
        )
        .await
        .is_err()
        {
            log("FAILED to clone", 0);
            return false;
        }
    }

    // 3. Fetch all LFS objects
    log("fetching LFS objects...", 0);
    if run_git(&["lfs", "fetch", "--all", "origin"], clone_path, false)
        .await
        .is_err()
    {
        log("FAILED to fetch LFS objects", 0);
        return false;
    }
    let _ = run_git(&["lfs", "pull"], clone_path, true).await;

    // 4. Discover and track branches
    let branch_output = match run_git(&["branch", "-r"], clone_path, true).await {
        Ok(s) => s,
        Err(_) => {
            log("FAILED to list branches", 0);
            return false;
        }
    };
    let branches = get_branches(&branch_output);
    log(&format!("branches: {branches:?}"), 0);

    for branch in &branches {
        if run_git(
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
            clone_path,
            true,
        )
        .await
        .is_err()
        {
            let _ = run_git(
                &[
                    "branch",
                    "--track",
                    branch,
                    &format!("origin/{branch}"),
                ],
                clone_path,
                true,
            )
            .await;
        }
    }

    // 5. Repoint remote
    let _ = run_git(
        &["remote", "set-url", "origin", kernel_url],
        clone_path,
        true,
    )
    .await;

    // 6. Squash each branch to a single orphan commit
    log("squashing branches...", 0);

    let current_branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"], clone_path, true)
        .await
        .unwrap_or_else(|_| "main".to_string())
        .trim()
        .to_string();

    let commit_msg = format!("Migrated from model repo {repo_id}");

    for branch in &branches {
        log(&format!("squashing {branch}"), 1);
        let temp = format!("_migrate_{branch}");

        let _ = run_git(&["checkout", branch], clone_path, true).await;
        let _ = run_git(&["checkout", "--orphan", &temp], clone_path, true).await;
        let _ = run_git(&["add", "-A"], clone_path, true).await;
        let _ = run_git(&["commit", "-m", &commit_msg], clone_path, true).await;
        let _ = run_git(&["branch", "-D", branch], clone_path, true).await;
        let _ = run_git(&["branch", "-m", &temp, branch], clone_path, true).await;
    }

    let _ = run_git(&["checkout", &current_branch], clone_path, true).await;

    // 7. Push
    log("pushing to kernel repo...", 0);
    if run_git(&["push", "--force", "--all", "origin"], clone_path, false)
        .await
        .is_err()
    {
        log("FAILED to push", 0);
        return false;
    }

    log(
        &format!("migration complete: https://huggingface.co/{repo_id}"),
        0,
    );
    true
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    let mut repos: Vec<String> = Vec::new();

    // Load from batch file
    if let Some(batch_path) = &args.batch {
        match std::fs::read_to_string(batch_path) {
            Ok(content) => {
                for line in content.lines() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        repos.push(line.to_string());
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading batch file: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Add positional args
    repos.extend(args.repo_ids.clone());

    if repos.is_empty() {
        eprintln!("No repos specified. Provide repo IDs as arguments or use --batch.");
        return ExitCode::FAILURE;
    }

    // Validate format
    for repo_id in &repos {
        if !repo_id.contains('/') {
            eprintln!("Invalid repo ID: {repo_id:?}. Expected format: 'org/name'");
            return ExitCode::FAILURE;
        }
    }

    // Create HF client
    let client = match HFClient::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to create HF client: {e}");
            if matches!(e, HFError::AuthRequired) {
                eprintln!("Hint: Set HF_TOKEN environment variable with a write-access token.");
            }
            return ExitCode::FAILURE;
        }
    };

    eprintln!("Migrating {} repo(s)...", repos.len());

    let mut succeeded = Vec::new();
    let mut failed = Vec::new();

    for repo_id in &repos {
        let ok = migrate_repo(
            repo_id,
            &client,
            args.dry_run,
            args.keep,
            args.work_dir.as_deref(),
            args.private,
        )
        .await;

        if ok {
            succeeded.push(repo_id.as_str());
        } else {
            failed.push(repo_id.as_str());
        }
    }

    eprintln!();
    eprintln!(
        "Results: {} succeeded, {} failed",
        succeeded.len(),
        failed.len()
    );
    if !failed.is_empty() {
        eprintln!("Failed: {}", failed.join(", "));
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
