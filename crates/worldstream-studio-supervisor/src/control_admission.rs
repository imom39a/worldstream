//! Installation control admission around the fully composed Supervisor router.
//!
//! This layer is always hardened. Only the temporary compile-time delivery gate
//! in the process shell selects whether it is installed before Studio retirement.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::control_access::ControlAccess;

/// Protect the complete route graph, including unknown paths and method fallbacks.
/// No operator routes may be merged outside this boundary afterward.
pub fn protect_operator_routes(router: Router, control: ControlAccess) -> Router {
    protect(router, control, false)
}

pub(crate) fn protect_managed_operator_routes(
    router: Router,
    control: ControlAccess,
    proof: crate::verified_control::ProofService,
) -> Router {
    // The only added unauthenticated route is this fixed bounded proof handler.
    // Callers cannot substitute an operator handler behind its exception.
    protect(
        router.merge(crate::managed_http::local_proof_router(proof)),
        control,
        true,
    )
}

struct Admission {
    control: ControlAccess,
    process_proof: bool,
}

fn protect(router: Router, control: ControlAccess, process_proof: bool) -> Router {
    router.layer(from_fn_with_state(
        Arc::new(Admission {
            control,
            process_proof,
        }),
        admit_control,
    ))
}

async fn admit_control(
    State(admission): State<Arc<Admission>>,
    mut request: Request,
    next: Next,
) -> Response {
    if admission.process_proof
        && request.method() == axum::http::Method::POST
        && request.uri().path() == "/api/v1/control/proof"
    {
        request.headers_mut().remove(header::AUTHORIZATION);
        return next.run(request).await;
    }
    if is_membership_browser_request(request.method().as_str(), request.uri().path()) {
        // These handlers retain their exact Origin, one-use handoff and opaque
        // Membership session checks. Installation authority never reaches them.
        request.headers_mut().remove(header::AUTHORIZATION);
        return next.run(request).await;
    }
    let headers = request.headers().clone();
    let authentication =
        tokio::task::spawn_blocking(move || admission.control.authenticate(&headers)).await;
    match authentication {
        Ok(Ok(true)) => {
            // Downstream operator adapters use their separately scoped authorities.
            request.headers_mut().remove(header::AUTHORIZATION);
            next.run(request).await
        }
        Ok(Ok(false)) => admission_error(StatusCode::UNAUTHORIZED),
        Ok(Err(_)) | Err(_) => admission_error(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn is_membership_browser_request(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET" | "OPTIONS", "/api/v1/participant-console/session")
            | (
                "POST" | "OPTIONS",
                "/api/v1/participant-console/handoffs:redeem"
                    | "/api/v1/participant-console/session:observe"
                    | "/api/v1/participant-console/session:acknowledge"
                    | "/api/v1/participant-console/session:act"
                    | "/api/v1/participant-console/session:replay"
            )
    )
}

#[derive(Serialize)]
struct AdmissionError {
    schema: &'static str,
    code: &'static str,
    message: &'static str,
}

fn admission_error(status: StatusCode) -> Response {
    let (code, message) = if status == StatusCode::UNAUTHORIZED {
        (
            "control_authentication_required",
            "Operator control authentication is required.",
        )
    } else {
        (
            "control_authentication_unavailable",
            "Operator control authentication is unavailable.",
        )
    };
    let mut response = (
        status,
        Json(AdmissionError {
            schema: "worldstream/operator-control-error/v1",
            code,
            message,
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"worldstream-operator-control\""),
        );
    }
    response
}
