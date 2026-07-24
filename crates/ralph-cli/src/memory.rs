//! CLI commands for the `ralph memory` namespace.
//!
//! Provides subcommands for managing persistent memories:
//! - `add`: Store a new memory
//! - `list`: List all memories
//! - `show`: Show a single memory by ID
//! - `delete`: Delete a memory by ID
//! - `search`: Find memories by query
//! - `prime`: Output memories for context injection
//! - `init`: Initialize memories file

use crate::operation_guard::OperationContext;
use crate::resolve_workspace_root;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::{
    MarkdownMemoryStore, Memory, MemoryType, MemoryVisibility, truncate_with_ellipsis,
};
use std::path::PathBuf;

/// ANSI color codes for terminal output.
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const GREEN: &str = "\x1b[32m";
    pub const CYAN: &str = "\x1b[36m";
    pub const MAGENTA: &str = "\x1b[35m";
}

/// Format a date string as a human-readable relative time.
fn format_relative_date(date_str: &str) -> String {
    format_relative_date_with_today(date_str, chrono::Utc::now().date_naive())
}

fn format_relative_date_with_today(date_str: &str, today: chrono::NaiveDate) -> String {
    let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
        return date_str.to_string();
    };

    let days = (today - date).num_days();

    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        2..=6 => format!("{} days ago", days),
        7..=13 => "1 week ago".to_string(),
        14..=20 => "2 weeks ago".to_string(),
        21..=27 => "3 weeks ago".to_string(),
        28..=44 => "1 month ago".to_string(),
        45..=89 => "2 months ago".to_string(),
        _ => {
            let months = days / 30;
            if months < 12 {
                format!("{} months ago", months)
            } else {
                date_str.to_string()
            }
        }
    }
}

/// Output format for memory commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
    /// Markdown format (for prime command)
    Markdown,
    /// ID-only output for scripting
    Quiet,
}

/// Memory management commands for persistent learning across sessions.
#[derive(Parser, Debug)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub command: MemoryCommands,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommands {
    /// Store a new memory
    Add(AddArgs),

    /// List all memories
    List(ListArgs),

    /// Show a single memory by ID
    Show(ShowArgs),

    /// Delete a memory by ID
    Delete(DeleteArgs),

    /// Find memories by query
    Search(SearchArgs),

    /// Output memories for context injection
    Prime(PrimeArgs),

    /// Initialize memories file
    Init(InitArgs),
}

/// Arguments for the `memory add` command.
#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The memory content to store
    pub content: String,

    /// Memory type
    #[arg(short = 't', long, default_value = "pattern")]
    pub r#type: MemoryType,

    /// Comma-separated tags
    #[arg(long)]
    pub tags: Option<String>,

    /// Mark this memory as private to the current hat (agent context only)
    #[arg(long)]
    pub private: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `memory list` command.
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Filter by memory type
    #[arg(short = 't', long)]
    pub r#type: Option<MemoryType>,

    /// Show only last N memories
    #[arg(long)]
    pub last: Option<usize>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `memory show` command.
#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// Memory ID (e.g., mem-1737372000-a1b2)
    pub id: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `memory delete` command.
#[derive(Parser, Debug)]
pub struct DeleteArgs {
    /// Memory ID to delete
    pub id: String,
}

/// Arguments for the `memory search` command.
#[derive(Parser, Debug)]
pub struct SearchArgs {
    /// Search query (fuzzy match on content/tags)
    pub query: Option<String>,

    /// Filter by memory type
    #[arg(short = 't', long)]
    pub r#type: Option<MemoryType>,

    /// Filter by tags (comma-separated, OR logic)
    #[arg(long)]
    pub tags: Option<String>,

    /// Show all results (no limit)
    #[arg(long)]
    pub all: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `memory prime` command.
#[derive(Parser, Debug)]
pub struct PrimeArgs {
    /// Maximum tokens to include (0 = unlimited)
    #[arg(long)]
    pub budget: Option<usize>,

    /// Filter by types (comma-separated)
    #[arg(short = 't', long)]
    pub r#type: Option<String>,

    /// Filter by tags (comma-separated)
    #[arg(long)]
    pub tags: Option<String>,

    /// Only memories from last N days
    #[arg(long)]
    pub recent: Option<u32>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
    pub format: OutputFormat,
}

/// Arguments for the `memory init` command.
#[derive(Parser, Debug)]
pub struct InitArgs {
    /// Overwrite existing file
    #[arg(long)]
    pub force: bool,
}

/// Maximum characters per memory content (rejected on add).
const MAX_MEMORY_CONTENT_CHARS: usize = 10_000;

/// Maximum private memories per hat id (rejected on add).
const MAX_PRIVATE_MEMORIES_PER_HAT: usize = 1_000;

/// Build an `OperationContext` for the current invocation.
fn operation_context_for(root: Option<&PathBuf>) -> OperationContext {
    OperationContext::detect(resolve_workspace_root(root))
}

/// Authorize a mutation against a memory, taking visibility rules
/// into account.
///
/// In agent context:
/// - **Shared** memories are immutable; only the human CLI may mutate them.
/// - **Private** memories may be mutated only by their `owner_hat_id`.
///
/// In human context this is a no-op (humans have full diagnostic access).
fn authorize_memory_action(
    memory: Option<&Memory>,
    ctx: &OperationContext,
    operation: &str,
) -> Result<()> {
    let Some(memory) = memory else {
        return Ok(());
    };
    if !ctx.is_agent_context {
        return Ok(());
    }
    match memory.visibility {
        MemoryVisibility::Shared => bail!(
            "{operation}: agent context cannot mutate shared memory '{id}' (human CLI required)",
            id = memory.id
        ),
        MemoryVisibility::Private => {
            let caller = ctx.current_hat_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "{operation}: agent context requires a current hat (set RALPH_CURRENT_HAT)"
                )
            })?;
            if memory.owner_hat_id.as_deref() != Some(caller) {
                bail!(
                    "{operation}: private memory '{id}' is owned by '{owner}' but caller is '{caller}'",
                    id = memory.id,
                    owner = memory.owner_hat_id.as_deref().unwrap_or("?"),
                    caller = caller
                );
            }
            Ok(())
        }
    }
}

