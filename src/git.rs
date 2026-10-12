use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub old_path: Option<String>,
    pub staged: char,
    pub unstaged: char,
}

impl Change {
    pub fn in_index(&self) -> bool {
        self.staged != ' ' && self.staged != '?'
    }

    pub fn in_worktree(&self) -> bool {
        self.unstaged != ' '
    }

    pub fn is_untracked(&self) -> bool {
        self.unstaged == '?'
    }

    pub fn is_conflict(&self) -> bool {
        matches!(
            (self.staged, self.unstaged),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub upstream_gone: bool,
    pub detached: bool,
    pub unborn: bool,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: String,
    pub short_id: String,
    pub author: String,
    pub time: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFile {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

struct CmdOut {
    stdout: Vec<u8>,
    stderr: String,
}

pub fn version() -> Result<String, String> {
    let mut cmd = git_cmd(None);
    cmd.arg("--version");
    let out = finish(cmd)?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        Ok("git".into())
    } else {
        Ok(text)
    }
}

pub fn toplevel(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err("路径不存在".into());
    }
    let mut cmd = git_cmd(None);
    cmd.arg("-C").arg(path).args(["rev-parse", "--show-toplevel"]);
    let out = finish(cmd)?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err("这不是 Git 仓库".into());
    }
    Ok(PathBuf::from(text))
}

pub fn snapshot(repo: &Path) -> Result<Snapshot, String> {
    let out = run(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "-b",
            "--untracked-files=normal",
        ],
    )?;
    parse_status_z(&out.stdout)
}

pub fn branches(repo: &Path) -> Result<Vec<BranchInfo>, String> {
    let out = run(repo, &["branch", "--format=%(HEAD)|%(refname:short)"])?;
    Ok(parse_branches(&String::from_utf8_lossy(&out.stdout)))
}

pub fn log(repo: &Path, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let n = limit.to_string();
    match run(
        repo,
        &[
            "log",
            "-n",
            &n,
            "--date=iso",
            "--pretty=format:%H%x1f%h%x1f%an%x1f%ci%x1f%s",
        ],
    ) {
        Ok(out) => Ok(parse_log(&String::from_utf8_lossy(&out.stdout))),
        Err(err) if no_commits(&err) => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

pub fn commit_files(repo: &Path, id: &str) -> Result<Vec<CommitFile>, String> {
    let out = run(
        repo,
        &[
            "diff-tree",
            "--root",
            "-M",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-z",
            id,
        ],
    )?;
    Ok(parse_name_status_z(&out.stdout))
}

pub fn diff_change(repo: &Path, change: &Change, staged: bool) -> Result<String, String> {
    if !staged && (change.is_untracked() || is_dir_path(&change.path)) {
        return untracked_preview(repo, &change.path);
    }
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--unified=3",
    ];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(&change.path);
    let out = run(repo, &args)?;
    Ok(diff_text(&out.stdout))
}

pub fn diff_commit_file(repo: &Path, id: &str, file: &CommitFile) -> Result<String, String> {
    let out = run(
        repo,
        &[
            "show",
            "--no-color",
            "--format=",
            "--unified=3",
            id,
            "--",
            &file.path,
        ],
    )?;
    Ok(diff_text(&out.stdout))
}

pub fn stage(repo: &Path, path: &str) -> Result<(), String> {
    check_rel(path)?;
    run(repo, &["add", "--", path]).map(|_| ())
}

pub fn stage_all(repo: &Path) -> Result<(), String> {
    run(repo, &["add", "-A"]).map(|_| ())
}

pub fn unstage(repo: &Path, path: &str, unborn: bool) -> Result<(), String> {
    check_rel(path)?;
    if unborn {
        run(repo, &["rm", "--cached", "-q", "--", path]).map(|_| ())
    } else {
        run(repo, &["restore", "--staged", "--", path]).map(|_| ())
    }
}

pub fn unstage_all(repo: &Path, unborn: bool) -> Result<(), String> {
    if unborn {
        run(repo, &["rm", "-r", "--cached", "-q", "--", "."]).map(|_| ())
    } else {
        run(repo, &["restore", "--staged", "--", "."]).map(|_| ())
    }
}

pub fn discard(repo: &Path, path: &str) -> Result<(), String> {
    check_rel(path)?;
    run(repo, &["restore", "--", path]).map(|_| ())
}

pub fn delete_untracked(repo: &Path, rel: &str) -> Result<(), String> {
    let path = safe_join(repo, rel)?;
    if path.is_dir() {
        return Err("不会删除整个未跟踪目录".into());
    }
    std::fs::remove_file(&path).map_err(|err| format!("删除失败：{err}"))
}

pub fn commit(repo: &Path, message: &str) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("提交说明不能为空".into());
    }
    let out = run(repo, &["commit", "-m", message])?;
    Ok(combined(&out))
}

