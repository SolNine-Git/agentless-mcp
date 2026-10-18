//! The stdio transport: one session on standard input and output, which ends with its input.

use rmcp::ServiceExt;
use rmcp::service::{RoleServer, ServerInitializeError};
use rmcp::transport::IntoTransport;

use crate::Server;
use crate::serve::Ended;

// A client that closes before it initializes ends the server as one that closes later does.
pub(crate) async fn serve<T, E, A>(server: Server, transport: T) -> Ended
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    match server.serve(transport).await {
        Ok(running) => {
            drop(running.waiting().await);
            Ended::Closed
        }
        Err(ServerInitializeError::ConnectionClosed(_)) => Ended::Closed,
        Err(error) => Ended::Failed(format!("agentless-mcp-server: {error}")),
    }
}
