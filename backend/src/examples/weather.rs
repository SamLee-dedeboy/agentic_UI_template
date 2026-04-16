//! `get_weather` tool handler — live data from the free Open-Meteo API.
//!
//! Two hops:
//!   1. Geocoding API: city/place name → latitude/longitude.
//!   2. Forecast API: lat/lon → current temperature + WMO weather code.
//!
//! Open-Meteo requires no API key and is safe to call from a template
//! demo. Forks that need accurate locales or richer fields (humidity,
//! uv-index, forecasts) can swap in another provider without touching
//! the rest of the tool-registration flow — same function signature,
//! same JSON shape back to the React `WeatherResultCard`.
//!
//! Docs: <https://open-meteo.com/en/docs>

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";

#[derive(Deserialize)]
struct GeocodeResponse {
    #[serde(default)]
    results: Option<Vec<GeocodeResult>>,
}

#[derive(Deserialize)]
struct GeocodeResult {
    name: String,
    #[serde(default)]
    country: Option<String>,
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct ForecastResponse {
    current: Current,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: f64,
    weather_code: u32,
    #[serde(default)]
    wind_speed_10m: Option<f64>,
}

/// Tool handler for `get_weather`. Input: `{ location: string }`.
/// Returns the JSON the `WeatherResultCard` component expects.
pub async fn fetch(input: Value) -> Result<Value> {
    let location = input
        .get("location")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if location.is_empty() {
        return Err(anyhow!("missing 'location' argument"));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("claude-ui-template-example/0.1")
        .build()?;

    let geocode: GeocodeResponse = client
        .get(GEOCODE_URL)
        .query(&[
            ("name", location.as_str()),
            ("count", "1"),
            ("format", "json"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let place = geocode
        .results
        .and_then(|r| r.into_iter().next())
        .ok_or_else(|| anyhow!("no place found for '{}'", location))?;

    let lat = format!("{:.4}", place.latitude);
    let lon = format!("{:.4}", place.longitude);
    let forecast: ForecastResponse = client
        .get(FORECAST_URL)
        .query(&[
            ("latitude", lat.as_str()),
            ("longitude", lon.as_str()),
            ("current", "temperature_2m,weather_code,wind_speed_10m"),
            ("timezone", "auto"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let label = match place.country.as_deref() {
        Some(c) if !c.is_empty() => format!("{}, {}", place.name, c),
        _ => place.name,
    };

    Ok(json!({
        "location": label,
        "temperature_c": round_to(forecast.current.temperature_2m, 1),
        "summary": weather_code_to_summary(forecast.current.weather_code),
        "wind_kmh": forecast.current.wind_speed_10m.map(|w| round_to(w, 1)),
    }))
}

fn round_to(v: f64, places: u32) -> f64 {
    let factor = 10f64.powi(places as i32);
    (v * factor).round() / factor
}

/// WMO weather interpretation codes. See Open-Meteo docs — the full
/// table is small enough to inline, and keeping it local means the
/// summaries read however the fork wants them to.
fn weather_code_to_summary(code: u32) -> &'static str {
    match code {
        0 => "clear sky",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 => "light drizzle",
        53 => "drizzle",
        55 => "heavy drizzle",
        56 | 57 => "freezing drizzle",
        61 => "light rain",
        63 => "rain",
        65 => "heavy rain",
        66 | 67 => "freezing rain",
        71 => "light snow",
        73 => "snow",
        75 => "heavy snow",
        77 => "snow grains",
        80 => "light rain showers",
        81 => "rain showers",
        82 => "violent rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unknown conditions",
    }
}
