//! Reference implementations for the template's example server tools.
//!
//! `main.rs` registers `get_weather` and `search_flights` against these
//! handlers. Forks typically replace the entire `examples` module with
//! their own domain logic (flight aggregator API, inventory service,
//! CRM, whatever) and drop the reference handlers.
//!
//! - [`weather`]: live Open-Meteo lookup, no API key required.
//! - [`flights`]: deterministic procedural generator so the stubbed
//!   data varies with the query parameters without needing a paid
//!   flight-search API.

pub mod flights;
pub mod weather;
