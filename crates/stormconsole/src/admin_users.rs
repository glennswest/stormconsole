//! Admin → Users (#266): `/api/v1/users`, and its groups (#334):
//! `/api/v1/groups`, an administrator's.
//!
//! Every change is made **as the signed-in administrator** — their minted
//! token, or the install token — so the apiserver's RBAC decides it, as it
//! does for `sc user`. A session with no apiserver identity (the node's own
//! bearer, a client certificate) is told to sign in as admin rather than
//! being lent the console's credential.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use console_core::Viewer;
use serde::Deserialize;
use serde_json::json;

use crate::server::AppState;
use crate::users::{Fail, Users};

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/users", get(list).post(create))
        .route("/api/v1/users/{name}", axum::routing::delete(delete))
        .route("/api/v1/users/{name}/password", put(password))
        .route("/api/v1/users/{name}/disabled", put(disabled))
        .route("/api/v1/users/{name}/roles", put(roles))
        .route("/api/v1/groups", get(groups).post(create_group))
        .route("/api/v1/groups/{name}", axum::routing::delete(delete_group))
        .route("/api/v1/groups/{name}/members", put(group_members))
        .route("/api/v1/groups/{name}/roles", put(group_roles))
        .with_state(state)
}

fn refuse(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(json!({"error": error.into()}))).into_response()
}

fn failed(f: Fail) -> Response {
    refuse(StatusCode::from_u16(f.status()).unwrap_or(StatusCode::BAD_GATEWAY), f.message())
}

/// The users store and the administrator's bearer, or why not.
// The refusal is the Response the handler returns; boxing it would only move the allocation.
#[allow(clippy::result_large_err)]
fn admin(state: &AppState, viewer: &Viewer) -> Result<(Arc<Users>, String), Response> {
    let Some(users) = state.users.clone() else {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            "users are not kept on the apiserver by this console: [api] user_namespace is not set",
        ));
    };
    if !viewer.has_role("admin") {
        return Err(refuse(StatusCode::FORBIDDEN, "Users is an administrator's"));
    }
    let Some(token) = viewer.token.clone() else {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "this session has no apiserver identity to make the change as: sign in as admin, \
             or with the install token, to manage users",
        ));
    };
    Ok((users, token))
}

async fn list(State(state): State<AppState>, viewer: Viewer) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let rows = match users.list(&token).await {
        Ok(r) => r,
        Err(f) => return failed(f),
    };
    // The roles are an offer, not the page: a refusal to list them leaves
    // the users listed.
    let offered = users.offered_roles(&token).await.unwrap_or_default();
    let groups = match users.groups(&token).await {
        Ok(g) => g,
        Err(f) => return failed(f),
    };
    Json(json!({"namespace": users.ns, "users": rows, "groups": groups, "roles": offered, "you": viewer.user})).into_response()
}

#[derive(Deserialize)]
struct NewUser {
    name: String,
    password: String,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    groups: Vec<String>,
}

async fn create(State(state): State<AppState>, viewer: Viewer, Json(b): Json<NewUser>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.create(&token, &b.name, &b.password, &b.roles, &b.groups).await {
        Ok(()) => {
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), user = %b.name, roles = %b.roles.join(","), groups = %b.groups.join(","), "users: created");
            (StatusCode::CREATED, Json(json!({"created": b.name}))).into_response()
        }
        Err(f) => failed(f),
    }
}

async fn delete(State(state): State<AppState>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if viewer.user.as_deref() == Some(name.as_str()) {
        return refuse(StatusCode::BAD_REQUEST, "you cannot delete yourself");
    }
    match users.delete(&token, &name).await {
        Ok(()) => {
            let n = state.sessions.end_user(&name, None);
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), user = %name, sessions = n, "users: deleted");
            Json(json!({"deleted": name})).into_response()
        }
        Err(f) => failed(f),
    }
}

#[derive(Deserialize)]
struct NewPassword {
    password: String,
}

async fn password(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    let keep = crate::auth::cookie_session(&req);
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), 64 * 1024).await else {
        return refuse(StatusCode::BAD_REQUEST, "no body");
    };
    let Ok(b) = serde_json::from_slice::<NewPassword>(&bytes) else {
        return refuse(StatusCode::BAD_REQUEST, "expected {\"password\": \"…\"}");
    };
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.set_password(&token, &name, &b.password).await {
        Ok(stamp) => {
            // Their other sessions end here: a reset is usually because
            // the old password got out. The administrator's own stays, with
            // the new password's stamp. Every other node's sweep ends theirs
            // there, the Secret's hash being no longer the one they signed
            // in with (#374).
            let n = state.sessions.end_user(&name, keep.as_deref());
            if let Some(id) = &keep {
                state.sessions.restamp(id, &name, &stamp);
            }
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), user = %name, sessions = n, "users: password set");
            Json(json!({"password": "set"})).into_response()
        }
        Err(f) => failed(f),
    }
}

