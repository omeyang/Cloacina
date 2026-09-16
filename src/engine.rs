use crate::cli::EngineChoice;
use crate::command::{Invocation, Runner, checked, which};
use crate::prompt::{Answer, Mode, Prompts};
use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Podman,
    Docker,
}

#[derive(Debug)]
pub struct Engine {
    pub kind: Kind,
    program: PathBuf,
    socket: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub names: Vec<String>,
    pub detail: String,
}

impl Item {
    fn label(&self) -> String {
        let id = self.id.strip_prefix("sha256:").unwrap_or(&self.id);
        format!(
            "{}  {}  [{}]",
            &id[..id.len().min(12)],
            self.names.join(" "),
            self.detail
        )
    }

    fn references(&self) -> Vec<String> {
        let tags: Vec<_> = self
            .names
            .iter()
            .filter(|name| name.as_str() != "<none>:<none>")
            .cloned()
            .collect();
        if tags.is_empty() {
            vec![self.id.clone()]
        } else {
            tags
        }
    }
}

fn inventory(text: &str, images: bool) -> Result<Vec<Item>> {
    let mut items: Vec<Item> = Vec::new();
    let mut positions = HashMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let parts: Vec<_> = line.split('|').collect();
        if parts.len() < 3 || !valid_id(parts[0]) {
            bail!("容器引擎返回了无法识别的清单：{line}");
        }
        let id = parts[0].to_owned();
        if images && positions.contains_key(&id) {
            let position: usize = positions[&id];
            if !items[position].names.iter().any(|name| name == parts[1]) {
                items[position].names.push(parts[1].to_owned());
            }
        } else {
            positions.insert(id.clone(), items.len());
            items.push(Item {
                id,
                names: vec![parts[1].to_owned()],
                detail: parts[2..].join(" | "),
            });
        }
    }
    Ok(items)
}

