//! `search_flights` tool handler — deterministic procedural generator.
//!
//! A template shouldn't require paid API keys to demo a flight-booking
//! flow, so this stand-in produces plausible-looking flights entirely
//! from the input parameters. The generator is seeded by
//! `(origin, destination, date)` via SHA-256, so:
//!
//! - The same query always returns the same flights (cache-friendly,
//!   easy to reason about in tests).
//! - Different queries return different flights (the UX illusion the
//!   demo needs).
//!
//! Forks replace this with a real aggregator call (Amadeus, Duffel,
//! Skyscanner, …). The JSON shape it returns matches what the
//! `show_flight_options` client tool + `FlightResults` React component
//! expect.

use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const AIRLINES: &[(&str, &str)] = &[
    ("United", "UA"),
    ("Delta", "DL"),
    ("American", "AA"),
    ("Air Canada", "AC"),
    ("Lufthansa", "LH"),
    ("British Airways", "BA"),
    ("Air France", "AF"),
    ("KLM", "KL"),
    ("ANA", "NH"),
    ("JAL", "JL"),
    ("Singapore Airlines", "SQ"),
    ("Emirates", "EK"),
    ("Qatar Airways", "QR"),
    ("Cathay Pacific", "CX"),
];

/// Small hand-picked table of common cities → IATA codes. Covers the
/// obvious demo prompts; falls back to first-three-letters otherwise.
const CITY_IATA: &[(&str, &str)] = &[
    ("SAN FRANCISCO", "SFO"),
    ("LOS ANGELES", "LAX"),
    ("NEW YORK", "JFK"),
    ("CHICAGO", "ORD"),
    ("SEATTLE", "SEA"),
    ("BOSTON", "BOS"),
    ("MIAMI", "MIA"),
    ("TORONTO", "YYZ"),
    ("VANCOUVER", "YVR"),
    ("MEXICO CITY", "MEX"),
    ("LONDON", "LHR"),
    ("PARIS", "CDG"),
    ("FRANKFURT", "FRA"),
    ("MUNICH", "MUC"),
    ("AMSTERDAM", "AMS"),
    ("MADRID", "MAD"),
    ("BARCELONA", "BCN"),
    ("ROME", "FCO"),
    ("BERLIN", "BER"),
    ("ISTANBUL", "IST"),
    ("DUBAI", "DXB"),
    ("DELHI", "DEL"),
    ("MUMBAI", "BOM"),
    ("BANGKOK", "BKK"),
    ("SINGAPORE", "SIN"),
    ("HONG KONG", "HKG"),
    ("TOKYO", "NRT"),
    ("OSAKA", "KIX"),
    ("SEOUL", "ICN"),
    ("SYDNEY", "SYD"),
    ("MELBOURNE", "MEL"),
    ("AUCKLAND", "AKL"),
];

/// Tool handler for `search_flights`. Input:
/// `{ origin: string, destination: string, date: string }`.
pub async fn search(input: Value) -> Result<Value> {
    let origin_input = input
        .get("origin")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let destination_input = input
        .get("destination")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let date = input
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let origin_iata = to_iata(&origin_input);
    let destination_iata = to_iata(&destination_input);

    let seed_key = format!("{}|{}|{}", origin_iata, destination_iata, date);
    let mut rng = Rng::seeded(&seed_key);

    // Base parameters vary with the route — same route always picks the
    // same base. Shape: 3h–14h total flight envelope, $350–$1650 economy.
    let base_duration_min: i64 = 180 + rng.range(0, 660);
    let base_price: i64 = 350 + rng.range(0, 1300);

    let mut flights: Vec<Value> = (0..3)
        .map(|i| build_flight(&mut rng, i, base_duration_min, base_price, &origin_iata, &destination_iata))
        .collect();

    // Cheapest first — more useful default ordering than insertion order.
    flights.sort_by_key(|f| f["price_usd"].as_i64().unwrap_or(0));

    Ok(json!({
        "origin": origin_iata,
        "destination": destination_iata,
        "origin_label": origin_input,
        "destination_label": destination_input,
        "date": date,
        "flights": flights,
    }))
}

