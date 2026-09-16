use anyhow::Result;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    All,
    Each,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Yes,
    No,
    Quit,
}

pub trait Prompts {
    fn mode(&mut self, label: &str) -> Result<Mode>;
    fn item(&mut self, label: &str) -> Result<Answer>;
}

pub struct TerminalPrompts;

fn read_line(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        println!();
        return Ok(None);
    }
    Ok(Some(input.trim().to_lowercase()))
}

impl Prompts for TerminalPrompts {
    fn mode(&mut self, label: &str) -> Result<Mode> {
        println!("  {label}：1) 全部删除  2) 逐个确认  3) 跳过（默认）");
        loop {
            match read_line("  请选择 [1/2/3，默认 3]: ")?.as_deref() {
                Some("1") => return Ok(Mode::All),
                Some("2") => return Ok(Mode::Each),
                Some("3" | "") | None => return Ok(Mode::Skip),
                _ => println!("  请输入 1、2 或 3。"),
            }
        }
    }

    fn item(&mut self, label: &str) -> Result<Answer> {
        loop {
            match read_line(&format!("  删除 {label}？[y/N/q，q 跳过本组剩余]: "))?.as_deref()
            {
                Some("y" | "yes") => return Ok(Answer::Yes),
                Some("n" | "no" | "") => return Ok(Answer::No),
                Some("q") | None => return Ok(Answer::Quit),
                _ => println!("  请输入 y、n 或 q。"),
            }
        }
    }
}