fn valid_id(id: &str) -> bool {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn local_socket(endpoint: String) -> Result<String> {
    let path = endpoint
        .strip_prefix("unix://")
        .context("Docker 当前连接不是本机 Unix socket，已跳过；请切换到本机 context")?;
    if !Path::new(path).is_absolute() || path == "/" {
        bail!("Docker Unix socket 路径无效");
    }
    Ok(endpoint)
}

pub fn discover(choice: EngineChoice, runner: &dyn Runner) -> (Vec<Engine>, Vec<String>) {
    let mut engines = Vec::new();
    let mut notices = Vec::new();
    let podman = which("podman");
    let docker = which("docker");
    if matches!(choice, EngineChoice::Auto | EngineChoice::Podman) {
        if let Some(program) = &podman {
            engines.push(Engine {
                kind: Kind::Podman,
                program: program.clone(),
                socket: None,
            });
        } else if matches!(choice, EngineChoice::Podman) {
            notices.push("未安装 Podman".into());
        }
    }
    if matches!(choice, EngineChoice::Auto | EngineChoice::Docker) {
        if let Some(program) = docker {
            let shim = podman
                .as_ref()
                .is_some_and(|p| p.canonicalize().ok() == program.canonicalize().ok())
                || checked(runner, &Invocation::new(&program, &["--version"]))
                    .is_ok_and(|version| version.to_lowercase().contains("podman"));
            if shim {
                if engines.is_empty() {
                    engines.push(Engine {
                        kind: Kind::Podman,
                        program,
                        socket: None,
                    });
                }
            } else {
                let endpoint = (|| -> Result<String> {
                    let context = std::env::var("DOCKER_CONTEXT")
                        .ok()
                        .filter(|s| !s.is_empty());
                    if context.is_none() {
                        if let Ok(host) = std::env::var("DOCKER_HOST") {
                            if !host.is_empty() {
                                return local_socket(host);
                            }
                        }
                    }
                    let mut args = vec!["context", "inspect"];
                    if let Some(context) = &context {
                        args.push(context);
                    }
                    args.extend(["--format", "{{json .Endpoints.docker.Host}}"]);
                    let data = checked(runner, &Invocation::new(&program, &args))?;
                    local_socket(serde_json::from_str::<String>(&data)?)
                })();
                match endpoint {
                    Ok(socket) => engines.push(Engine {
                        kind: Kind::Docker,
                        program,
                        socket: Some(socket),
                    }),
                    Err(error) => notices.push(format!("Docker：{error:#}")),
                }
            }
        } else if matches!(choice, EngineChoice::Docker) {
            notices.push("未安装 Docker".into());
        }
    }
    (engines, notices)
}

impl Engine {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::Podman => "Podman",
            Kind::Docker => "Docker",
        }
    }

    fn command(&self, args: &[&str]) -> Invocation {
        let mut prefix = match self.kind {
            Kind::Podman => vec!["--remote=false"],
            Kind::Docker => vec![
                "--host",
                self.socket
                    .as_deref()
                    .expect("Docker requires local socket"),
            ],
        };
        prefix.extend_from_slice(args);
        let mut command = Invocation::new(&self.program, &prefix);
        if self.kind == Kind::Docker {
            command.remove_env = vec![
                "DOCKER_HOST",
                "DOCKER_CONTEXT",
                "DOCKER_TLS_VERIFY",
                "DOCKER_CERT_PATH",
            ];
        }
        command
    }

    fn items(&self, runner: &dyn Runner, images: bool) -> Result<Vec<Item>> {
        let args = if images {
            vec![
                "images",
                "--all",
                "--no-trunc",
                "--format",
                "{{.ID}}|{{.Repository}}:{{.Tag}}|{{.Size}}",
            ]
        } else {
            vec![
                "ps",
                "--all",
                "--no-trunc",
                "--format",
                "{{.ID}}|{{.Names}}|{{.Status}}|{{.Image}}",
            ]
        };
        inventory(&checked(runner, &self.command(&args))?, images)
    }

    fn in_use(&self, runner: &dyn Runner, image: &Item) -> Result<bool> {
        let id = image.id.strip_prefix("sha256:").unwrap_or(&image.id);
        let filter = format!("ancestor={id}");
        Ok(!checked(
            runner,
            &self.command(&["ps", "--all", "--quiet", "--filter", &filter]),
        )?
        .is_empty())
    }

    fn remove_image(&self, runner: &dyn Runner, image: &Item) -> Result<()> {
        if self.in_use(runner, image)? {
            bail!("镜像仍被容器引用，保留镜像及标签");
        }
        let references = image.references();
        let mut args = vec!["rmi", "--no-prune"];
        args.extend(references.iter().map(String::as_str));
        // No --force: Podman would also remove containers. No parent pruning in either mode.
        checked(runner, &self.command(&args))?;
        Ok(())
    }

    pub fn cleanup(&self, runner: &dyn Runner, prompts: &mut dyn Prompts, dry_run: bool) -> usize {
        let mut errors = 0;
        for images in [false, true] {
            if let Err(error) = self.group(runner, prompts, dry_run, images) {
                eprintln!("  {}：{error:#}", self.name());
                errors += 1;
            }
        }
        errors
    }

    fn group(
        &self,
        runner: &dyn Runner,
        prompts: &mut dyn Prompts,
        dry_run: bool,
        images: bool,
    ) -> Result<()> {
        let label = if images { "镜像" } else { "容器" };
        println!("\n▸ 本机 {} {label}", self.name());
        let items = self.items(runner, images)?;
        if items.is_empty() {
            println!("  没有{label}。");
            return Ok(());
        }
        for item in &items {
            println!("  {}", item.label());
        }
        if dry_run {
            println!("  以上为清理候选；实际运行时交互选择。");
            return Ok(());
        }
        if images {
            println!("  同一 ID 的所有标签一起处理；被保留容器引用的镜像也会保留。");
        } else {
            println!("  删除运行中的容器时会停止该容器。");
        }
        let selected = select(&items, label, prompts)?;
        if selected.is_empty() {
            println!("  已跳过{label}清理。");
            return Ok(());
        }
        if images {
            self.remove_images(runner, &selected)
        } else {
            let mut args = vec!["rm", "--force"];
            args.extend(selected.iter().map(|item| item.id.as_str()));
            checked(runner, &self.command(&args))?;
            println!("  已删除 {} 个容器。", selected.len());
            Ok(())
        }
    }

    fn remove_images(&self, runner: &dyn Runner, selected: &[Item]) -> Result<()> {
        let mut pending: HashSet<_> = selected.iter().map(|item| item.id.clone()).collect();
        let mut failures = HashMap::new();
        // Intermediate images may precede their children. Retry only selected IDs if a pass
        // removed something; stop when no progress is possible. Never widen the selection.
        while !pending.is_empty() {
            let before = pending.len();
            let current = self.items(runner, true)?;
            let existing: HashSet<_> = current.iter().map(|i| i.id.clone()).collect();
            pending.retain(|id| existing.contains(id));
            for image in current.iter().filter(|image| pending.contains(&image.id)) {
                if let Err(error) = self.remove_image(runner, image) {
                    failures.insert(image.id.clone(), format!("{error:#}"));
                }
            }
            let remaining: HashSet<_> = self
                .items(runner, true)?
                .into_iter()
                .map(|i| i.id)
                .collect();
            pending.retain(|id| remaining.contains(id));
            if pending.len() >= before {
                break;
            }
        }
        println!(
            "  已删除 {} 个镜像，保留 {} 个。",
            selected.len() - pending.len(),
            pending.len()
        );
        if !pending.is_empty() {
            for item in selected.iter().filter(|item| pending.contains(&item.id)) {
                eprintln!(
                    "  保留 {}：{}",
                    item.label(),
                    failures
                        .get(&item.id)
                        .map(String::as_str)
                        .unwrap_or("仍有依赖，无法删除")
                );
            }
            bail!("{} 个选中的镜像未能删除", pending.len());
        }
        Ok(())
    }
}

