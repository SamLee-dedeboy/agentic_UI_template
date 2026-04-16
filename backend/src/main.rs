use clap::Parser;
use serde_json::json;

mod claude_binary;
mod commands;
mod core;
mod examples;
mod process;
mod web_server;

#[derive(Parser)]
#[command(name = "claude-ui-app")]
#[command(
    about = "Web server exposing Claude Code as a customer-facing browser UI. \
             Ships with guest-cookie sessions, per-cookie rate limits, and a \
             tool-bridge so Claude can call fork-registered domain tools."
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

    // Build the tool registry. This is the seam every fork edits to wire
    // its own domain tools. The defaults below (`get_weather` server
    // tool + `show_choice` client tool) are references — real forks
    // replace them with `search_flights`, `reserve_seat`, etc.
    let tools = build_tool_registry();

    if let Err(e) = web_server::start_web_mode(host, args.port, tools).await {
        eprintln!("❌ Failed to start web server: {}", e);
        std::process::exit(1);
    }
}

/// Reference tool registry. Replace in your fork.
fn build_tool_registry() -> core::tools::ToolRegistry {
    let mut b = core::tools::ToolRegistry::builder();

    // Server tool — runs in this Rust process. Calls the free
    // Open-Meteo API to return real current weather for any location.
    // See `examples::weather` for the implementation; swap in a richer
    // provider (AccuWeather, Pirate Weather, …) as a one-file change.
    b.server_tool(
        "get_weather",
        "Return the current weather for a city. Use this whenever the \
         user asks about the weather anywhere in the world. Supports \
         city names in any language (e.g. 'Tokyo', 'São Paulo', 'Oslo').",
        json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "City name, e.g. 'Tokyo' or 'Oslo'."
                }
            },
            "required": ["location"],
            "additionalProperties": false
        }),
        |input| async move { examples::weather::fetch(input).await },
    );

    // Client tool — rendered by a React component (wired in Phase 4).
    // Claude calls this to ask the user a multiple-choice question; the
    // user's click becomes the tool's return value.
    b.client_tool(
        "show_choice",
        "Present the user with a short list of choices and wait for them \
         to pick one. Use this whenever you want the user to commit to a \
         concrete option (confirm a booking, pick a color, etc).",
        json!({
            "type": "object",
            "properties": {
                "prompt":  { "type": "string" },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 2
                }
            },
            "required": ["prompt", "options"],
            "additionalProperties": false
        }),
    );

    // Server tool — procedurally-generated flight search (see
    // `examples::flights`). Deterministic seed by (origin, destination,
    // date) so the same query returns the same flights. Replace the
    // handler with a real aggregator (Amadeus, Duffel, Skyscanner) when
    // moving to production.
    b.server_tool(
        "search_flights",
        "Search available flights between two cities on a given date. \
         Always call this before offering flights to the user. The \
         result is a list of flights the user will then pick from via \
         `show_flight_options`.",
        json!({
            "type": "object",
            "properties": {
                "origin": {
                    "type": "string",
                    "description": "Origin city name or IATA code, e.g. 'SFO' or 'San Francisco'."
                },
                "destination": {
                    "type": "string",
                    "description": "Destination city name or IATA code."
                },
                "date": {
                    "type": "string",
                    "description": "Departure date as YYYY-MM-DD."
                }
            },
            "required": ["origin", "destination", "date"],
            "additionalProperties": false
        }),
        |input| async move { examples::flights::search(input).await },
    );

    // Client tool — renders a list of flight options and returns the
    // id the user picks. The handler is
    // `src/core/tools/builtins/FlightResults.tsx` on the frontend.
    b.client_tool(
        "show_flight_options",
        "Render a list of flights for the user to pick from. Call this \
         only after `search_flights` so you have real flight data to pass. \
         The returned `picked_id` matches one of the input flights' `id`.",
        json!({
            "type": "object",
            "properties": {
                "origin":      { "type": "string" },
                "destination": { "type": "string" },
                "date":        { "type": "string" },
                "flights": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id":               { "type": "string" },
                            "airline":          { "type": "string" },
                            "flight_number":    { "type": "string" },
                            "origin":           { "type": "string" },
                            "destination":      { "type": "string" },
                            "depart_time":      { "type": "string" },
                            "arrive_time":      { "type": "string" },
                            "duration_minutes": { "type": "number" },
                            "stops":            { "type": "number" },
                            "price_usd":        { "type": "number" },
                            "cabin":            { "type": "string" }
                        },
                        "required": [
                            "id", "airline", "flight_number",
                            "depart_time", "arrive_time",
                            "duration_minutes", "stops", "price_usd"
                        ]
                    }
                }
            },
            "required": ["origin", "destination", "date", "flights"],
            "additionalProperties": false
        }),
    );

    b.build()
}