/// Execute a memory command.
pub fn execute(args: MemoryArgs, use_colors: bool) -> Result<()> {
    let root = resolve_workspace_root(args.root.as_ref());
    let store = MarkdownMemoryStore::with_default_path(&root);
    let ctx = operation_context_for(args.root.as_ref());

    match args.command {
        MemoryCommands::Add(add_args) => add_command(&store, &ctx, add_args, use_colors),
        MemoryCommands::List(list_args) => list_command(&store, &ctx, list_args, use_colors),
        MemoryCommands::Show(show_args) => show_command(&store, &ctx, show_args, use_colors),
        MemoryCommands::Delete(delete_args) => {
            delete_command(&store, &ctx, delete_args, use_colors)
        }
        MemoryCommands::Search(search_args) => {
            search_command(&store, &ctx, search_args, use_colors)
        }
        MemoryCommands::Prime(prime_args) => prime_command(&store, &ctx, prime_args),
        MemoryCommands::Init(init_args) => init_command(&store, init_args, use_colors),
    }
}

fn add_command(
    store: &MarkdownMemoryStore,
    ctx: &OperationContext,
    args: AddArgs,
    use_colors: bool,
) -> Result<()> {
    // P3 guard: reject empty content up front
    if args.content.trim().is_empty() {
        bail!("memory add: content must not be empty");
    }
    // P3 guard: reject oversized content
    if args.content.chars().count() > MAX_MEMORY_CONTENT_CHARS {
        bail!(
            "memory add: content exceeds {} characters",
            MAX_MEMORY_CONTENT_CHARS
        );
    }

    // Parse tags
    let tags: Vec<String> = args
        .tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    // Decide visibility / owner before constructing the memory
    let (owner_hat_id, visibility) = if args.private {
        // --private is only meaningful in agent context, where the
        // owning hat is the active hat. Humans invoking --private
        // would create an owner-less private entry (a fail-closed
        // state), so we reject the request up front.
        if !ctx.is_agent_context {
            bail!(
                "memory add: --private requires agent context (set RALPH_CURRENT_HAT to scope the memory)"
            );
        }
        let hat_id = ctx.current_hat_id.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "memory add: --private requires RALPH_CURRENT_HAT to identify the owning hat"
            )
        })?;
        // P3 guard: per-hat private threshold
        let n = store
            .count_private_for_owner(&hat_id)
            .context("Failed to count private memories")?;
        if n >= MAX_PRIVATE_MEMORIES_PER_HAT {
            bail!(
                "memory add: hat '{hat_id}' already owns {n} private memories (limit: {limit})",
                hat_id = hat_id,
                limit = MAX_PRIVATE_MEMORIES_PER_HAT
            );
        }
        (Some(hat_id), MemoryVisibility::Private)
    } else {
        (None, MemoryVisibility::Shared)
    };

    let memory = Memory::new_with_owner(args.r#type, args.content, tags, owner_hat_id, visibility);
    let id = memory.id.clone();

    store.append(&memory).context("Failed to store memory")?;

    // Output based on format
    match args.format {
        OutputFormat::Quiet => {
            println!("{}", id);
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&memory)?;
            println!("{}", json);
        }
        OutputFormat::Markdown => {
            let owner_meta = memory
                .owner_hat_id
                .as_deref()
                .map(|o| format!(" | owner: {}", o))
                .unwrap_or_default();
            println!(
                "### {}\n> {}\n<!-- tags: {} | created: {} | visibility: {}{} -->",
                memory.id,
                memory.content.replace('\n', "\n> "),
                memory.tags.join(", "),
                memory.created,
                memory.visibility.as_str(),
                owner_meta,
            );
        }
        OutputFormat::Table => {
            if use_colors {
                println!("{}📝 Memory stored:{} {}", colors::GREEN, colors::RESET, id);
            } else {
                println!("Memory stored: {}", id);
            }
        }
    }

    Ok(())
}

