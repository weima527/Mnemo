//! The daemon: accept loop, per-connection dispatch, graceful shutdown.

use crate::protocol::{
    self, codes, DaemonStatus, IndexInfo, ProjectInfo, Request, Response, RpcError,
};
use crate::transport;
use mnemo_core::{CoreError, ProjectId};
use mnemo_tenant::{ProjectContext, TenantConfig, TenantManager};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;

/// Shared daemon state handed to every connection task.
struct DaemonState {
    manager: Arc<TenantManager>,
    started: Instant,
    shutdown: Arc<Notify>,
}

/// Run the daemon on `endpoint` until `daemon.shutdown` or Ctrl-C.
pub async fn run(endpoint: &str, manager: Arc<TenantManager>) -> anyhow::Result<()> {
    let listener = transport::bind(endpoint)?;
    let state = Arc::new(DaemonState {
        manager,
        started: Instant::now(),
        shutdown: Arc::new(Notify::new()),
    });
    tracing::info!(endpoint, "mnemo daemon listening");

    loop {
        tokio::select! {
            accepted = transport::accept(&listener) => match accepted {
                Ok(conn) => {
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(conn, state).await {
                            tracing::warn!(error = %e, "connection ended with error");
                        }
                    });
                }
                Err(e) => tracing::warn!(error = %e, "accept failed"),
            },
            _ = state.shutdown.notified() => {
                tracing::info!("shutdown requested via daemon.shutdown");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received Ctrl-C; shutting down");
                break;
            }
        }
    }
    // Let any in-flight response (e.g. the shutdown ack) flush before we exit.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    Ok(())
}

/// Build a default `TenantManager` and run on the default endpoint.
pub async fn run_default() -> anyhow::Result<()> {
    let manager = Arc::new(TenantManager::new(TenantConfig::default()));
    run(&transport::default_endpoint(), manager).await
}

/// Serve framed requests on one connection until EOF.
async fn handle_conn(mut conn: transport::Stream, state: Arc<DaemonState>) -> std::io::Result<()> {
    while let Some(frame) = protocol::read_frame(&mut conn).await? {
        let (response, is_shutdown) = match serde_json::from_slice::<Request>(&frame) {
            Ok(req) => {
                let is_shutdown = req.method == "daemon.shutdown";
                (process(&state, req).await, is_shutdown)
            }
            Err(e) => (
                Response::err(
                    Value::Null,
                    RpcError::new(codes::PARSE_ERROR, format!("invalid request: {e}")),
                ),
                false,
            ),
        };
        let bytes = serde_json::to_vec(&response).expect("response serializes");
        protocol::write_frame(&mut conn, &bytes).await?;
        if is_shutdown {
            // The ack is flushed above; only now trigger the daemon to stop, so
            // the client reliably receives the response.
            state.shutdown.notify_one();
            break;
        }
    }
    Ok(())
}

/// Dispatch one request, recording method + latency + outcome.
async fn process(state: &DaemonState, req: Request) -> Response {
    let started = Instant::now();
    let id = req.id.clone();
    let outcome = dispatch(state, &req.method, req.params).await;
    let elapsed_ms = started.elapsed().as_millis();
    match &outcome {
        Ok(_) => tracing::info!(method = %req.method, elapsed_ms, "ok"),
        Err(e) => {
            tracing::warn!(method = %req.method, elapsed_ms, code = e.code, "err: {}", e.message)
        }
    }
    match outcome {
        Ok(result) => Response::ok(id, result),
        Err(error) => Response::err(id, error),
    }
}