fn select(items: &[Item], label: &str, prompts: &mut dyn Prompts) -> Result<Vec<Item>> {
    match prompts.mode(label)? {
        Mode::All => Ok(items.to_vec()),
        Mode::Skip => Ok(Vec::new()),
        Mode::Each => {
            let mut selected = Vec::new();
            for item in items {
                match prompts.item(&format!("{label} {}", item.label()))? {
                    Answer::Yes => selected.push(item.clone()),
                    Answer::No => (),
                    Answer::Quit => break,
                }
            }
            Ok(selected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    struct FakeRunner {
        results: RefCell<VecDeque<(Vec<String>, i32, String)>>,
        calls: RefCell<Vec<Invocation>>,
    }
    impl FakeRunner {
        fn new(sequence: &[(&[&str], i32, &str)]) -> Self {
            Self {
                results: RefCell::new(
                    sequence
                        .iter()
                        .map(|(args, code, out)| {
                            (
                                args.iter().map(|s| (*s).into()).collect(),
                                *code,
                                (*out).into(),
                            )
                        })
                        .collect(),
                ),
                calls: RefCell::new(Vec::new()),
            }
        }
    }
    impl Runner for FakeRunner {
        fn run(&self, command: &Invocation) -> Result<Output> {
            self.calls.borrow_mut().push(command.clone());
            let (args, code, stdout) = self
                .results
                .borrow_mut()
                .pop_front()
                .expect("unexpected command");
            let actual: Vec<_> = command
                .args
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            assert_eq!(actual, args);
            Ok(Output {
                status: ExitStatus::from_raw(code << 8),
                stdout: stdout.into_bytes(),
                stderr: Vec::new(),
            })
        }
    }
    fn podman() -> Engine {
        Engine {
            kind: Kind::Podman,
            program: "podman".into(),
            socket: None,
        }
    }

    #[test]
    fn duplicate_tags_are_grouped_once() {
        let items = inventory(
            "sha256:abc|repo:a|10 MB\nsha256:abc|repo:b|10 MB\ndef|<none>:<none>|2 MB",
            true,
        )
        .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].references(), ["repo:a", "repo:b"]);
        assert_eq!(items[1].references(), ["def"]);
        assert!(inventory("--all|bad|bad", true).is_err());
    }

    #[test]
    fn delete_multitag_image_without_force_or_parent_pruning() {
        let runner = FakeRunner::new(&[
            (
                &[
                    "--remote=false",
                    "ps",
                    "--all",
                    "--quiet",
                    "--filter",
                    "ancestor=abc",
                ],
                0,
                "",
            ),
            (
                &["--remote=false", "rmi", "--no-prune", "repo:a", "repo:b"],
                0,
                "",
            ),
        ]);
        let image = inventory("sha256:abc|repo:a|10 MB\nsha256:abc|repo:b|10 MB", true)
            .unwrap()
            .remove(0);
        podman().remove_image(&runner, &image).unwrap();
        assert!(runner.results.borrow().is_empty());
    }

    #[test]
    fn in_use_image_is_not_untagged() {
        let runner = FakeRunner::new(&[(
            &[
                "--remote=false",
                "ps",
                "--all",
                "--quiet",
                "--filter",
                "ancestor=abc",
            ],
            0,
            "123",
        )]);
        let image = inventory("sha256:abc|repo:a|10 MB", true)
            .unwrap()
            .remove(0);
        assert!(podman().remove_image(&runner, &image).is_err());
        assert_eq!(runner.calls.borrow().len(), 1);
    }

    #[test]
    fn docker_is_pinned_to_local_socket_and_environment_is_cleared() {
        assert!(local_socket("ssh://host".into()).is_err());
        assert!(local_socket("tcp://127.0.0.1:2375".into()).is_err());
        assert!(local_socket("unix://relative.sock".into()).is_err());
        let socket = local_socket("unix:///run/user/1000/docker.sock".into()).unwrap();
        let engine = Engine {
            kind: Kind::Docker,
            program: "docker".into(),
            socket: Some(socket),
        };
        let command = engine.command(&["rm", "--force", "abc"]);
        assert_eq!(command.args[0], "--host");
        assert_eq!(command.args[1], "unix:///run/user/1000/docker.sock");
        assert!(command.remove_env.contains(&"DOCKER_CONTEXT"));
    }

    struct Choices {
        mode: Mode,
        answers: VecDeque<Answer>,
    }
    impl Prompts for Choices {
        fn mode(&mut self, _: &str) -> Result<Mode> {
            Ok(self.mode)
        }
        fn item(&mut self, _: &str) -> Result<Answer> {
            Ok(self.answers.pop_front().unwrap())
        }
    }
    #[test]
    fn each_and_quit_only_select_approved_items() {
        let items = inventory("aaa|a|running\nbbb|b|stopped\nccc|c|stopped", false).unwrap();
        let mut choices = Choices {
            mode: Mode::Each,
            answers: [Answer::No, Answer::Yes, Answer::Quit].into(),
        };
        let selected = select(&items, "容器", &mut choices).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "bbb");
        choices.mode = Mode::Skip;
        assert!(select(&items, "容器", &mut choices).unwrap().is_empty());
        choices.mode = Mode::All;
        assert_eq!(select(&items, "容器", &mut choices).unwrap().len(), 3);
    }
}