pub fn switch_branch(repo: &Path, name: &str) -> Result<(), String> {
    if !valid_branch_name(name) {
        return Err("分支名不合法".into());
    }
    run(repo, &["switch", "--", name]).map(|_| ())
}

pub fn create_branch(repo: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if !valid_branch_name(name) {
        return Err("分支名不合法".into());
    }
    run(repo, &["switch", "-c", name]).map(|_| ())
}

pub fn pull(repo: &Path) -> Result<String, String> {
    let out = run(repo, &["pull"])?;
    Ok(combined(&out))
}

pub fn push(repo: &Path) -> Result<String, String> {
    match run(repo, &["push"]) {
        Ok(out) => Ok(combined(&out)),
        Err(err) if needs_upstream(&err) => {
            let out = run(repo, &["push", "-u", "origin", "HEAD"])?;
            Ok(combined(&out))
        }
        Err(err) => Err(err),
    }
}

pub fn valid_branch_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        return false;
    }
    if name.starts_with('-') || name.ends_with('.') || name.ends_with('/') || name.ends_with(".lock")
    {
        return false;
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return false;
    }
    name.chars().all(|ch| {
        !ch.is_control()
            && ch != ' '
            && ch != '~'
            && ch != '^'
            && ch != ':'
            && ch != '\\'
            && ch != '?'
            && ch != '*'
            && ch != '['
    })
}

fn needs_upstream(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("no upstream") || lower.contains("has no upstream") || lower.contains("set the upstream")
}

fn no_commits(err: &str) -> bool {
    err.contains("does not have any commits") || err.contains("bad default revision")
}

fn diff_text(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout).trim_end().to_string();
    if text.is_empty() {
        "没有可显示的文本差异。".into()
    } else {
        limit_text(text)
    }
}

fn is_dir_path(path: &str) -> bool {
    path.ends_with('/') || path.ends_with('\\')
}

