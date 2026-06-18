//! CLI commands for `ralph tools handoff` namespace (2026-06-18-002 plan U2).
//!
//! Subcommands:
//! - `prepare`: 给上游 hat 一个**确定性**的 `handoff_path` 与
//!   五段式 skeleton。agent 拿到路径后填写 → emit 时 payload
//!   带 `handoff_path` → gate accept。
//!
//! 关键契约(KTD-13/KTD-14):
//! - 文件名 `seq` 由 `LoopState.hat_handoff_seq + 1` 决定,agent
//!   不手猜。
//! - 同 path 默认不覆盖(`--force` 才覆盖);不可写已 accept 的旧 seq。

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ralph_core::hat_handoff::allocator::{
    self, PrepareInputs, WriteOutcome, compute, write_skeleton,
};
use serde::Serialize;
use std::path::PathBuf;

use crate::operation_guard::OperationContext;

/// `ralph tools handoff` 子命令组。
#[derive(Parser, Debug)]
pub struct HandoffArgs {
    #[command(subcommand)]
    pub command: HandoffCommands,

    /// Working directory (default: current directory).
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum HandoffCommands {
    /// Compute `handoff_path` + skeleton for the next macro-edge emit
    /// and (unless `--no-write`) write the skeleton to disk.
    Prepare(PrepareArgs),
}

/// `ralph tools handoff prepare` 参数。
#[derive(Parser, Debug)]
pub struct PrepareArgs {
    /// Upstream hat id (the emit hat). 必须存在于 preset 中。
    #[arg(long)]
    pub from: String,

    /// Downstream hat id (the consumer of the topic).
    #[arg(long)]
    pub to: String,

    /// Topic of the macro edge being emitted (informational only;
    /// validation against the unique-consumer index happens at
    /// runtime in the gate).
    #[arg(long)]
    pub topic: String,

    /// Current loop iteration (1-indexed). Defaults to 1 when not
    /// running inside a loop context.
    #[arg(long, default_value_t = 1)]
    pub iteration: u32,

    /// Current `LoopState.hat_handoff_seq`. Defaults to 0 (no
    /// handoff accepted in this iteration yet).
    #[arg(long, default_value_t = 0)]
    pub current_seq: u32,

    /// Overwrite the same `handoff_path` if it already exists.
    /// Required for the KTD-14 retry path (reject → fix → same path).
    #[arg(long)]
    pub force: bool,

    /// Compute and print without writing to disk.
    #[arg(long)]
    pub no_write: bool,

    /// Output JSON instead of human-readable.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct PrepareOutputJson {
    handoff_path: String,
    seq: u32,
    iteration: u32,
    from: String,
    to: String,
    topic: String,
    written: bool,
}

/// 执行 `ralph tools handoff ...`。
pub fn execute(args: HandoffArgs) -> Result<()> {
    let root = resolve_root(args.root)?;
    let _ctx = OperationContext::detect(root.clone());

    match args.command {
        HandoffCommands::Prepare(p) => execute_prepare(&root, &p),
    }
}

fn execute_prepare(root: &std::path::Path, args: &PrepareArgs) -> Result<()> {
    let inputs = PrepareInputs {
        iteration: args.iteration,
        current_seq: args.current_seq,
        from: &args.from,
        to: &args.to,
        topic: &args.topic,
    };
    let result = compute(&inputs);

    let written = if args.no_write {
        false
    } else {
        match write_skeleton(root, &result.handoff_path, &result.skeleton, args.force)
            .with_context(|| format!("writing handoff file at {}", result.handoff_path))?
        {
            WriteOutcome::Written => true,
            WriteOutcome::AlreadyExists => false,
        }
    };

    if args.json {
        let payload = PrepareOutputJson {
            handoff_path: result.handoff_path.clone(),
            seq: result.seq,
            iteration: args.iteration,
            from: args.from.clone(),
            to: args.to.clone(),
            topic: args.topic.clone(),
            written,
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("handoff_path: {}", result.handoff_path);
        println!("seq:          {}", result.seq);
        println!("iteration:    {}", args.iteration);
        println!(
            "from → to:    {} → {}",
            allocator::sanitize(&args.from),
            allocator::sanitize(&args.to)
        );
        println!("topic:        {}", args.topic);
        println!("written:      {written}");
        if !written && !args.no_write {
            eprintln!(
                "(file already exists; pass --force to overwrite. KTD-14 retry path.)"
            );
        }
    }
    Ok(())
}

fn resolve_root(root: Option<PathBuf>) -> Result<PathBuf> {
    let resolved = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn prepare_args_parse() {
        let args = HandoffArgs::parse_from([
            "handoff",
            "prepare",
            "--from",
            "executor",
            "--to",
            "review-coordinator",
            "--topic",
            "work.done",
            "--iteration",
            "3",
            "--current-seq",
            "1",
            "--json",
        ]);
        match args.command {
            HandoffCommands::Prepare(p) => {
                assert_eq!(p.from, "executor");
                assert_eq!(p.to, "review-coordinator");
                assert_eq!(p.topic, "work.done");
                assert_eq!(p.iteration, 3);
                assert_eq!(p.current_seq, 1);
                assert!(p.json);
                assert!(!p.force);
                assert!(!p.no_write);
            }
        }
    }

    #[test]
    fn execute_prepare_writes_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        let args = PrepareArgs {
            from: "executor".into(),
            to: "review-coordinator".into(),
            topic: "work.done".into(),
            iteration: 3,
            current_seq: 1,
            force: false,
            no_write: false,
            json: false,
        };
        execute_prepare(dir.path(), &args).unwrap();
        let path = dir.path().join(".ralph/agent/hat-handoff/3-2-executor-review_coordinator.md");
        assert!(path.exists(), "skeleton file must exist at {path:?}");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Handoff: executor → review-coordinator"));
        assert!(content.contains("## context"));
        assert!(content.contains("## next"));
    }
}