fn list_command(
    store: &MarkdownMemoryStore,
    ctx: &OperationContext,
    args: ListArgs,
    use_colors: bool,
) -> Result<()> {
    // P3: agent context filters by visibility; human context sees everything.
    let mut memories = if ctx.is_agent_context {
        store
            .load_visible(ctx.current_hat_id.as_deref())
            .context("Failed to load memories")?
    } else {
        store.load().context("Failed to load memories")?
    };

    // Filter by type if specified
    if let Some(memory_type) = args.r#type {
        memories.retain(|m| m.memory_type == memory_type);
    }

    // Apply last N filter
    if let Some(n) = args.last
        && memories.len() > n
    {
        memories = memories.into_iter().rev().take(n).rev().collect();
    }

    if memories.is_empty() {
        if use_colors {
            println!("\n{}No memories yet.{}\n", colors::DIM, colors::RESET);
            println!("Create your first memory:");
            println!(
                "  {}ralph tools memory add \"<content>\" -t pattern --tags tag1,tag2{}\n",
                colors::CYAN,
                colors::RESET
            );
            println!("Memory types: pattern, decision, fix, context");
            println!();
        } else {
            println!("\nNo memories yet.\n");
            println!("Create your first memory:");
            println!("  ralph tools memory add \"<content>\" -t pattern --tags tag1,tag2\n");
            println!("Memory types: pattern, decision, fix, context");
            println!();
        }
        return Ok(());
    }

    output_memories(&memories, args.format, use_colors);
    Ok(())
}

fn show_command(
    store: &MarkdownMemoryStore,
    ctx: &OperationContext,
    args: ShowArgs,
    use_colors: bool,
) -> Result<()> {
    // P3: in agent context use visibility-aware lookup; in human
    // context use the raw store.
    let memory = if ctx.is_agent_context {
        store
            .get_visible(&args.id, ctx.current_hat_id.as_deref())
            .context("Failed to read memories")?
    } else {
        store.get(&args.id).context("Failed to read memories")?
    };

    let Some(memory) = memory else {
        // In agent context, distinguish "does not exist" from
        // "hidden by visibility rules" so the agent doesn't learn
        // about hidden entries by guessing.
        if ctx.is_agent_context
            && store
                .get(&args.id)
                .context("Failed to read memories")?
                .is_some()
        {
            bail!(
                "Memory not found or hidden by visibility rules: {}",
                args.id
            );
        }
        bail!("Memory not found: {}", args.id);
    };

    output_memory(&memory, args.format, use_colors);
    Ok(())
}

fn delete_command(
    store: &MarkdownMemoryStore,
    ctx: &OperationContext,
    args: DeleteArgs,
    use_colors: bool,
) -> Result<()> {
    // P3: enforce visibility-aware authorization before mutating.
    let memory = store.get(&args.id).context("Failed to read memories")?;
    authorize_memory_action(memory.as_ref(), ctx, "memory delete")?;

    let deleted = store.delete(&args.id).context("Failed to delete memory")?;

    if deleted {
        if use_colors {
            println!(
                "{}🗑️  Memory deleted:{} {}",
                colors::GREEN,
                colors::RESET,
                args.id
            );
        } else {
            println!("Memory deleted: {}", args.id);
        }
        Ok(())
    } else {
        bail!("Memory not found: {}", args.id)
    }
}

fn search_command(
    store: &MarkdownMemoryStore,
    ctx: &OperationContext,
    args: SearchArgs,
    use_colors: bool,
) -> Result<()> {
    // P3: agent context filters by visibility; human context sees everything.
    let all_memories = if ctx.is_agent_context {
        store
            .load_visible(ctx.current_hat_id.as_deref())
            .context("Failed to load memories")?
    } else {
        store.load().context("Failed to load memories")?
    };
    let total_count = all_memories.len();
    let mut memories = all_memories;

    // Filter by query if provided
    if let Some(ref query) = args.query {
        memories.retain(|m| m.matches_query(query));
    }

    // Filter by type if specified
    if let Some(memory_type) = args.r#type {
        memories.retain(|m| m.memory_type == memory_type);
    }

    // Filter by tags if specified
    if let Some(ref tags_str) = args.tags {
        let tags: Vec<String> = tags_str.split(',').map(|s| s.trim().to_string()).collect();
        memories.retain(|m| m.has_any_tag(&tags));
    }

    let match_count = memories.len();
    let truncated = !args.all && match_count > 10;

    // Limit results unless --all is specified
    if truncated {
        memories.truncate(10);
    }

    if memories.is_empty() {
        if use_colors {
            println!(
                "\n{}No matching memories found in {} total memories.{}",
                colors::DIM,
                total_count,
                colors::RESET
            );
            println!(
                "{}Try a different search term or use `ralph tools memory list` to see all.{}\n",
                colors::DIM,
                colors::RESET
            );
        } else {
            println!(
                "\nNo matching memories found in {} total memories.",
                total_count
            );
            println!("Try a different search term or use `ralph tools memory list` to see all.\n");
        }
        return Ok(());
    }

    // Print search header (only for table format)
    if args.format == OutputFormat::Table {
        if use_colors {
            if let Some(ref query) = args.query {
                println!(
                    "\n{}Search results for \"{}\"{} ({} of {} memories)",
                    colors::DIM,
                    query,
                    colors::RESET,
                    match_count,
                    total_count
                );
            }
        } else if let Some(ref query) = args.query {
            println!(
                "\nSearch results for \"{}\" ({} of {} memories)",
                query, match_count, total_count
            );
        }
    }

    output_memories(&memories, args.format, use_colors);

    // Show truncation hint (only for table format)
    if truncated && args.format == OutputFormat::Table {
        if use_colors {
            println!(
                "{}Showing 10 of {} matches • Use --all to see all results{}\n",
                colors::DIM,
                match_count,
                colors::RESET
            );
        } else {
            println!(
                "Showing 10 of {} matches • Use --all to see all results\n",
                match_count
            );
        }
    }

    Ok(())
}