fn untracked_preview(repo: &Path, rel: &str) -> Result<String, String> {
    if is_dir_path(rel) {
        return Ok(format!("未跟踪目录 {rel}\n暂存后会加入其中的文件。"));
    }
    let path = safe_join(repo, rel)?;
    if path.is_dir() {
        return Ok(format!("未跟踪目录 {rel}\n暂存后会加入其中的文件。"));
    }
    let meta = std::fs::metadata(&path).map_err(|err| format!("无法读取文件：{err}"))?;
    if meta.len() > 512 * 1024 {
        return Ok(format!(
            "文件较大（{} KB），不预览内容。",
            meta.len() / 1024
        ));
    }
    let bytes = std::fs::read(&path).map_err(|err| format!("无法读取文件：{err}"))?;
    if bytes.contains(&0) {
        return Ok("二进制文件，不预览内容。".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut out = format!("新文件 {rel}\n");
    for (index, line) in text.lines().enumerate() {
        if index >= 2000 {
            out.push_str("… 已截断\n");
            break;
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if text.is_empty() {
        out.push_str("（空文件）\n");
    }
    Ok(out)
}

fn limit_text(text: String) -> String {
    const MAX: usize = 200_000;
    if text.len() <= MAX {
        return text;
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… 差异过长，已截断", &text[..end])
}

fn parse_status_z(bytes: &[u8]) -> Result<Snapshot, String> {
    let entries: Vec<&[u8]> = bytes.split(|byte| *byte == 0).filter(|part| !part.is_empty()).collect();
    if entries.is_empty() {
        return Err("无法读取仓库状态".into());
    }
    let header = String::from_utf8_lossy(entries[0]).trim().to_string();
    if !header.starts_with("##") {
        return Err("无法识别仓库状态".into());
    }
    let mut snap = parse_branch_header(&header);
    let mut index = 1;
    while index < entries.len() {
        let rec = entries[index];
        if rec.len() < 3 {
            index += 1;
            continue;
        }
        let staged = rec[0] as char;
        let unstaged = rec[1] as char;
        let path = String::from_utf8_lossy(&rec[3..]).to_string();
        let mut old_path = None;
        if staged == 'R' || staged == 'C' || unstaged == 'R' || unstaged == 'C' {
            if let Some(old) = entries.get(index + 1) {
                old_path = Some(String::from_utf8_lossy(old).to_string());
                index += 1;
            }
        }
        snap.changes.push(Change {
            path,
            old_path,
            staged,
            unstaged,
        });
        index += 1;
    }
    Ok(snap)
}

fn parse_branch_header(header: &str) -> Snapshot {
    let mut snap = Snapshot {
        branch: "HEAD".into(),
        upstream: None,
        ahead: 0,
        behind: 0,
        upstream_gone: false,
        detached: false,
        unborn: false,
        changes: Vec::new(),
    };
    let text = header.trim().trim_start_matches("##").trim();
    if text.starts_with("HEAD (no branch)") || text.starts_with("HEAD (detached") {
        snap.branch = "HEAD".into();
        snap.detached = true;
        return snap;
    }
    let (body, unborn) = text
        .strip_prefix("No commits yet on ")
        .map(|rest| (rest, true))
        .unwrap_or((text, false));
    snap.unborn = unborn;
    let (name, rest) = body
        .split_once("...")
        .map(|(name, rest)| (name, Some(rest)))
        .unwrap_or((body, None));
    snap.branch = name.trim().to_string();
    if let Some(rest) = rest {
        apply_tracking(&mut snap, rest);
    }
    snap
}

fn apply_tracking(snap: &mut Snapshot, rest: &str) {
    let (upstream, bracket) = rest
        .split_once(" [")
        .map(|(upstream, bracket)| (upstream, Some(bracket)))
        .unwrap_or((rest, None));
    let upstream = upstream.trim();
    if !upstream.is_empty() {
        snap.upstream = Some(upstream.to_string());
    }
    let Some(bracket) = bracket else {
        return;
    };
    let inner = bracket.trim().trim_end_matches(']');
    if inner == "gone" {
        snap.upstream_gone = true;
        return;
    }
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            snap.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            snap.behind = n.parse().unwrap_or(0);
        }
    }
}

fn parse_branches(text: &str) -> Vec<BranchInfo> {
    text.lines()
        .filter_map(|line| {
            let (head, name) = line.split_once('|')?;
            let name = name.trim();
            if name.is_empty() || name.starts_with('(') {
                return None;
            }
            Some(BranchInfo {
                name: name.to_string(),
                current: head.trim() == "*",
            })
        })
        .collect()
}

fn parse_log(text: &str) -> Vec<CommitInfo> {
    text.lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            let id = parts.next()?.to_string();
            let short_id = parts.next()?.to_string();
            let author = parts.next()?.to_string();
            let raw_time = parts.next()?;
            let summary = parts.next()?.to_string();
            let time = raw_time.get(..16).unwrap_or(raw_time).to_string();
            Some(CommitInfo {
                id,
                short_id,
                author,
                time,
                summary,
            })
        })
        .collect()
}

fn parse_name_status_z(bytes: &[u8]) -> Vec<CommitFile> {
    let parts: Vec<&[u8]> = bytes.split(|byte| *byte == 0).filter(|part| !part.is_empty()).collect();
    let mut index = 0;
    let mut files = Vec::new();
    while index < parts.len() {
        let code = String::from_utf8_lossy(parts[index]);
        let kind = code.chars().next().unwrap_or('?');
        if kind == 'R' || kind == 'C' {
            if index + 2 >= parts.len() {
                break;
            }
            files.push(CommitFile {
                status: kind.to_string(),
                old_path: Some(String::from_utf8_lossy(parts[index + 1]).to_string()),
                path: String::from_utf8_lossy(parts[index + 2]).to_string(),
            });
            index += 3;
        } else {
            if index + 1 >= parts.len() {
                break;
            }
            files.push(CommitFile {
                status: kind.to_string(),
                path: String::from_utf8_lossy(parts[index + 1]).to_string(),
                old_path: None,
            });
            index += 2;
        }
    }
    files
}

fn check_rel(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains('\0')
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.split(['/', '\\']).any(|part| part == "..")
    {
        return Err("非法路径".into());
    }
    Ok(())
}

fn safe_join(repo: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim_end_matches(['/', '\\']);
    check_rel(rel)?;
    Ok(repo.join(rel))
}

fn combined(out: &CmdOut) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = out.stderr.trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => "完成".into(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn run(repo: &Path, args: &[&str]) -> Result<CmdOut, String> {
    let mut cmd = git_cmd(Some(repo));
    cmd.arg("-c").arg("core.quotepath=false");
    cmd.args(args);
    finish(cmd)
}

fn git_cmd(repo: Option<&Path>) -> Command {
    let mut cmd = Command::new("git");
    if let Some(repo) = repo {
        cmd.current_dir(repo);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "auto")
        .env("GIT_PAGER", "cat");
    hide_console(&mut cmd);
    cmd
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
}

fn finish(mut cmd: Command) -> Result<CmdOut, String> {
    let output = cmd.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "未找到 git。请先安装 Git for Windows，并把它加入 PATH。".into()
        } else {
            format!("无法启动 git：{err}")
        }
    })?;
    output_to_result(output)
}

