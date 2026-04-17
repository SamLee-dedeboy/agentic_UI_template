use clap::Parser;

mod core;
mod web_server;

#[derive(Parser)]
#[command(name = "claude-ui-app")]
#[command(
    about = "Agentic data-visualization app: upload a dataset, ask questions, \
             and get interleaved prose + charts from Claude. The data tools \
             (describe_dataset, query_dataset, create_chart) live in a \
             Python MCP sidecar under `data_server/`."
)]
struct Args {
    /// Port to listen on.
    #[arg(short, long, default_value = "8080")]
    port: u16,

    /// Host to bind to. Use 0.0.0.0 to expose on the LAN (remember to set
    /// a strong APP_SESSION_KEY in that case).
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    host: String,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = Args::parse();

    let host: std::net::IpAddr = match args.host.parse() {
        Ok(ip) => ip,
        Err(e) => {
            eprintln!("❌ Invalid --host '{}': {}", args.host, e);
            std::process::exit(2);
        }
    };

    println!("🚀 Starting {}", env!("CARGO_PKG_NAME"));

    // Tool registry for Rust-side tools. Empty by default: the viz app's
    // data tools (describe_dataset, query_dataset, create_chart) live in
    // the Python MCP server under `data_server/`. Forks that want to add
    // Rust-side tools alongside Python ones can register them here via
    // `b.server_tool(...)` / `b.client_tool(...)`.
    let tools = build_tool_registry();

    if let Err(e) = web_server::start_web_mode(host, args.port, tools).await {
        eprintln!("❌ Failed to start web server: {}", e);
        std::process::exit(1);
    }
}

fn build_tool_registry() -> core::tools::ToolRegistry {
    core::tools::ToolRegistry::builder().build()
}