fn prime_command(
    store: &MarkdownMemoryStore,
    ctx: &OperationContext,
    args: PrimeArgs,
) -> Result<()> {
    // P3: agent context filters by visibility; human context sees everything.
    let mut memories = if ctx.is_agent_context {
        store
            .load_visible(ctx.current_hat_id.as_deref())
            .context("Failed to load memories")?
    } else {
        store.load().context("Failed to load memories")?
    };

    // Filter by types if specified
    if let Some(ref types_str) = args.r#type {
        let types: Vec<MemoryType> = types_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !types.is_empty() {
            memories.retain(|m| types.contains(&m.memory_type));
        }
    }

    // Filter by tags if specified
    if let Some(ref tags_str) = args.tags {
        let tags: Vec<String> = tags_str.split(',').map(|s| s.trim().to_string()).collect();
        memories.retain(|m| m.has_any_tag(&tags));
    }

    // Filter by recent days if specified
    if let Some(days) = args.recent {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();
        memories.retain(|m| m.created >= cutoff_str);
    }

    if memories.is_empty() {
        return Ok(());
    }

    // Generate output
    let output = match args.format {
        OutputFormat::Json => serde_json::to_string_pretty(&memories)?,
        OutputFormat::Markdown => format_memories_as_markdown(&memories),
        OutputFormat::Table => format_memories_as_text(&memories),
        OutputFormat::Quiet => {
            memories
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>()
                .join("\n")
                + if memories.is_empty() { "" } else { "\n" }
        }
    };

    // Apply budget if specified
    let final_output = if let Some(budget) = args.budget {
        if budget > 0 {
            truncate_to_budget(&output, budget)
        } else {
            output
        }
    } else {
        output
    };

    print!("{}", final_output);
    Ok(())
}

fn init_command(store: &MarkdownMemoryStore, args: InitArgs, use_colors: bool) -> Result<()> {
    store.init(args.force).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            anyhow::anyhow!(
                "Memories file already exists at {}. Use --force to overwrite.",
                store.path().display()
            )
        } else {
            anyhow::anyhow!("Failed to initialize memories: {}", e)
        }
    })?;

    if use_colors {
        println!(
            "{}✓{} Initialized memories file at {}",
            colors::GREEN,
            colors::RESET,
            store.path().display()
        );
    } else {
        println!("Initialized memories file at {}", store.path().display());
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Output Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn output_memories(memories: &[Memory], format: OutputFormat, use_colors: bool) {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(memories).unwrap_or_default();
            println!("{}", json);
        }
        OutputFormat::Markdown => {
            print!("{}", format_memories_as_markdown(memories));
        }
        OutputFormat::Quiet => {
            for memory in memories {
                println!("{}", memory.id);
            }
        }
        OutputFormat::Table => {
            print_memories_table(memories, use_colors);
        }
    }
}

fn output_memory(memory: &Memory, format: OutputFormat, use_colors: bool) {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(memory).unwrap_or_default();
            println!("{}", json);
        }
        OutputFormat::Markdown => {
            let owner_meta = memory
                .owner_hat_id
                .as_deref()
                .map(|o| format!(" | owner: {}", o))
                .unwrap_or_default();
            println!(
                "### {}\n> {}\n<!-- tags: {} | created: {} | visibility: {}{} -->",
                memory.id,
                memory.content.replace('\n', "\n> "),
                memory.tags.join(", "),
                memory.created,
                memory.visibility.as_str(),
                owner_meta,
            );
        }
        OutputFormat::Quiet => {
            println!("{}", memory.id);
        }
        OutputFormat::Table => {
            print_memory_detail(memory, use_colors);
        }
    }
}

fn print_memories_table(memories: &[Memory], use_colors: bool) {
    use colors::*;

    // Header - simplified columns: Type, Age, Tags, Content
    if use_colors {
        println!("\n{BOLD}  # │ Type      │ Age          │ Tags             │ Content{RESET}");
        println!(
            "{DIM}────┼───────────┼──────────────┼──────────────────┼────────────────────────────────────────{RESET}"
        );
    } else {
        println!("\n  # | Type      | Age          | Tags             | Content");
        println!(
            "----|-----------|--------------|------------------|----------------------------------------"
        );
    }

    for (i, memory) in memories.iter().enumerate() {
        let emoji = memory.memory_type.emoji();
        let type_name = memory.memory_type.to_string();
        let age = format_relative_date(&memory.created);
        let tags = if memory.tags.is_empty() {
            "-".to_string()
        } else {
            memory.tags.join(", ")
        };
        // Longer content preview (50 chars) for better readability
        let content_preview = truncate_with_ellipsis(&memory.content.replace('\n', " "), 50);

        if use_colors {
            println!(
                "{DIM}{:>3}{RESET} │ {} {:<7} │ {:<12} │ {CYAN}{:<16}{RESET} │ {}",
                i + 1,
                emoji,
                type_name,
                age,
                truncate_with_ellipsis(&tags, 16),
                content_preview
            );
        } else {
            println!(
                "{:>3} | {} {:<7} | {:<12} | {:<16} | {}",
                i + 1,
                emoji,
                type_name,
                age,
                truncate_with_ellipsis(&tags, 16),
                content_preview
            );
        }
    }

    // Footer with hint
    if use_colors {
        println!(
            "\n{DIM}Showing {} memories • Use `ralph tools memory show <id>` for details{RESET}",
            memories.len()
        );
    } else {
        println!(
            "\nShowing {} memories • Use `ralph tools memory show <id>` for details",
            memories.len()
        );
    }
}