fn output_to_result(output: Output) -> Result<CmdOut, String> {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(CmdOut {
            stdout: output.stdout,
            stderr,
        })
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut message = if !stderr.is_empty() { stderr } else { stdout };
        if message.is_empty() {
            message = format!("git 执行失败（{}）", output.status);
        }
        if message.contains("Please tell me who you are") || message.contains("user.email") {
            message.push_str("\n\n请先配置提交身份，例如：\ngit config --global user.name \"你的名字\"\ngit config --global user.email \"you@example.com\"");
        }
        if message.contains("not a git repository") {
            message = "这不是 Git 仓库。请选择仓库目录，或它的子目录。".into();
        }
        Err(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_header_and_entries() {
        let raw = b"## main...origin/main [ahead 2, behind 1]\0MM src/main.rs\0R  new.txt\0old.txt\0?? notes.txt\0?? dir/\0";
        let snap = parse_status_z(raw).unwrap();
        assert_eq!(snap.branch, "main");
        assert_eq!(snap.upstream.as_deref(), Some("origin/main"));
        assert_eq!(snap.ahead, 2);
        assert_eq!(snap.behind, 1);
        assert_eq!(snap.changes.len(), 4);
        assert_eq!(snap.changes[1].path, "new.txt");
        assert_eq!(snap.changes[1].old_path.as_deref(), Some("old.txt"));
        assert!(snap.changes[2].is_untracked());
        assert!(snap.changes[0].in_index() && snap.changes[0].in_worktree());
    }

    #[test]
    fn parses_unborn_and_detached_headers() {
        let unborn = parse_branch_header("## No commits yet on main");
        assert!(unborn.unborn);
        assert_eq!(unborn.branch, "main");
        let gone = parse_branch_header("## No commits yet on master...origin/master [gone]");
        assert!(gone.unborn);
        assert_eq!(gone.branch, "master");
        assert_eq!(gone.upstream.as_deref(), Some("origin/master"));
        assert!(gone.upstream_gone);
        let detached = parse_branch_header("## HEAD (no branch)");
        assert!(detached.detached);
    }

    #[test]
    fn parses_log_branches_and_name_status() {
        let commits = parse_log("abc\u{1f}ab\u{1f}Ada\u{1f}2026-10-12 12:11:52 +0800\u{1f}init\n");
        assert_eq!(commits[0].short_id, "ab");
        assert_eq!(commits[0].time, "2026-10-12 12:11");
        assert_eq!(commits[0].summary, "init");

        let branches = parse_branches("*|main\n |feature\n*|(HEAD detached at ab)\n");
        assert_eq!(branches.len(), 2);
        assert!(branches[0].current);

        let files = parse_name_status_z(b"M\0src/main.rs\0R100\0old.txt\0new.txt\0");
        assert_eq!(files[0].status, "M");
        assert_eq!(files[1].path, "new.txt");
        assert_eq!(files[1].old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn rejects_bad_branch_names() {
        assert!(valid_branch_name("feature/ui"));
        assert!(!valid_branch_name(""));
        assert!(!valid_branch_name("has space"));
        assert!(!valid_branch_name("a..b"));
        assert!(!valid_branch_name("-temp"));
    }

    #[test]
    fn local_repo_roundtrip() {
        let root = std::env::temp_dir().join(format!("gitshow-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let hooks = root.join("empty-hooks");
        std::fs::create_dir_all(&hooks).unwrap();

        fn git(repo: &Path, args: &[&str]) {
            let status = Command::new("git")
                .current_dir(repo)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        }

        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.email", "gitshow@example.com"]);
        git(&root, &["config", "user.name", "GitShow"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        git(&root, &["config", "core.hooksPath", hooks.to_str().unwrap()]);

        std::fs::write(root.join("a.txt"), "hello\n").unwrap();
        let before = snapshot(&root).unwrap();
        assert!(before.unborn);
        assert!(before.changes.iter().any(|change| change.path == "a.txt"));

        stage(&root, "a.txt").unwrap();
        let staged = snapshot(&root).unwrap();
        assert!(staged.changes.iter().any(|change| change.path == "a.txt" && change.in_index()));

        commit(&root, "init").unwrap();
        let history = log(&root, 10).unwrap();
        assert_eq!(history[0].summary, "init");
        std::fs::write(root.join("a.txt"), "hello world\n").unwrap();
        let diff = diff_change(
            &root,
            &Change {
                path: "a.txt".into(),
                old_path: None,
                staged: ' ',
                unstaged: 'M',
            },
            false,
        )
        .unwrap();
        assert!(diff.contains("+hello world") || diff.contains("hello world"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
