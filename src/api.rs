use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-floatpower";
pub const CONCERN: &str = "float arithmetic: base raised to exp";
pub const DEPENDS_ON: &[&str] = &["srvcs-isnumber"];

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub isnumber_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    #[schema(value_type = Object)]
    pub base: Value,
    #[schema(value_type = Object)]
    pub exp: Value,
}

#[derive(Serialize, ToSchema)]
pub struct PowerResponse {
    #[schema(value_type = Object)]
    pub base: Value,
    #[schema(value_type = Object)]
    pub exp: Value,
    pub result: f64,
}

/// The single concern: `base` raised to the power `exp`, over the reals.
///
/// Returns `None` when the result is not a real number (e.g. a negative base
/// raised to a fractional exponent yields `NaN`).
pub fn floatpower(base: f64, exp: f64) -> Option<f64> {
    let result = base.powf(exp);
    if result.is_nan() {
        None
    } else {
        Some(result)
    }
}

fn ok(base: Value, exp: Value, result: f64) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "base": base, "exp": exp, "result": result })),
    )
        .into_response()
}

fn invalid(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": reason })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

/// Forward a dependency's response verbatim (used to propagate `422` for invalid
/// input, so floatpower reports the same rejection its dependency did).
fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

/// Validate one operand is a number by asking `srvcs-isnumber`, mapping its
/// failures to the response this service should return.
async fn ask_is_number(url: &str, value: &Value, dependency: &str) -> Result<(), Response> {
    match client::call(url, &json!({ "value": value })).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => {
            let is_number = body.get("result").and_then(Value::as_bool).unwrap_or(false);
            if is_number {
                Ok(())
            } else {
                Err(invalid("value is not a number"))
            }
        }
        // Invalid input propagates from the leaf dependency; forward it.
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — compute `base.powf(exp)`.
///
/// Input validation is delegated to `srvcs-isnumber` over HTTP (the single
/// source of truth for "is this a number"), once per operand. Both integer and
/// float inputs are accepted — this is a float service. If the dependency is
/// unreachable, this service reports itself degraded rather than guessing. If
/// the computed result is not a real number (`NaN`, e.g. a negative base with a
/// fractional exponent), it is rejected as a domain error.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = PowerResponse),
        (status = 422, description = "an operand is not a number, or the result is not a real number"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    // 1. Delegate "is this a number" to srvcs-isnumber, once per operand.
    if let Err(resp) = ask_is_number(&deps.isnumber_url, &req.base, "srvcs-isnumber").await {
        return resp;
    }
    if let Err(resp) = ask_is_number(&deps.isnumber_url, &req.exp, "srvcs-isnumber").await {
        return resp;
    }

    // 2. Coerce both operands to f64 (integers and floats both accepted).
    let Some(base) = req.base.as_f64() else {
        return invalid("base is not a number");
    };
    let Some(exp) = req.exp.as_f64() else {
        return invalid("exp is not a number");
    };

    // 3. Compute; reject results that are not real numbers.
    match floatpower(base, exp) {
        Some(result) => ok(req.base, req.exp, result),
        None => invalid("result is not a real number"),
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, PowerResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[test]
    fn power_of_whole_exponent_is_exact() {
        assert_eq!(floatpower(2.0, 10.0), Some(1024.0));
        assert_eq!(floatpower(5.0, 0.0), Some(1.0));
        assert_eq!(floatpower(3.0, 2.0), Some(9.0));
        assert_eq!(floatpower(2.0, -1.0), Some(0.5));
    }

    #[test]
    fn power_of_fractional_exponent_is_approximate() {
        let got = floatpower(2.0, 0.5).expect("2^0.5 is real");
        assert!((got - std::f64::consts::SQRT_2).abs() < 1e-9);

        let cube_root = floatpower(27.0, 1.0 / 3.0).expect("27^(1/3) is real");
        assert!((cube_root - 3.0).abs() < 1e-9);
    }

    #[test]
    fn negative_base_with_fractional_exponent_is_not_real() {
        assert_eq!(floatpower(-8.0, 0.5), None);
        assert_eq!(floatpower(-2.0, 0.3), None);
    }

    #[test]
    fn negative_base_with_integer_exponent_is_real() {
        assert_eq!(floatpower(-2.0, 3.0), Some(-8.0));
        assert_eq!(floatpower(-2.0, 2.0), Some(4.0));
    }

    #[tokio::test]
    async fn index_reports_dependency() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-floatpower");
        assert_eq!(info.concern, "float arithmetic: base raised to exp");
        assert_eq!(info.depends_on, vec!["srvcs-isnumber"]);
    }
}