fn print_memory_detail(memory: &Memory, use_colors: bool) {
    use colors::*;

    let relative_date = format_relative_date(&memory.created);
    let tags_display = if memory.tags.is_empty() {
        "-".to_string()
    } else {
        memory.tags.join(", ")
    };
    let owner_display = memory.owner_hat_id.as_deref().unwrap_or("-");
    let visibility_display = memory.visibility.as_str();

    if use_colors {
        println!();
        println!("{DIM}╭────────────────────────────────────────────────────────────────╮{RESET}");
        println!(
            "{DIM}│{RESET} {} {BOLD}{}{RESET}",
            memory.memory_type.emoji(),
            memory.memory_type.to_string().to_uppercase()
        );
        println!("{DIM}╰────────────────────────────────────────────────────────────────╯{RESET}");
        println!();
        println!("  {BOLD}ID:{RESET}         {DIM}{}{RESET}", memory.id);
        println!(
            "  {BOLD}Created:{RESET}    {} {DIM}({}){RESET}",
            relative_date, memory.created
        );
        println!("  {BOLD}Tags:{RESET}       {CYAN}{}{RESET}", tags_display);
        println!(
            "  {BOLD}Visibility:{RESET} {MAGENTA}{}{RESET}",
            visibility_display
        );
        if memory.owner_hat_id.is_some() {
            println!("  {BOLD}Owner:{RESET}      {CYAN}{}{RESET}", owner_display);
        }
        println!();
        println!("  {BOLD}Content:{RESET}");
        println!("{DIM}  ─────────────────────────────────────────────────────────────{RESET}");
        for line in memory.content.lines() {
            println!("  {}", line);
        }
        println!();
    } else {
        println!();
        println!("┌────────────────────────────────────────────────────────────────┐");
        println!(
            "│ {} {}",
            memory.memory_type.emoji(),
            memory.memory_type.to_string().to_uppercase()
        );
        println!("└────────────────────────────────────────────────────────────────┘");
        println!();
        println!("  ID:         {}", memory.id);
        println!("  Created:    {} ({})", relative_date, memory.created);
        println!("  Tags:       {}", tags_display);
        println!("  Visibility: {}", visibility_display);
        if memory.owner_hat_id.is_some() {
            println!("  Owner:      {}", owner_display);
        }
        println!();
        println!("  Content:");
        println!("  ─────────────────────────────────────────────────────────────");
        for line in memory.content.lines() {
            println!("  {}", line);
        }
        println!();
    }
}

fn format_memories_as_markdown(memories: &[Memory]) -> String {
    let mut output = String::from("# Memories\n");

    // Group by type
    for memory_type in MemoryType::all() {
        let type_memories: Vec<_> = memories
            .iter()
            .filter(|m| m.memory_type == *memory_type)
            .collect();

        if type_memories.is_empty() {
            continue;
        }

        output.push_str(&format!("\n## {}\n", memory_type.section_name()));

        for memory in type_memories {
            let owner_meta = memory
                .owner_hat_id
                .as_deref()
                .map(|o| format!(" | owner: {}", o))
                .unwrap_or_default();
            output.push_str(&format!(
                "\n### {}\n> {}\n<!-- tags: {} | created: {} | visibility: {}{} -->\n",
                memory.id,
                memory.content.replace('\n', "\n> "),
                memory.tags.join(", "),
                memory.created,
                memory.visibility.as_str(),
                owner_meta,
            ));
        }
    }

    output
}

fn format_memories_as_text(memories: &[Memory]) -> String {
    let mut output = String::new();

    for memory in memories {
        output.push_str(&format!(
            "# {} [{}]\n{}\n",
            memory.id,
            memory.memory_type.section_name(),
            memory.content
        ));
        if !memory.tags.is_empty() {
            output.push_str(&format!("Tags: {}\n", memory.tags.join(", ")));
        }
        output.push_str(&format!("Created: {}\n\n", memory.created));
    }

    output
}

