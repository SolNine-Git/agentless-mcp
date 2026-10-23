//! The stdio transport: one session on standard input and output, which ends with its input or at
//! the first write that fails.

use std::pin::pin;
use std::sync::{Arc, OnceLock};

use rmcp::ServiceExt;
use rmcp::service::{RoleServer, RxJsonRpcMessage, ServerInitializeError, TxJsonRpcMessage};
use rmcp::transport::{IntoTransport, Transport};
use tokio::sync::Notify;

use crate::Server;
use crate::serve::Ended;
use crate::service::first;

// A client that closes before it initializes ends the server as one that closes later does. A
// failed write ends it as a client that stops reading, whatever else ended the session.
pub(crate) async fn serve<T, A>(server: Server, transport: T) -> Ended
where
    T: IntoTransport<RoleServer, std::io::Error, A>,
{
    let broken = Arc::new(Broken::default());
    let watched = Watched {
        inner: transport.into_transport(),
        broken: Arc::clone(&broken),
    };
    let ended = match server.serve(watched).await {
        Ok(running) => {
            drop(running.waiting().await);
            Ended::Closed
        }
        Err(ServerInitializeError::ConnectionClosed(_)) => Ended::Closed,
        Err(error) => Ended::Failed(format!("agentless-mcp-server: {error}")),
    };
    broken.over(ended)
}

// The first failed write of a session, and the wake-up of the read that waits for it.
#[derive(Default)]
struct Broken {
    error: OnceLock<String>,
    written: Notify,
}

impl Broken {
    fn record(&self, error: &std::io::Error) {
        if self.error.set(io::os_error_line(error)).is_ok() {
            self.written.notify_one();
        }
    }

    fn is_broken(&self) -> bool {
        self.error.get().is_some()
    }

    // Ends when a write has failed. A permit that `record` stores wakes a wait that starts later.
    async fn failed(&self) {
        if !self.is_broken() {
            self.written.notified().await;
        }
    }

    fn over(&self, ended: Ended) -> Ended {
        match self.error.get() {
            Some(error) => Ended::Unwritable(format!(
                "agentless-mcp-server: cannot write to standard output: {error}"
            )),
            None => ended,
        }
    }
}

// The session's transport, which records its first failed write and then ends its input.
struct Watched<T> {
    inner: T,
    broken: Arc<Broken>,
}

impl<T> Transport<RoleServer> for Watched<T>
where
    T: Transport<RoleServer, Error = std::io::Error>,
{
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let sent = self.inner.send(item);
        let broken = Arc::clone(&self.broken);
        async move {
            let result = sent.await;
            if let Err(error) = &result {
                broken.record(error);
            }
            result
        }
    }

    // After a failed write the input ends, even with lines still buffered.
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        if self.broken.is_broken() {
            return None;
        }
        let failed = pin!(self.broken.failed());
        first(self.inner.receive(), failed).await.flatten()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}
