//! Чтение git-истории через CLI и раскладка её в граф для карты мыслей.
//!
//! Модуль ничего не знает про egui: он возвращает [`GraphView`] — узлы с
//! координатами и рёбра между ними, которые вызывающий код превращает в
//! документ карты формата `git-graph`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

/// Ширина одной «колонки» (поколения коммитов) на графе.
const GRAPH_COL_W: f32 = 320.0;
/// Высота одной «дорожки» (ветки) на графе.
const GRAPH_LANE_H: f32 = 76.0;
/// Сколько всего коммитов показываем на графе, чтобы не распухать на больших
/// репозиториях. Лишние (самые старые из середины) отбрасываются.
const MAX_GRAPH_COMMITS: usize = 160;

/// Один коммит из `git log --oneline`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub subject: String,
}

/// Ссылка на ветку: локальная или удалённая.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub head: String,
    pub is_remote: bool,
    pub is_head: bool,
}

/// Всё, что нужно для построения карты: ветки и коммиты репозитория.
#[derive(Clone, Debug)]
pub struct Repo {
    pub name: String,
    /// Имя текущей ветки (нет при detached HEAD).
    pub head: Option<String>,
    /// Хэш текущего коммита (есть всегда, если история не пуста).
    pub head_commit: Option<String>,
    pub branches: Vec<Branch>,
    pub commits: Vec<Commit>,
}

/// Узел графа истории: подпись, координаты в мировых точках, дорожка и
/// признак корня (корень — это имя проекта).
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub text: String,
    pub x: f32,
    pub y: f32,
    /// Индекс дорожки: 0 — основная ветка, дальше по порядку.
    pub color: usize,
    pub root: bool,
}

/// Готовый граф истории: узлы и рёбра по их индексам.
///
/// Ребро `(from, to)` направлено от ребёнка (новее) к родителю (старее).
#[derive(Clone, Debug, PartialEq)]
pub struct GraphView {
    pub title: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(usize, usize)>,
}

/// Запускает git в каталоге `dir` и возвращает stdout.
fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| format!("не удалось запустить git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr
            .lines()
            .next()
            .unwrap_or("git завершился с ошибкой")
            .trim()
            .to_owned();
        return Err(if message.is_empty() {
            "git завершился с ошибкой".to_owned()
        } else {
            message
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Читает ветки и коммиты репозитория, в котором лежит `dir`.
pub fn load_repo(dir: &Path) -> Result<Repo, String> {
    let root = run_git(dir, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "выбранная папка не является git-репозиторием".to_owned())?;
    let root = root.trim();
    let name = Path::new(root)
        .file_name()
        .map(|part| part.to_string_lossy().trim().to_owned())
        .filter(|part| !part.is_empty())
        .unwrap_or_else(|| "Git".to_owned());

    let head = run_git(dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());

    let head_commit = run_git(dir, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());

    let branches = parse_branches(&run_git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)\t%(objectname)\t%(*objectname)\t%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    )?);

    // В репозитории без коммитов `git log` падает — это не ошибка чтения.
    let commits = match run_git(
        dir,
        &[
            "log",
            "--all",
            "--date=short",
            "--pretty=format:%H\t%h\t%P\t%s",
        ],
    ) {
        Ok(text) => parse_commits(&text),
        Err(_) => Vec::new(),
    };

    if branches.is_empty() && commits.is_empty() {
        return Err("в репозитории пока нет коммитов".to_owned());
    }

    Ok(Repo {
        name,
        head,
        head_commit,
        branches,
        commits,
    })
}

/// Разбирает вывод `for-each-ref` (поля разделены табуляцией).
fn parse_branches(text: &str) -> Vec<Branch> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let Some(reference) = fields.next() else {
            continue;
        };
        let Some(object) = fields.next() else {
            continue;
        };
        let peeled = fields.next().unwrap_or("");
        let is_head = fields
            .next()
            .map(|flag| flag.trim() == "*")
            .unwrap_or(false);

        let (name, is_remote) = if let Some(name) = reference.strip_prefix("refs/heads/") {
            (name, false)
        } else if let Some(name) = reference.strip_prefix("refs/remotes/") {
            (name, true)
        } else {
            continue;
        };
        // Символическая ссылка origin/HEAD не ветка.
        if name.is_empty() || name.ends_with("/HEAD") {
            continue;
        }

        let head = if peeled.trim().is_empty() {
            object.trim().to_owned()
        } else {
            peeled.trim().to_owned()
        };
        if head.is_empty() {
            continue;
        }

        out.push(Branch {
            name: name.to_owned(),
            head,
            is_remote,
            is_head,
        });
    }
    out
}