/// Truncate content to approximately fit within a token budget.
///
/// Uses a simple heuristic of ~4 characters per token.
fn truncate_to_budget(content: &str, budget: usize) -> String {
    // Rough estimate: 4 chars per token
    let char_budget = budget * 4;

    if content.len() <= char_budget {
        return content.to_string();
    }

    // Find a good break point (end of a memory block)
    let truncated = &content[..char_budget];

    // Try to find the last complete memory block (ends with -->)
    if let Some(last_complete) = truncated.rfind("-->") {
        let end = last_complete + 3;
        // Find the next newline after -->
        let final_end = truncated[end..].find('\n').map_or(end, |n| end + n + 1);
        format!(
            "{}\n\n<!-- truncated: budget {} tokens exceeded -->",
            &content[..final_end],
            budget
        )
    } else {
        format!(
            "{}\n\n<!-- truncated: budget {} tokens exceeded -->",
            truncated, budget
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate};

    fn fixed_today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 31).expect("valid date")
    }

    fn date_days_ago(days: i64) -> String {
        (fixed_today() - Duration::days(days))
            .format("%Y-%m-%d")
            .to_string()
    }

    #[test]
    fn format_relative_date_with_today_handles_ranges() {
        let today = fixed_today();
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(0), today),
            "today"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(1), today),
            "yesterday"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(2), today),
            "2 days ago"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(7), today),
            "1 week ago"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(14), today),
            "2 weeks ago"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(21), today),
            "3 weeks ago"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(28), today),
            "1 month ago"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(45), today),
            "2 months ago"
        );
        assert_eq!(
            format_relative_date_with_today(&date_days_ago(90), today),
            "3 months ago"
        );
    }

    #[test]
    fn format_relative_date_with_today_returns_input_on_invalid() {
        let today = fixed_today();
        let value = "not-a-date";
        assert_eq!(format_relative_date_with_today(value, today), "not-a-date");
    }

    #[test]
    fn format_relative_date_with_today_returns_date_for_old_entries() {
        let today = fixed_today();
        let date_str = date_days_ago(400);
        assert_eq!(format_relative_date_with_today(&date_str, today), date_str);
    }

    #[test]
    fn truncate_to_budget_prefers_complete_memory_blocks() {
        let content = "### mem-1\n> hi\n<!-- tags: a | created: 2026-01-31 -->\n\n\
### mem-2\n> more\n<!-- tags: b | created: 2026-01-31 -->\n"
            .to_string();
        let first_end = content.find("-->").expect("marker") + 3;
        let budget = (first_end + 6).div_ceil(4);
        let truncated = truncate_to_budget(&content, budget);

        assert!(truncated.contains("mem-1"));
        assert!(!truncated.contains("mem-2"));
        assert!(truncated.contains("<!-- truncated: budget"));
    }

    #[test]
    fn truncate_to_budget_falls_back_without_marker() {
        let content = "abcdefghijklmnopqrstuvwxyz";
        let truncated = truncate_to_budget(content, 1);
        assert!(truncated.starts_with("abcd"));
        assert!(truncated.contains("truncated: budget 1 tokens exceeded"));
    }

    #[test]
    fn format_memories_as_markdown_groups_by_type() {
        let memories = vec![
            Memory {
                id: "mem-1".to_string(),
                memory_type: MemoryType::Pattern,
                content: "alpha".to_string(),
                tags: vec!["tag1".to_string()],
                created: "2026-01-31".to_string(),
                ..Default::default()
            },
            Memory {
                id: "mem-2".to_string(),
                memory_type: MemoryType::Fix,
                content: "beta".to_string(),
                tags: vec![],
                created: "2026-01-31".to_string(),
                ..Default::default()
            },
        ];

        let output = format_memories_as_markdown(&memories);
        assert!(output.contains("# Memories"));
        assert!(output.contains("## Patterns"));
        assert!(output.contains("## Fixes"));
        assert!(!output.contains("## Decisions"));
        assert!(output.contains("mem-1"));
        assert!(output.contains("mem-2"));
    }

    #[test]
    fn format_memories_as_text_has_plain_fields() {
        let memories = vec![Memory {
            id: "mem-1".to_string(),
            memory_type: MemoryType::Decision,
            content: "beta".to_string(),
            tags: vec!["tag1".to_string()],
            created: "2026-01-31".to_string(),
            ..Default::default()
        }];

        let output = format_memories_as_text(&memories);
        assert!(output.contains("# mem-1"));
        assert!(output.contains("beta"));
        assert!(output.contains("Tags: tag1"));
        assert!(output.contains("Created: 2026-01-31"));
    }

    // ---- P3 CLI visibility / owner / authorization tests ----

    /// Build an `OperationContext` for tests with an injected env resolver.
    fn ctx_for(workspace: &std::path::Path, hat: Option<&str>) -> OperationContext {
        OperationContext::detect_with_env(workspace.to_path_buf(), move |key| {
            if key == "RALPH_CURRENT_HAT" {
                hat.map(String::from)
            } else {
                None
            }
        })
    }

    /// Build a `MarkdownMemoryStore` rooted at a fresh temp dir.
    fn temp_store() -> (tempfile::TempDir, MarkdownMemoryStore) {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let store = MarkdownMemoryStore::with_default_path(tmp.path());
        (tmp, store)
    }

    /// Append a memory directly to the store (bypassing CLI guards) so
    /// tests can set up fixtures for visibility / owner scenarios.
    fn seed(
        store: &MarkdownMemoryStore,
        content: &str,
        owner: Option<&str>,
        visibility: MemoryVisibility,
    ) -> String {
        let m = Memory::new_with_owner(
            MemoryType::Pattern,
            content.to_string(),
            vec![],
            owner.map(String::from),
            visibility,
        );
        let id = m.id.clone();
        store.append(&m).expect("seed");
        id
    }

    #[test]
    fn test_add_command_stamps_owner_hat_when_private() {
        let (tmp, store) = temp_store();
        let args = AddArgs {
            content: "private note".to_string(),
            r#type: MemoryType::Pattern,
            tags: None,
            private: true,
            format: OutputFormat::Quiet,
        };
        let ctx = ctx_for(tmp.path(), Some("executor"));

        add_command(&store, &ctx, args, false).expect("add should succeed");

        let raw = std::fs::read_to_string(store.path()).expect("read file");
        assert!(
            raw.contains("owner: executor"),
            "metadata missing owner: {raw}"
        );
        assert!(
            raw.contains("visibility: private"),
            "metadata missing visibility: {raw}"
        );
    }

    #[test]
    fn test_add_command_default_visibility_shared() {
        let (tmp, store) = temp_store();
        let args = AddArgs {
            content: "shared note".to_string(),
            r#type: MemoryType::Pattern,
            tags: None,
            private: false,
            format: OutputFormat::Quiet,
        };
        let ctx = ctx_for(tmp.path(), Some("executor"));

        add_command(&store, &ctx, args, false).expect("add should succeed");

        let raw = std::fs::read_to_string(store.path()).expect("read file");
        assert!(
            raw.contains("visibility: shared"),
            "shared visibility missing: {raw}"
        );
        assert!(
            !raw.contains("owner: "),
            "shared memory should not carry owner: {raw}"
        );
    }

    #[test]
    fn test_add_command_rejects_empty_content() {
        let (tmp, store) = temp_store();
        let args = AddArgs {
            content: "   \n  ".to_string(),
            r#type: MemoryType::Pattern,
            tags: None,
            private: false,
            format: OutputFormat::Quiet,
        };
        let ctx = ctx_for(tmp.path(), Some("executor"));

        let err = add_command(&store, &ctx, args, false).expect_err("empty must fail");
        assert!(err.to_string().contains("must not be empty"));
        assert!(!store.exists());
    }

    #[test]
    fn test_add_command_rejects_oversized() {
        let (tmp, store) = temp_store();
        let oversized = "a".repeat(MAX_MEMORY_CONTENT_CHARS + 1);
        let args = AddArgs {
            content: oversized,
            r#type: MemoryType::Pattern,
            tags: None,
            private: false,
            format: OutputFormat::Quiet,
        };
        let ctx = ctx_for(tmp.path(), Some("executor"));

        let err = add_command(&store, &ctx, args, false).expect_err("oversized must fail");
        assert!(err.to_string().contains("exceeds"));
        assert!(!store.exists());
    }

    #[test]
    fn test_add_command_private_threshold_per_hat() {
        let (tmp, store) = temp_store();
        let ctx = ctx_for(tmp.path(), Some("executor"));

        // Seed `MAX_PRIVATE_MEMORIES_PER_HAT` private memories directly.
        for i in 0..MAX_PRIVATE_MEMORIES_PER_HAT {
            seed(
                &store,
                &format!("seed {i}"),
                Some("executor"),
                MemoryVisibility::Private,
            );
        }

        let args = AddArgs {
            content: "one too many".to_string(),
            r#type: MemoryType::Pattern,
            tags: None,
            private: true,
            format: OutputFormat::Quiet,
        };
        let err = add_command(&store, &ctx, args, false).expect_err("private threshold must fail");
        assert!(err.to_string().contains("limit"));
    }

    #[test]
    fn test_add_command_rejects_private_in_human_context() {
        let (tmp, store) = temp_store();
        let args = AddArgs {
            content: "human private".to_string(),
            r#type: MemoryType::Pattern,
            tags: None,
            private: true,
            format: OutputFormat::Quiet,
        };
        let ctx = ctx_for(tmp.path(), None); // human context

        let err = add_command(&store, &ctx, args, false).expect_err("human --private must fail");
        assert!(err.to_string().contains("agent context"));
    }

    #[test]
    fn test_list_command_agent_sees_shared_and_own_private() {
        let (tmp, store) = temp_store();
        // seed: one shared, one executor private, one reviewer private
        seed(&store, "shared", None, MemoryVisibility::Shared);
        seed(&store, "mine", Some("executor"), MemoryVisibility::Private);
        seed(
            &store,
            "theirs",
            Some("reviewer"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), Some("executor"));
        let memories = store
            .load_visible(ctx.current_hat_id.as_deref())
            .expect("load");
        let contents: Vec<_> = memories.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.contains(&"shared"));
        assert!(contents.contains(&"mine"));
        assert!(!contents.contains(&"theirs"));
    }

    #[test]
    fn test_show_command_rejects_other_hat_private() {
        let (tmp, store) = temp_store();
        let id = seed(
            &store,
            "executor-only",
            Some("executor"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), Some("reviewer"));
        let memory = store
            .get_visible(&id, ctx.current_hat_id.as_deref())
            .expect("visible lookup");
        assert!(memory.is_none(), "reviewer should not see executor private");
    }

    #[test]
    fn test_show_command_distinguishes_not_found_from_hidden() {
        let (tmp, store) = temp_store();
        let id = seed(
            &store,
            "executor-only",
            Some("executor"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), Some("reviewer"));
        let visible = store
            .get_visible(&id, ctx.current_hat_id.as_deref())
            .expect("visible lookup");
        let raw = store.get(&id).expect("raw lookup");
        assert!(visible.is_none());
        assert!(raw.is_some(), "raw store should still see the hidden entry");

        // show_command via CLI path
        let args = ShowArgs {
            id: id.clone(),
            format: OutputFormat::Quiet,
        };
        let err =
            show_command(&store, &ctx, args, false).expect_err("show should reject hidden entry");
        assert!(err.to_string().contains("not found or hidden"));
    }

    #[test]
    fn test_prime_command_hides_other_hat_private() {
        let (tmp, store) = temp_store();
        seed(&store, "shared note", None, MemoryVisibility::Shared);
        seed(
            &store,
            "exec note",
            Some("executor"),
            MemoryVisibility::Private,
        );
        seed(
            &store,
            "rev note",
            Some("reviewer"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), Some("executor"));
        let visible = store
            .load_visible(ctx.current_hat_id.as_deref())
            .expect("load");
        let contents: Vec<_> = visible.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.contains(&"shared note"));
        assert!(contents.contains(&"exec note"));
        assert!(!contents.contains(&"rev note"));
    }

    #[test]
    fn test_delete_command_rejects_other_hat_private() {
        let (tmp, store) = temp_store();
        let id = seed(
            &store,
            "executor-only",
            Some("executor"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), Some("reviewer"));
        let memory = store.get(&id).expect("get");
        let err = authorize_memory_action(memory.as_ref(), &ctx, "memory delete")
            .expect_err("reviewer cannot delete executor private");
        assert!(err.to_string().contains("private memory"));
    }

    #[test]
    fn test_delete_command_rejects_shared_from_agent_context() {
        let (tmp, store) = temp_store();
        let id = seed(&store, "shared", None, MemoryVisibility::Shared);

        let ctx = ctx_for(tmp.path(), Some("executor"));
        let memory = store.get(&id).expect("get");
        let err = authorize_memory_action(memory.as_ref(), &ctx, "memory delete")
            .expect_err("agent cannot delete shared");
        assert!(
            err.to_string()
                .contains("agent context cannot mutate shared")
        );
    }

    #[test]
    fn test_delete_command_allows_human_cli_shared() {
        let (tmp, store) = temp_store();
        let id = seed(&store, "shared", None, MemoryVisibility::Shared);

        let ctx = ctx_for(tmp.path(), None); // human
        let memory = store.get(&id).expect("get");
        authorize_memory_action(memory.as_ref(), &ctx, "memory delete")
            .expect("human may delete shared");
    }

    #[test]
    fn test_delete_command_allows_human_cli_private() {
        let (tmp, store) = temp_store();
        let id = seed(
            &store,
            "executor-only",
            Some("executor"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), None); // human
        let memory = store.get(&id).expect("get");
        authorize_memory_action(memory.as_ref(), &ctx, "memory delete")
            .expect("human may delete any private");
    }

    #[test]
    fn test_list_command_human_sees_all() {
        let (tmp, store) = temp_store();
        seed(&store, "shared", None, MemoryVisibility::Shared);
        seed(&store, "exec", Some("executor"), MemoryVisibility::Private);
        seed(&store, "rev", Some("reviewer"), MemoryVisibility::Private);

        // Human context (no RALPH_CURRENT_HAT) — raw load, no visibility filter.
        let ctx = ctx_for(tmp.path(), None);
        assert!(!ctx.is_agent_context);
        let all = store.load().expect("load");
        let contents: Vec<_> = all.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents.len(), 3);
        assert!(contents.contains(&"shared"));
        assert!(contents.contains(&"exec"));
        assert!(contents.contains(&"rev"));
    }

    #[test]
    fn test_search_command_agent_filters_by_visibility() {
        let (tmp, store) = temp_store();
        seed(&store, "alpha shared", None, MemoryVisibility::Shared);
        seed(
            &store,
            "alpha exec",
            Some("executor"),
            MemoryVisibility::Private,
        );
        seed(
            &store,
            "alpha rev",
            Some("reviewer"),
            MemoryVisibility::Private,
        );

        let ctx = ctx_for(tmp.path(), Some("executor"));
        let visible = store
            .load_visible(ctx.current_hat_id.as_deref())
            .expect("load");
        let alpha: Vec<_> = visible
            .iter()
            .filter(|m| m.content.contains("alpha"))
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(alpha.len(), 2);
        assert!(alpha.contains(&"alpha shared"));
        assert!(alpha.contains(&"alpha exec"));
        assert!(!alpha.contains(&"alpha rev"));
    }

    #[test]
    fn test_markdown_output_includes_owner_when_present() {
        let memory = Memory::new_with_owner(
            MemoryType::Pattern,
            "x".to_string(),
            vec![],
            Some("executor".to_string()),
            MemoryVisibility::Private,
        );
        // format_memories_as_markdown should embed both owner and visibility
        let md = format_memories_as_markdown(std::slice::from_ref(&memory));
        assert!(md.contains("owner: executor"), "owner missing in md: {md}");
        assert!(
            md.contains("visibility: private"),
            "visibility missing in md: {md}"
        );
    }
}