fn build_flight(
    rng: &mut Rng,
    seq: usize,
    base_duration_min: i64,
    base_price: i64,
    origin: &str,
    destination: &str,
) -> Value {
    let (airline, code) = *rng.pick(AIRLINES);
    let number = 100 + rng.range(0, 899);
    let flight_number = format!("{code}{number}");

    // Spread departures across the day so three results don't all leave
    // at 3am. Bias toward business-friendly hours.
    let depart_total_min = rng.range(5 * 60, 22 * 60 + 30);
    let depart_time = hm(depart_total_min);

    let duration = (base_duration_min + rng.range(-30, 90)).max(45);
    let stops = if rng.range(0, 3) == 0 { 1 } else { 0 };
    let duration_with_stops = duration + if stops > 0 { rng.range(45, 120) } else { 0 };

    let arrive_time = hm((depart_total_min + duration_with_stops) % (24 * 60));

    let cabin_roll = rng.range(0, 9);
    let cabin = if cabin_roll < 7 {
        "economy"
    } else if cabin_roll < 9 {
        "premium"
    } else {
        "business"
    };

    let cabin_mult = match cabin {
        "business" => 3.2,
        "premium" => 1.7,
        _ => 1.0,
    };
    let stop_discount = if stops > 0 { 0.85 } else { 1.0 };
    let variation = 0.85 + (rng.range(0, 30) as f64) / 100.0;
    let price = ((base_price as f64) * cabin_mult * stop_discount * variation).round() as i64;

    json!({
        "id": format!("{flight_number}-{seq}"),
        "airline": airline,
        "flight_number": flight_number,
        "origin": origin,
        "destination": destination,
        "depart_time": depart_time,
        "arrive_time": arrive_time,
        "duration_minutes": duration_with_stops,
        "stops": stops,
        "price_usd": price,
        "cabin": cabin,
    })
}

fn hm(total_minutes: i64) -> String {
    let m = total_minutes.rem_euclid(24 * 60);
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn to_iata(input: &str) -> String {
    let trimmed = input.trim();
    let upper = trimmed.to_uppercase();
    // Already looks like an IATA code.
    if trimmed.len() == 3 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        return upper;
    }
    // Known city match (most specific first isn't needed — no overlaps).
    for (city, iata) in CITY_IATA {
        if upper.contains(city) {
            return iata.to_string();
        }
    }
    // Fallback: first three ASCII letters uppercased, or "???" if the
    // input has none (e.g. "123").
    let fallback: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .take(3)
        .collect::<String>()
        .to_uppercase();
    if fallback.is_empty() {
        "???".to_string()
    } else {
        fallback
    }
}

/// Seeded splitmix64 — tiny deterministic PRNG, no dependencies beyond
/// the sha2 we already pull in for the session-cookie HMAC.
struct Rng(u64);

impl Rng {
    fn seeded(key: &str) -> Self {
        let digest = Sha256::digest(key.as_bytes());
        let mut seed_bytes = [0u8; 8];
        seed_bytes.copy_from_slice(&digest[..8]);
        // Ensure seed is nonzero so splitmix64 doesn't degenerate.
        let seed = u64::from_le_bytes(seed_bytes) | 1;
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Inclusive on both ends; returns a value in `[low, high]`.
    fn range(&mut self, low: i64, high: i64) -> i64 {
        debug_assert!(high >= low);
        let span = (high - low + 1) as u64;
        low + (self.next_u64() % span) as i64
    }

    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        let idx = (self.next_u64() as usize) % slice.len();
        &slice[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn same_query_is_deterministic() {
        let input = json!({
            "origin": "SFO",
            "destination": "NRT",
            "date": "2026-05-10"
        });
        let a = search(input.clone()).await.unwrap();
        let b = search(input).await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn different_queries_diverge() {
        let a = search(json!({"origin":"SFO","destination":"NRT","date":"2026-05-10"}))
            .await
            .unwrap();
        let b = search(json!({"origin":"JFK","destination":"LHR","date":"2026-05-10"}))
            .await
            .unwrap();
        assert_ne!(a["flights"], b["flights"]);
    }

    #[test]
    fn iata_lookup_handles_common_cities() {
        assert_eq!(to_iata("Tokyo"), "NRT");
        assert_eq!(to_iata("san francisco"), "SFO");
        assert_eq!(to_iata("SFO"), "SFO");
        assert_eq!(to_iata("XYZ"), "XYZ");
    }
}
