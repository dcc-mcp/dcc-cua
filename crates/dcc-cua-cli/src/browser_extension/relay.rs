//! Native messages stay serialized; a pairing can never migrate between MCP owners.
use std::time::Duration;

use dcc_cua_client::HostClient;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

use super::{read_native_message, write_native_message};

struct Command {
    owner: String,
    deadline: u64,
    body: Value,
    reply: oneshot::Sender<Value>,
}

pub(super) async fn run(
    hello: Value,
    origin: String,
    browser_pid: u32,
    browser_window: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, mut commands) = mpsc::channel::<Command>(16);
    let (native_tx, mut native_rx) = mpsc::channel(1);
    let mut workers = JoinSet::new();
    let (shutdown, _) = watch::channel(false);
    let mut selected = false;
    let mut paired_owner: Option<String> = None;
    let reader = tokio::spawn(async move {
        loop {
            let message = read_native_message(&mut tokio::io::stdin()).await.ok();
            let ended = message.is_none();
            if native_tx.send(message).await.is_err() || ended {
                break;
            }
        }
    });
    let mut output = tokio::io::stdout();
    let mut discover = tokio::time::interval(Duration::from_secs(2));
    let result = async {
        loop {
            tokio::select! {
                _ = discover.tick() => {
                    if selected { continue; }
                    let mut endpoints = Vec::new();
                    for record in crate::extension_transport::candidates()? {
                        let connected = tokio::time::timeout(Duration::from_millis(500), async {
                            let mut client = crate::extension_transport::connect(&record).await?;
                            let binding = client.request("browser_extension_transport_binding", json!({})).await.map_err(|error| error.to_string())?;
                            Ok::<_, String>((client, binding.value))
                        }).await;
                        if let Ok(Ok((client, binding))) = connected
                            && binding["process_id"].as_u64() == Some(u64::from(browser_pid)) && binding["window_handle"].as_u64() == Some(browser_window) {
                                endpoints.push((record, client));
                        }
                    }
                    if endpoints.len() > 1 {
                        return Err(std::io::Error::other("multiple matching extension tasks; revoke ambiguity and pair again").into());
                    }
                    if let Some((record, client)) = endpoints.pop() {
                        selected = true;
                        let (tx, hello, origin) = (tx.clone(), hello.clone(), origin.clone());
                        let stopped = shutdown.subscribe();
                        workers.spawn(async move {
                            let _ = worker(client, &record, hello, origin, (browser_pid, browser_window), tx, stopped).await;
                        });
                    }
                }
                _ = workers.join_next(), if selected => {
                    return Err(std::io::Error::other("paired task disconnected; pair the tab again").into());
                }
                message = native_rx.recv() => {
                    if message.flatten().is_none() { return Ok(()); }
                    return Err(std::io::Error::other("unsolicited native response").into());
                }
                Some(command) = commands.recv() => {
                    if matches!(command.body["method"].as_str(), Some("click" | "type" | "navigate"))
                        && super::current_browser_window(browser_pid).await.ok() != Some(browser_window) {
                        let _ = command.reply.send(json!({"schema":"dcc-cua.browser-extension.v1","type":"response","request_id":command.body["request_id"],"ok":false,"error":{"code":"target_not_foreground","message":"paired native window is no longer foreground; take a fresh observation"}}));
                        continue;
                    }
                    dispatch(command, &mut paired_owner, &mut output, &mut native_rx).await?;
                }
            }
        }
    }.await;
    reader.abort();
    commands.close();
    while commands.try_recv().is_ok() {}
    let _ = shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while workers.join_next().await.is_some() {}
    })
    .await;
    workers.abort_all();
    while workers.join_next().await.is_some() {}
    result
}

