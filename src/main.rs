mod cli;
mod command;
mod engine;
mod files;
mod prompt;

use anyhow::Result;
use clap::Parser;
use command::{Invocation, Runner, SystemRunner, checked, which};
use std::io::{self, IsTerminal};
use std::process::ExitCode;

fn external_cleanup(options: &cli::Options, uid: u32, runner: &dyn Runner) -> usize {
    let mut commands = Vec::new();
    if uid != 0 {
        if options.all || options.system || options.deep {
            println!("\n  当前不是 root，跳过系统日志和系统包缓存。");
        }
        return 0;
    }
    if options.deep || options.all {
        if let Some(program) = which("dnf5") {
            commands.push(Invocation::new(program, &["clean", "all"]));
        } else if let Some(program) = which("apt-get") {
            commands.push(Invocation::new(program, &["clean"]));
        } else {
            println!("\n  未检测到 dnf5 / apt-get，跳过系统包缓存。");
        }
    }
    if options.system || options.all {
        if let Some(program) = which("journalctl") {
            commands.push(Invocation::new(
                program,
                &[&format!("--vacuum-time={}d", options.journal_days)],
            ));
        } else {
            println!("\n  未安装 journalctl，跳过 journal 清理。");
        }
    }
    let mut errors = 0;
    if !commands.is_empty() {
        println!("\n▸ 系统包缓存与 journal");
    }
    for command in commands {
        if options.dry_run {
            println!("  将执行 {}", command.display());
        } else {
            match checked(runner, &command) {
                Ok(_) => println!("  已执行 {}", command.display()),
                Err(error) => {
                    eprintln!("  执行失败：{error:#}");
                    errors += 1;
                }
            }
        }
    }
    errors
}

fn run(options: &cli::Options) -> Result<usize> {
    let roots = files::Roots::from_environment()?;
    let runner = SystemRunner;
    let mut errors = 0;
    if !options.skip_containers {
        if !options.dry_run && !io::stdin().is_terminal() {
            println!("▸ 未检测到交互终端，跳过容器和镜像；使用 -n 可预览候选。");
        } else {
            let (engines, notices) = engine::discover(options.engine, &runner);
            for notice in &notices {
                println!("  跳过：{notice}");
            }
            if engines.is_empty() {
                println!("▸ 没有可清理的本机容器引擎。");
            }
            if !matches!(options.engine, cli::EngineChoice::Auto) && engines.is_empty() {
                errors += 1;
            }
            for engine in engines {
                errors += engine.cleanup(&runner, &mut prompt::TerminalPrompts, options.dry_run);
            }
        }
    }
    if !options.containers_only {
        let plan = files::Plan::build(&roots, options)?;
        errors += plan.execute(&roots, options);
        errors += external_cleanup(options, roots.uid, &runner);
    }
    if options.dry_run {
        println!("\n预演结束，未实际删除。去掉 -n 后执行。");
    }
    if errors > 0 {
        eprintln!("\n有 {errors} 处清理未完成，详情见上方输出。");
    }
    Ok(errors)
}

fn main() -> ExitCode {
    let options = cli::Options::parse();
    match run(&options) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("cleanup: {error:#}");
            ExitCode::FAILURE
        }
    }
}