/// Разбирает вывод `git log` с табуляцией между полями.
fn parse_commits(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\t');
            let hash = fields.next()?.trim();
            let short = fields.next()?.trim();
            let parents = fields.next().unwrap_or("").trim();
            let subject = fields.next().unwrap_or("").trim();
            if hash.is_empty() || short.is_empty() {
                return None;
            }
            Some(Commit {
                hash: hash.to_owned(),
                short: short.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

/// Подписи веток, указывающих на коммит (как `git log --decorate`).
fn decoration(repo: &Repo, hash: &str) -> String {
    let names: Vec<String> = repo
        .branches
        .iter()
        .filter(|branch| branch.head == hash)
        .map(|branch| {
            let kind = if branch.is_remote {
                " · удалённая"
            } else {
                ""
            };
            let marker = if branch.is_head { "* " } else { "" };
            format!("{marker}{}{kind}", branch.name)
        })
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!(" ({})", names.join(", "))
    }
}

/// Подпись коммита: короткий хэш и тема.
fn commit_label(commit: &Commit) -> String {
    let subject = commit.subject.trim();
    if subject.is_empty() {
        format!("{} (без сообщения)", commit.short)
    } else {
        format!("{} {subject}", commit.short)
    }
}

/// Насколько ветка подходит на роль основной: чем меньше, тем важнее.
fn main_rank(branch: &Branch, head: Option<&str>) -> u8 {
    let tail = branch
        .name
        .rsplit('/')
        .next()
        .unwrap_or(branch.name.as_str());
    if !branch.is_remote && (branch.name == "main" || branch.name == "master") {
        0
    } else if Some(branch.name.as_str()) == head {
        1
    } else if tail == "main" || tail == "master" {
        2
    } else if !branch.is_remote {
        3
    } else {
        4
    }
}

/// Основная ветка («мастер-путь»).
fn pick_main(repo: &Repo) -> Option<&Branch> {
    repo.branches
        .iter()
        .min_by_key(|branch| main_rank(branch, repo.head.as_deref()))
}

/// Остальные ветки: сначала локальные, затем удалённые, внутри — по имени.
fn other_branches<'a>(repo: &'a Repo, main: Option<&Branch>) -> Vec<&'a Branch> {
    let skip = main.map(|branch| branch.name.as_str());
    let mut locals: Vec<&Branch> = repo
        .branches
        .iter()
        .filter(|branch| !branch.is_remote && Some(branch.name.as_str()) != skip)
        .collect();
    let mut remotes: Vec<&Branch> = repo
        .branches
        .iter()
        .filter(|branch| branch.is_remote && Some(branch.name.as_str()) != skip)
        .collect();
    locals.sort_by(|a, b| a.name.cmp(&b.name));
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    locals.extend(remotes);
    locals
}

/// Длина самого длинного пути от корня истории до коммита `hash`.
///
/// По ней считается колонка (ось X): чем позже появился коммит, тем правее
/// он стоит, а merge-коммиты оказываются правее обоих своих родителей.
fn generation(
    hash: &str,
    index: &HashMap<&str, usize>,
    commits: &[Commit],
    memo: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&known) = memo.get(hash) {
        return known;
    }
    // Заглушка от возможных циклов: настоящая история — DAG.
    memo.insert(hash.to_owned(), 0);
    let mut result = 0;
    if let Some(&i) = index.get(hash) {
        for parent in &commits[i].parents {
            result = result.max(generation(parent, index, commits, memo) + 1);
        }
    }
    memo.insert(hash.to_owned(), result);
    result
}

