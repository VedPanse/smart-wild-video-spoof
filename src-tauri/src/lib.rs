use serde::Deserialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

#[derive(Debug)]
struct HttpUrl {
    host: String,
    port: u16,
    path: String,
}

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
    "http://localhost:8000/predict/webrtc/offer",
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
        "Could not establish WebRTC signaling with the Rust backend on port 8000. {}",
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
    let url = parse_http_url(endpoint)?;
    let mut stream = TcpStream::connect((url.host.as_str(), url.port))
        .map_err(|error| format!("Could not connect to {}: {}", endpoint, error))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: {}\r\nAccept: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        url.path,
        url.host,
        url.port,
        content_type,
        accept,
        body.as_bytes().len(),
        body
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut raw_response = Vec::new();
    stream
        .read_to_end(&mut raw_response)
        .map_err(|error| error.to_string())?;

    parse_http_response(&raw_response)
}

fn parse_http_url(endpoint: &str) -> Result<HttpUrl, String> {
    let endpoint = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| "Only plain HTTP signaling endpoints are supported.".to_string())?;
    let (authority, path) = endpoint.split_once('/').unwrap_or((endpoint, ""));
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| "Signaling endpoint must include a port, e.g. http://localhost:8000.".to_string())?;
    let port = port
        .parse::<u16>()
        .map_err(|_| "Signaling endpoint port is not valid.".to_string())?;

    Ok(HttpUrl {
        host: host.to_string(),
        port,
        path: format!("/{}", path),
    })
}

fn parse_http_response(raw_response: &[u8]) -> Result<HttpResponse, String> {
    let separator = raw_response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "Invalid HTTP response from signaling server.".to_string())?;
    let header_text = String::from_utf8_lossy(&raw_response[..separator]);
    let body = &raw_response[separator + 4..];
    let mut header_lines = header_text.lines();
    let status_line = header_lines
        .next()
        .ok_or_else(|| "Missing HTTP status from signaling server.".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "Invalid HTTP status from signaling server.".to_string())?
        .parse::<u16>()
        .map_err(|_| "Invalid HTTP status code from signaling server.".to_string())?;

    let mut headers = HashMap::new();
    for line in header_lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        decode_chunked_body(body)?
    } else {
        body.to_vec()
    };

    let body = String::from_utf8(body).map_err(|error| error.to_string())?;

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    let mut cursor = 0;

    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "Invalid chunked response from signaling server.".to_string())?
            + cursor;
        let size_text = String::from_utf8_lossy(&body[cursor..line_end]);
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| "Invalid chunk size from signaling server.".to_string())?;
        cursor = line_end + 2;

        if size == 0 {
            break;
        }

        let chunk_end = cursor + size;
        if chunk_end + 2 > body.len() {
            return Err("Chunked response ended early.".into());
        }

        decoded.extend_from_slice(&body[cursor..chunk_end]);
        cursor = chunk_end + 2;
    }

    Ok(decoded)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![exchange_h264_webrtc_offer])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
