use serde::Deserialize;
use std::{
    collections::HashMap,
    time::Duration,
};

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

#[derive(Deserialize)]
struct JsonSessionDescription {
    #[serde(default)]
    sdp: String,
}

const WEBRTC_BACKEND_ENDPOINTS: &[&str] = &[
    "https://wildsafe-ml-service-4z6c.onrender.com/predict/webrtc/offer",
];

#[tauri::command]
fn exchange_h264_webrtc_offer(offer_sdp: String) -> Result<String, String> {
    let mut errors = Vec::new();

    for endpoint in WEBRTC_BACKEND_ENDPOINTS {
        match exchange_offer_with_endpoint(endpoint, &offer_sdp) {
            Ok(answer_sdp) => return Ok(answer_sdp),
            Err(error) => errors.push(format!("{}: {}", endpoint, error)),
        }
    }

    Err(format!(
        "Could not establish WebRTC signaling with the configured backend. {}",
        errors.join(" | ")
    ))
}

fn exchange_offer_with_endpoint(endpoint: &str, offer_sdp: &str) -> Result<String, String> {
    let json_body = serde_json::json!({
        "type": "offer",
        "sdp": offer_sdp,
        "sample_fps": 3.0,
        "confidence_threshold": 0.1,
        "camera_id": "macbook-pro-camera",
        "latitude": 37.7749,
        "longitude": -122.4194,
        "road_name": "Smart Wild Desktop Test",
        "direction": "desktop",
        "mile_marker": "local",
        "use_pose_detection": false,
    })
    .to_string();

    let json_response = post_http(
        endpoint,
        "application/json",
        "application/json, application/sdp",
        &json_body,
    )?;

    if json_response.status >= 400 {
        return Err(format!(
            "Signaling server returned HTTP {}: {}",
            json_response.status, json_response.body
        ));
    }

    parse_answer(json_response)
}

fn parse_answer(response: HttpResponse) -> Result<String, String> {
    let content_type = response
        .headers
        .get("content-type")
        .map(String::as_str)
        .unwrap_or("");

    if content_type.contains("application/json") || response.body.trim_start().starts_with('{') {
        let answer: JsonSessionDescription =
            serde_json::from_str(&response.body).map_err(|error| error.to_string())?;
        if answer.sdp.trim().is_empty() {
            return Err("JSON answer did not include an SDP body.".into());
        }
        return Ok(answer.sdp);
    }

    if response.body.trim().is_empty() {
        return Err("Signaling server returned an empty SDP answer.".into());
    }

    Ok(response.body)
}

fn post_http(
    endpoint: &str,
    content_type: &str,
    accept: &str,
    body: &str,
) -> Result<HttpResponse, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;

    let mut headers = HashMap::new();
    let response = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .header(reqwest::header::ACCEPT, accept)
        .body(body.to_string())
        .send()
        .map_err(|error| format!("Could not post WebRTC offer: {}", error))?;
    let status = response.status().as_u16();

    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            headers.insert(name.as_str().to_ascii_lowercase(), value.to_string());
        }
    }

    let body = response.text().map_err(|error| error.to_string())?;

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![exchange_h264_webrtc_offer])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
