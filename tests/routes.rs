use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_floatpower::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

/// Spin up a mock `srvcs-isnumber` that actually computes its answer: it returns
/// `{"result": <bool>}` where the boolean reflects whether the incoming `value`
/// is a JSON number. This lets the orchestration tests exercise the real
/// validation flow without standing up the rest of the fleet.
async fn spawn_isnumber_mock() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|Json(body): Json<Value>| async move {
            let is_number = body.get("value").map(Value::is_number).unwrap_or(false);
            (StatusCode::OK, Json(json!({ "result": is_number })))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(isnumber_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            isnumber_url: isnumber_url.to_string(),
        },
    )
}

async fn eval(isnumber_url: &str, base: Value, exp: Value) -> (StatusCode, Value) {
    let res = app(isnumber_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "base": base, "exp": exp }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// A base URL with nothing listening — exercises the degraded path.
const DEAD_URL: &str = "http://127.0.0.1:1";

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn index_ok() {
    assert_eq!(status_of("/").await, StatusCode::OK);
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn metrics_ok() {
    assert_eq!(status_of("/metrics").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn generates_request_id_when_absent() {
    let res = app(DEAD_URL)
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        res.headers().contains_key("x-request-id"),
        "response must carry a generated x-request-id"
    );
}

#[tokio::test]
async fn fractional_exponent_is_approximate() {
    let isnumber = spawn_isnumber_mock().await;
    let (status, body) = eval(&isnumber, json!(2), json!(0.5)).await;
    assert_eq!(status, StatusCode::OK);
    let got = body["result"].as_f64().expect("result is a number");
    assert!((got - std::f64::consts::SQRT_2).abs() < 1e-9);
}

#[tokio::test]
async fn whole_exponent_is_exact() {
    let isnumber = spawn_isnumber_mock().await;
    let (status, body) = eval(&isnumber, json!(2), json!(10)).await;
    assert_eq!(status, StatusCode::OK);
    let got = body["result"].as_f64().expect("result is a number");
    assert!((got - 1024.0).abs() < 1e-9);
}

#[tokio::test]
async fn float_base_and_exponent_accepted() {
    let isnumber = spawn_isnumber_mock().await;
    let (status, body) = eval(&isnumber, json!(9.0), json!(0.5)).await;
    assert_eq!(status, StatusCode::OK);
    let got = body["result"].as_f64().expect("result is a number");
    assert!((got - 3.0).abs() < 1e-9);
}

#[tokio::test]
async fn negative_base_fractional_exponent_is_domain_error() {
    let isnumber = spawn_isnumber_mock().await;
    let (status, body) = eval(&isnumber, json!(-8), json!(0.5)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "result is not a real number");
}

#[tokio::test]
async fn rejects_operand_that_is_not_a_number() {
    let isnumber = spawn_isnumber_mock().await;
    let (status, _) = eval(&isnumber, json!("nope"), json!(2)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = eval(&isnumber, json!(2), json!("nope")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn degrades_when_isnumber_is_unreachable() {
    let (status, body) = eval(DEAD_URL, json!(2), json!(10)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-isnumber");
}
