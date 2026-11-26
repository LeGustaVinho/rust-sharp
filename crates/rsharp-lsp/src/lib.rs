pub mod analysis;
pub mod builtins;
pub mod diagnostics;
pub mod handlers;
pub mod position;
pub mod semantic_tokens;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::InitializeParams;
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as LspNotification, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as LspRequest,
    SemanticTokensFullRequest,
};

use crate::handlers::{ServerState, server_capabilities};

/// Runs the R# Language Server over standard input and output.
pub fn run_stdio() -> Result<(), String> {
    let (connection, io_threads) = Connection::stdio();
    let result = run_connection(&connection);
    drop(connection);
    let _ = io_threads.join();
    result
}

/// Runs the full LSP initialize -> serve -> shutdown loop over any `lsp_server::Connection`.
pub fn run_connection(connection: &Connection) -> Result<(), String> {
    let caps = serde_json::to_value(server_capabilities()).unwrap();
    let init_value = connection
        .initialize(caps)
        .map_err(|e| format!("LSP initialize failed: {}", e))?;

    let mut state = ServerState::default();
    if let Ok(init_params) = serde_json::from_value::<InitializeParams>(init_value)
        && let Some(opts) = init_params.initialization_options
    {
        state.apply_settings(&opts);
    }

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req).unwrap_or(false) {
                    return Ok(());
                }
                let resp = dispatch_request(&state, req);
                let _ = connection.sender.send(Message::Response(resp));
            }
            Message::Notification(notif) => {
                if let Some(diag_params) = dispatch_notification(&mut state, notif) {
                    let out =
                        Notification::new(PublishDiagnostics::METHOD.to_string(), diag_params);
                    let _ = connection.sender.send(Message::Notification(out));
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn dispatch_notification(
    state: &mut ServerState,
    notif: Notification,
) -> Option<lsp_types::PublishDiagnosticsParams> {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = serde_json::from_value(notif.params).ok()?;
            Some(state.did_open(params))
        }
        DidChangeTextDocument::METHOD => {
            let params = serde_json::from_value(notif.params).ok()?;
            state.did_change(params)
        }
        DidSaveTextDocument::METHOD => {
            let params = serde_json::from_value(notif.params).ok()?;
            state.did_save(params)
        }
        DidCloseTextDocument::METHOD => {
            let params = serde_json::from_value(notif.params).ok()?;
            Some(state.did_close(params))
        }
        DidChangeConfiguration::METHOD => {
            if let Ok(params) = serde_json::from_value(notif.params) {
                state.did_change_configuration(params);
            }
            None
        }
        _ => None,
    }
}

fn dispatch_request(state: &ServerState, req: Request) -> Response {
    let id = req.id;
    match req.method.as_str() {
        DocumentSymbolRequest::METHOD => handle_req(id, req.params, |p| state.document_symbol(p)),
        HoverRequest::METHOD => handle_req(id, req.params, |p| state.hover(p)),
        GotoDefinition::METHOD => handle_req(id, req.params, |p| state.goto_definition(p)),
        Completion::METHOD => handle_req(id, req.params, |p| state.completion(p)),
        SemanticTokensFullRequest::METHOD => {
            handle_req(id, req.params, |p| state.semantic_tokens_full(p))
        }
        other => Response::new_err(
            id,
            ErrorCode::MethodNotFound as i32,
            format!("Unsupported LSP method '{}'", other),
        ),
    }
}

fn handle_req<P, R>(
    id: lsp_server::RequestId,
    raw_params: serde_json::Value,
    handler: impl FnOnce(P) -> R,
) -> Response
where
    P: serde::de::DeserializeOwned,
    R: serde::Serialize,
{
    match serde_json::from_value::<P>(raw_params) {
        Ok(params) => Response::new_ok(id, handler(params)),
        Err(err) => Response::new_err(
            id,
            ErrorCode::InvalidParams as i32,
            format!("Invalid params: {}", err),
        ),
    }
}