async fn worker(
    mut client: HostClient,
    record: &crate::connection_diagnostics::ConnectionRecord,
    hello: Value,
    origin: String,
    browser: (u32, u64),
    commands: mpsc::Sender<Command>,
    mut stopped: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let registered = client
        .request(
            "register_browser_extension",
            json!({"hello":hello,"invocation_origin":origin,"browser_process_id":browser.0,"browser_window_handle":browser.1}),
        )
        .await?;
    let provider_id = registered.value["provider_id"]
        .as_str()
        .ok_or("missing provider id")?
        .to_owned();
    let secret = registered.value["provider_secret"]
        .as_str()
        .ok_or("missing provider secret")?
        .to_owned();
    let result = tokio::select! {
        _ = stopped.changed() => Ok(()),
        result = async {
    loop {
        let next = client
            .request(
                "browser_extension_next",
                json!({"provider_id":provider_id,"provider_secret":secret,"timeout_ms":1000}),
            )
            .await?;
        let mut body = next.value["command"].clone();
        if body.is_null() {
            continue;
        }
        let task_owner = body
            .as_object_mut()
            .and_then(|body| body.remove("_dcc_task_owner"))
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or("missing task owner binding")?;
        let deadline = body
            .as_object_mut()
            .and_then(|body| body.remove("_dcc_deadline_unix_ms"))
            .and_then(|value| value.as_u64())
            .ok_or("missing command deadline")?;
        let owner = format!("{}:{task_owner}", record.connection_id);
        let (reply, receive) = oneshot::channel();
        commands
            .send(Command {
                owner,
                deadline,
                body,
                reply,
            })
            .await?;
        let response = receive.await?;
        client
            .request(
                "complete_browser_extension",
                json!({"provider_id":provider_id,"provider_secret":secret,"response":response}),
            )
            .await?;
    }
        #[allow(unreachable_code)]
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        } => result,
    };
    let _ = unregister(record, &provider_id, &secret).await;
    result
}

async fn unregister(
    record: &crate::connection_diagnostics::ConnectionRecord,
    provider_id: &str,
    secret: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Cancelling a poll invalidates its framed connection. Cleanup must prove
    // the same endpoint's OS peer and full birth again on a fresh connection.
    tokio::time::timeout(Duration::from_millis(500), async {
        let mut cleanup = crate::extension_transport::connect(record).await?;
        cleanup
            .request(
                "unregister_browser_extension",
                json!({"provider_id":provider_id,"provider_secret":secret}),
            )
            .await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    Ok(())
}

async fn dispatch<W: tokio::io::AsyncWrite + Unpin>(
    command: Command,
    owner: &mut Option<String>,
    output: &mut W,
    responses: &mut mpsc::Receiver<Option<Value>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    if command.deadline <= now {
        let _ = command.reply.send(json!({"schema":"dcc-cua.browser-extension.v1","type":"response","request_id":command.body["request_id"],"ok":false,"error":{"code":"command_expired","message":"expired command was not dispatched; take a fresh observation"}}));
        return Ok(());
    }
    if owner.as_ref().is_some_and(|owner| owner != &command.owner) {
        let _ = command.reply.send(json!({"schema":"dcc-cua.browser-extension.v1","type":"response","request_id":command.body["request_id"],"ok":false,"error":{"code":"pairing_owned","message":"pairing belongs to another task; pair the tab again to change owner"}}));
        return Ok(());
    }
    owner.get_or_insert(command.owner);
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis((command.deadline - now).min(30_000));
    tokio::time::timeout_at(deadline, write_native_message(output, &command.body))
        .await
        .map_err(|_| {
            std::io::Error::other("native write timeout; outcome unknown; do not retry")
        })??;
    let response = tokio::time::timeout_at(deadline, responses.recv())
        .await
        .map_err(|_| {
            std::io::Error::other("extension response timeout; outcome unknown; do not retry")
        })?
        .flatten()
        .ok_or_else(|| std::io::Error::other("extension disconnected; outcome unknown"))?;
    if response["request_id"] != command.body["request_id"] {
        return Err(
            std::io::Error::other("native response correlation mismatch; outcome unknown").into(),
        );
    }
    let _ = command.reply.send(response);
    Ok(())
}

#[cfg(test)]
mod tests;