/// Map a method + params to a result value (or an `RpcError`).
async fn dispatch(state: &DaemonState, method: &str, params: Value) -> Result<Value, RpcError> {
    match method {
        "daemon.status" => Ok(to_value(daemon_status(state))),

        // The actual shutdown is triggered by `handle_conn` after this ack is
        // flushed (see there), so the client reliably receives the response.
        "daemon.shutdown" => Ok(json!({ "ok": true })),

        "project.attach" => {
            let p: protocol::PathParams = parse(params)?;
            let ctx = state
                .manager
                .attach(Path::new(&p.path))
                .await
                .map_err(internal)?;
            Ok(to_value(project_info(&ctx)))
        }

        "project.detach" => {
            let p: protocol::DetachParams = parse(params)?;
            let id = ProjectId::from_str(&p.project_id).map_err(|e| {
                RpcError::new(codes::INVALID_PARAMS, format!("bad project_id: {e}"))
            })?;
            state.manager.detach(id).await.map_err(internal)?;
            Ok(json!({ "ok": true }))
        }

        "project.list" => {
            let projects: Vec<ProjectInfo> = state
                .manager
                .list_active()
                .into_iter()
                .map(|s| ProjectInfo {
                    project_id: s.project_id.to_hex(),
                    canonical_path: s.canonical_path.to_string_lossy().into_owned(),
                    symbol_count: s.symbol_count,
                })
                .collect();
            Ok(to_value(projects))
        }

        "project.index" => {
            let p: protocol::IndexParams = parse(params)?;
            let path = p.path.clone();
            let force = p.force;
            let result = tokio::task::spawn_blocking(move || {
                mnemo_index::index_repo(Path::new(&path), force)
            })
            .await
            .expect("index task panicked")
            .map_err(internal)?;
            // Refresh the cached graph so subsequent queries see the new snapshot.
            let ctx = state
                .manager
                .get(Path::new(&p.path))
                .await
                .map_err(internal)?;
            ctx.rehydrate().await.map_err(internal)?;
            Ok(to_value(IndexInfo {
                snapshot: result.snapshot.into(),
                file_count: result.file_count,
                symbol_count: result.symbol_count,
                edge_count: result.edge_count,
            }))
        }

        "query.callers" | "query.callees" | "query.search" | "query.symbol" => {
            let p: protocol::QueryParams = parse(params)?;
            let ctx = state
                .manager
                .get(Path::new(&p.path))
                .await
                .map_err(internal)?;
            let graph = ctx.graph();
            ctx.record_query();
            let result = match method {
                "query.callers" => to_value(mnemo_index::query::callers_in(&graph, &p.query)),
                "query.callees" => to_value(mnemo_index::query::callees_in(&graph, &p.query)),
                "query.search" => to_value(mnemo_index::query::search_in(&graph, &p.query)),
                "query.symbol" => to_value(mnemo_index::query::symbols_named(&graph, &p.query)),
                _ => unreachable!("matched above"),
            };
            Ok(result)
        }

        "overlay.set" => {
            let p: protocol::OverlaySetParams = parse(params)?;
            let ctx = state
                .manager
                .get(Path::new(&p.path))
                .await
                .map_err(internal)?;
            let mut files: Vec<mnemo_index::overlay::OverlayFile> = p
                .files
                .into_iter()
                .map(|f| mnemo_index::overlay::OverlayFile {
                    rel_path: f.rel_path,
                    content: Some(f.content),
                })
                .collect();
            files.extend(
                p.deleted
                    .into_iter()
                    .map(|rel_path| mnemo_index::overlay::OverlayFile {
                        rel_path,
                        content: None,
                    }),
            );
            ctx.set_overlay(files).await.map_err(internal)?;
            Ok(json!({ "ok": true, "symbol_count": ctx.graph().symbol_count() }))
        }

        "overlay.clear" => {
            let p: protocol::PathParams = parse(params)?;
            let ctx = state
                .manager
                .get(Path::new(&p.path))
                .await
                .map_err(internal)?;
            ctx.clear_overlay();
            Ok(json!({ "ok": true }))
        }

        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        )),
    }
}

fn daemon_status(state: &DaemonState) -> DaemonStatus {
    let projects = state
        .manager
        .list_active()
        .into_iter()
        .map(|s| ProjectInfo {
            project_id: s.project_id.to_hex(),
            canonical_path: s.canonical_path.to_string_lossy().into_owned(),
            symbol_count: s.symbol_count,
        })
        .collect();
    DaemonStatus {
        active_projects: state.manager.active_count(),
        max_active_projects: state.manager.max_active_projects(),
        uptime_secs: state.started.elapsed().as_secs(),
        projects,
    }
}

fn project_info(ctx: &ProjectContext) -> ProjectInfo {
    ProjectInfo {
        project_id: ctx.project_id().to_hex(),
        canonical_path: ctx.canonical_path().to_string_lossy().into_owned(),
        symbol_count: ctx.graph().symbol_count(),
    }
}

/// Deserialize method params, mapping failures to an `INVALID_PARAMS` error.
fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("invalid params: {e}")))
}

/// Serialize a result; infallible for our types.
fn to_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("result serializes")
}

/// Map a `CoreError` to an internal `RpcError`.
fn internal(e: CoreError) -> RpcError {
    RpcError::new(codes::INTERNAL_ERROR, e.to_string())
}
