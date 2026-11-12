use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};

use crate::AppState;

const INDEX: &str = include_str!("../web/index.html");
const STYLES: &str = include_str!("../web/styles.css");
const SCRIPT: &str = include_str!("../web/app.js");
const LOGO: &[u8] = include_bytes!("../web/assets/sentinela-logo.png");
const FONT: &[u8] = include_bytes!("../web/assets/inter-latin.woff2");

pub(crate) async fn index(State(state): State<AppState>) -> Response {
    let html = INDEX.replace("__SENTINELA_TOKEN_VALUE__", state.token.as_str());
    text_response(html, "text/html; charset=utf-8")
}

pub(crate) async fn styles() -> Response {
    text_response(STYLES.to_string(), "text/css; charset=utf-8")
}

pub(crate) async fn script() -> Response {
    text_response(SCRIPT.to_string(), "text/javascript; charset=utf-8")
}

pub(crate) async fn logo() -> Response {
    bytes_response(LOGO, "image/png")
}

pub(crate) async fn font() -> Response {
    bytes_response(FONT, "font/woff2")
}

fn text_response(body: String, content_type: &'static str) -> Response {
    let mut response = body.into_response();
    set_headers(&mut response, content_type);
    response
}

fn bytes_response(body: &'static [u8], content_type: &'static str) -> Response {
    let mut response = body.into_response();
    set_headers(&mut response, content_type);
    response
}

fn set_headers(response: &mut Response, content_type: &'static str) {
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