/// Раскладывает историю в граф «слева направо»: корень — имя проекта,
/// основная ветка идёт по дорожке 0, остальные ветки — по своим дорожкам
/// выше/ниже. Рёбра идут от коммита к его родителям, поэтому линии слияния
/// действительно сходятся в merge-коммит.
pub fn build_graph(repo: &Repo) -> GraphView {
    let index: HashMap<&str, usize> = repo
        .commits
        .iter()
        .enumerate()
        .map(|(i, commit)| (commit.hash.as_str(), i))
        .collect();
    let commit_at = |hash: &str| index.get(hash).map(|&i| &repo.commits[i]);

    let main = pick_main(repo);

    // Мастер-путь: цепочка первых родителей от верхушки основной ветки.
    let start = main
        .map(|branch| branch.head.clone())
        .or_else(|| repo.head_commit.clone());
    let mut spine: Vec<String> = Vec::new();
    if let Some(start) = start {
        let mut cursor = start;
        while let Some(commit) = commit_at(&cursor) {
            spine.push(commit.hash.clone());
            match commit.parents.first() {
                Some(parent) => cursor = parent.clone(),
                None => break,
            }
        }
        spine.reverse();
    }

    // Дорожка для каждого коммита. 0 — мастер-путь, дальше — остальные ветки.
    let mut lane: HashMap<String, usize> = HashMap::new();
    for hash in &spine {
        lane.insert(hash.clone(), 0);
    }
    let mut next_lane = if spine.is_empty() { 0 } else { 1 };

    for branch in other_branches(repo, main) {
        let mut own: Vec<String> = Vec::new();
        let mut cursor = branch.head.clone();
        while let Some(commit) = commit_at(&cursor) {
            if lane.contains_key(&commit.hash) {
                break;
            }
            own.push(commit.hash.clone());
            match commit.parents.first() {
                Some(parent) => cursor = parent.clone(),
                None => break,
            }
        }
        if own.is_empty() {
            continue;
        }
        for hash in own {
            lane.insert(hash, next_lane);
        }
        next_lane += 1;
    }

    // Коммиты, до которых не добралась ни одна ветка (например, из удалённых
    // веток), тоже раскладываем цепочками по своим дорожкам.
    let orphan: HashSet<String> = repo
        .commits
        .iter()
        .map(|commit| commit.hash.clone())
        .filter(|hash| !lane.contains_key(hash))
        .collect();
    if !orphan.is_empty() {
        let mut has_child: HashSet<String> = HashSet::new();
        for commit in &repo.commits {
            if !orphan.contains(&commit.hash) {
                continue;
            }
            if let Some(parent) = commit.parents.first()
                && orphan.contains(parent)
            {
                has_child.insert(parent.clone());
            }
        }
        let mut tips: Vec<String> = orphan
            .iter()
            .filter(|hash| !has_child.contains(*hash))
            .cloned()
            .collect();
        tips.sort();
        for tip in tips {
            let mut cursor = tip;
            let mut used = false;
            while let Some(commit) = commit_at(&cursor) {
                if lane.contains_key(&commit.hash) {
                    break;
                }
                lane.insert(commit.hash.clone(), next_lane);
                used = true;
                match commit.parents.first() {
                    Some(parent) => cursor = parent.clone(),
                    None => break,
                }
            }
            if used {
                next_lane += 1;
            }
        }
    }

    // Поколение (колонка) каждого коммита.
    let mut gen_memo: HashMap<String, usize> = HashMap::new();
    for commit in &repo.commits {
        generation(&commit.hash, &index, &repo.commits, &mut gen_memo);
    }

    // Порядок узлов: слева направо, сверху вниз.
    let mut ordered: Vec<&Commit> = repo.commits.iter().collect();
    ordered.sort_by(|a, b| {
        let ga = gen_memo.get(&a.hash).copied().unwrap_or(0);
        let gb = gen_memo.get(&b.hash).copied().unwrap_or(0);
        ga.cmp(&gb)
            .then(
                lane.get(&a.hash)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&lane.get(&b.hash).copied().unwrap_or(0)),
            )
            .then(a.hash.cmp(&b.hash))
    });

    let truncated = ordered.len() > MAX_GRAPH_COMMITS;
    if truncated {
        // Оставляем самый старый коммит (чтобы корень не потерялся) и самые
        // новые — этого достаточно для ориентира по истории.
        let oldest = ordered[0];
        let tail = ordered.len() - (MAX_GRAPH_COMMITS - 1);
        ordered = std::iter::once(oldest)
            .chain(ordered[tail..].iter().copied())
            .collect();
    }

    let lane_count = lane.values().copied().max().unwrap_or(0) + 1;
    let y_center = (lane_count.saturating_sub(1)) as f32 * GRAPH_LANE_H * 0.5;

    // Узел 0 — корень-проект, он стоит на дорожке основной ветки.
    let mut nodes: Vec<GraphNode> = vec![GraphNode {
        text: repo.name.clone(),
        x: 0.0,
        y: -y_center,
        color: 0,
        root: true,
    }];
    let mut node_index: HashMap<&str, usize> = HashMap::new();

    for commit in &ordered {
        let column = gen_memo.get(&commit.hash).copied().unwrap_or(0);
        let commit_lane = lane.get(&commit.hash).copied().unwrap_or(0);
        let mut text = commit_label(commit);
        let deco = decoration(repo, &commit.hash);
        if !deco.is_empty() {
            text.push_str(&deco);
        }
        node_index.insert(commit.hash.as_str(), nodes.len());
        nodes.push(GraphNode {
            text,
            x: (column + 1) as f32 * GRAPH_COL_W,
            y: commit_lane as f32 * GRAPH_LANE_H - y_center,
            color: commit_lane,
            root: false,
        });
    }

    // Рёбра: от коммита к родителям. У коммитов без показанных родителей
    // ребро идёт к корню-проекту, чтобы не было висящих узлов.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for commit in &ordered {
        let Some(&from) = node_index.get(commit.hash.as_str()) else {
            continue;
        };
        let mut has_shown_parent = false;
        for parent in &commit.parents {
            if let Some(&to) = node_index.get(parent.as_str()) {
                edges.push((from, to));
                has_shown_parent = true;
            }
        }
        if !has_shown_parent {
            edges.push((from, 0));
        }
    }

    let title = if truncated {
        format!(
            "{} · показаны последние {} коммитов",
            repo.name,
            ordered.len()
        )
    } else {
        repo.name.clone()
    };

    GraphView {
        title,
        nodes,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str], subject: &str) -> Commit {
        Commit {
            hash: hash.to_owned(),
            short: hash.chars().take(7).collect(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            subject: subject.to_owned(),
        }
    }

    fn repo_with_merge() -> Repo {
        Repo {
            name: "demo".to_owned(),
            head: Some("main".to_owned()),
            head_commit: Some("m".to_owned()),
            branches: vec![
                Branch {
                    name: "main".to_owned(),
                    head: "m".to_owned(),
                    is_remote: false,
                    is_head: true,
                },
                Branch {
                    name: "feature".to_owned(),
                    head: "b2".to_owned(),
                    is_remote: false,
                    is_head: false,
                },
            ],
            commits: vec![
                commit("m", &["a2", "b2"], "Merge branch 'feature'"),
                commit("a2", &["a1"], "work on main"),
                commit("a1", &["root"], "start"),
                commit("b2", &["b1"], "feature two"),
                commit("b1", &["root"], "feature one"),
                commit("root", &[], "initial"),
            ],
        }
    }

    #[test]
    fn parse_branches_reads_local_and_remote_refs() {
        let text = concat!(
            "refs/heads/main\te3193d4\t\t*\n",
            "refs/heads/feature\t6b95c0b\t\t \n",
            "refs/remotes/origin/main\te3193d4\t\t \n",
            "refs/remotes/origin/HEAD\t9c0ffee\t\t \n",
        );
        let branches = parse_branches(text);
        assert_eq!(branches.len(), 3);
        assert!(branches[0].is_head && !branches[0].is_remote);
        assert!(branches[2].is_remote && branches[2].name == "origin/main");
    }

    #[test]
    fn parse_commits_splits_parents_and_subject() {
        let text = "abc1234\tabc1234\tdef5678 9999aaa\tMerge two branches\n\
                    def5678\tdef5678\t\tfirst commit";
        let commits = parse_commits(text);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, vec!["def5678", "9999aaa"]);
        assert_eq!(commits[0].subject, "Merge two branches");
        assert!(commits[1].parents.is_empty());
    }

    fn node_index(graph: &GraphView, prefix: &str) -> usize {
        graph
            .nodes
            .iter()
            .position(|node| node.text.starts_with(prefix))
            .unwrap_or_else(|| panic!("нет узла {prefix:?}"))
    }

    #[test]
    fn graph_has_project_root_and_straight_main_lane() {
        let graph = build_graph(&repo_with_merge());
        assert_eq!(graph.title, "demo");
        assert!(graph.nodes[0].root);
        assert_eq!(graph.nodes[0].text, "demo");
        // коммиты основной ветки — все на дорожке 0
        for prefix in [
            "root initial",
            "a1 start",
            "a2 work on main",
            "m Merge branch",
        ] {
            assert_eq!(graph.nodes[node_index(&graph, prefix)].color, 0);
        }
        // порядок слева направо: старый коммит левее нового
        let root = node_index(&graph, "root initial");
        let merge = node_index(&graph, "m Merge branch");
        assert!(graph.nodes[root].x < graph.nodes[merge].x);
    }

    #[test]
    fn graph_branch_gets_its_own_lane_and_converges() {
        let graph = build_graph(&repo_with_merge());
        let merge = node_index(&graph, "m Merge branch");
        let a2 = node_index(&graph, "a2 work on main");
        let b1 = node_index(&graph, "b1 feature one");
        let b2 = node_index(&graph, "b2 feature two");
        assert_eq!(graph.nodes[b1].color, 1);
        assert_eq!(graph.nodes[b2].color, 1);
        // линии слияния сходятся в merge-коммит: рёбра merge -> родители
        assert!(graph.edges.contains(&(merge, a2)));
        assert!(graph.edges.contains(&(merge, b2)));
        // ветка выше/ниже основной
        assert!(graph.nodes[b2].y != graph.nodes[merge].y);
    }

    #[test]
    fn graph_links_root_commit_to_project_root() {
        let graph = build_graph(&repo_with_merge());
        let root_commit = node_index(&graph, "root initial");
        assert!(graph.edges.contains(&(root_commit, 0)));
    }

    #[test]
    fn graph_decorates_branch_refs() {
        let mut repo = repo_with_merge();
        repo.branches.push(Branch {
            name: "origin/main".to_owned(),
            head: "m".to_owned(),
            is_remote: true,
            is_head: false,
        });
        let graph = build_graph(&repo);
        let merge = &graph.nodes[node_index(&graph, "m Merge branch")];
        assert!(merge.text.contains("(* main"));
        assert!(merge.text.contains("origin/main · удалённая"));
    }

    #[test]
    fn graph_puts_orphan_commits_on_their_own_lane() {
        let mut repo = repo_with_merge();
        repo.commits.push(commit("zz", &["root"], "lost work"));
        let graph = build_graph(&repo);
        let orphan = &graph.nodes[node_index(&graph, "zz lost work")];
        assert!(orphan.color > 0);
    }
}