#[derive(Deserialize)]
struct Disabled {
    disabled: bool,
}

async fn disabled(
    State(state): State<AppState>,
    viewer: Viewer,
    Path(name): Path<String>,
    Json(b): Json<Disabled>,
) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if b.disabled && viewer.user.as_deref() == Some(name.as_str()) {
        return refuse(StatusCode::BAD_REQUEST, "you cannot disable yourself");
    }
    match users.set_disabled(&token, &name, b.disabled).await {
        Ok(()) => {
            let n = if b.disabled { state.sessions.end_user(&name, None) } else { 0 };
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), user = %name, disabled = b.disabled, sessions = n, "users: disabled set");
            Json(json!({"disabled": b.disabled})).into_response()
        }
        Err(f) => failed(f),
    }
}

#[derive(Deserialize)]
struct Roles {
    roles: Vec<String>,
}

async fn roles(State(state): State<AppState>, viewer: Viewer, Path(name): Path<String>, Json(b): Json<Roles>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.set_roles(&token, &name, &b.roles).await {
        Ok(()) => {
            // In effect for their sessions here now; elsewhere at the next sweep.
            if let Ok(r) = users.roles(&name).await {
                state.sessions.set_roles(&name, &r);
            }
            state.registry.forget_reviews().await;
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), user = %name, roles = %b.roles.join(","), "users: roles set");
            Json(json!({"roles": b.roles})).into_response()
        }
        Err(f) => failed(f),
    }
}

/// Roles changed for these users: in effect for their sessions here now,
/// elsewhere at the next sweep (30 s).
async fn refresh(state: &AppState, users: &Users, who: &[String]) {
    for u in who {
        if let Ok(r) = users.roles(u).await {
            state.sessions.set_roles(u, &r);
        }
    }
    state.registry.forget_reviews().await;
}

async fn groups(State(state): State<AppState>, viewer: Viewer) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.groups(&token).await {
        Ok(g) => Json(json!({"namespace": users.ns, "groups": g})).into_response(),
        Err(f) => failed(f),
    }
}

#[derive(Deserialize)]
struct NewGroup {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    roles: Vec<String>,
}

async fn create_group(State(state): State<AppState>, viewer: Viewer, Json(b): Json<NewGroup>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.create_group(&token, &b.name, &b.description, &b.members, &b.roles).await {
        Ok(()) => {
            refresh(&state, &users, &b.members).await;
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), group = %b.name, members = %b.members.join(","), roles = %b.roles.join(","), "users: group created");
            (StatusCode::CREATED, Json(json!({"created": b.name}))).into_response()
        }
        Err(f) => failed(f),
    }
}

async fn delete_group(State(state): State<AppState>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.delete_group(&token, &name).await {
        Ok(was) => {
            refresh(&state, &users, &was).await;
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), group = %name, "users: group deleted");
            Json(json!({"deleted": name})).into_response()
        }
        Err(f) => failed(f),
    }
}

#[derive(Deserialize)]
struct Members {
    members: Vec<String>,
}

async fn group_members(State(state): State<AppState>, viewer: Viewer, Path(name): Path<String>, Json(b): Json<Members>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.set_members(&token, &name, &b.members).await {
        Ok(touched) => {
            refresh(&state, &users, &touched).await;
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), group = %name, members = %b.members.join(","), "users: group members set");
            Json(json!({"members": b.members})).into_response()
        }
        Err(f) => failed(f),
    }
}

async fn group_roles(State(state): State<AppState>, viewer: Viewer, Path(name): Path<String>, Json(b): Json<Roles>) -> Response {
    let (users, token) = match admin(&state, &viewer) {
        Ok(x) => x,
        Err(r) => return r,
    };
    match users.set_group_roles(&token, &name, &b.roles).await {
        Ok(members) => {
            refresh(&state, &users, &members).await;
            tracing::info!(by = viewer.user.as_deref().unwrap_or("?"), group = %name, roles = %b.roles.join(","), "users: group roles set");
            Json(json!({"roles": b.roles})).into_response()
        }
        Err(f) => failed(f),
    }
}
